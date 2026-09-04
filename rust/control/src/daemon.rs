use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

use devcoordinator2_api::{
    EmptyParams, ErrorCode, MAX_REQUEST_BYTES, PingData, ProtocolError, RequestEnvelope,
    ResponseEnvelope, encode_response, parse_request,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::{error, info};

use crate::access::Caller;
use crate::{DATABASE_SCHEMA_VERSION, SOURCE_COMMIT};

const READ_DEADLINE: Duration = Duration::from_secs(5);
const WRITE_DEADLINE: Duration = Duration::from_secs(10);

/// Synchronous domain boundary executed on Tokio's blocking pool. Domain
/// services own their own serialization and may invoke bounded local process
/// adapters without blocking the async socket reactor.
pub trait OperationExecutor: Send + Sync + 'static {
    fn execute(
        &self,
        operation: &str,
        params: Value,
        caller: &Caller,
    ) -> Result<Value, ProtocolError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone)]
struct PingExecutor {
    socket_display: Arc<str>,
}

impl OperationExecutor for PingExecutor {
    fn execute(
        &self,
        operation: &str,
        params: Value,
        _caller: &Caller,
    ) -> Result<Value, ProtocolError> {
        if operation != "ping" {
            return Err(ProtocolError::new(
                ErrorCode::InternalError,
                "operation handler is not installed",
            ));
        }
        serde_json::from_value::<EmptyParams>(params).map_err(|error| {
            ProtocolError::new(ErrorCode::ParamsInvalid, "ping parameters are invalid")
                .with_detail(error.to_string())
        })?;
        serde_json::to_value(PingData {
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: devcoordinator2_api::PROTOCOL_VERSION,
            schema_version: DATABASE_SCHEMA_VERSION,
            executor_schema: devcoordinator2_executor_protocol::EXECUTOR_SCHEMA,
            source_commit: SOURCE_COMMIT.to_owned(),
            socket: self.socket_display.to_string(),
        })
        .map_err(|error| {
            ProtocolError::new(ErrorCode::InternalError, "cannot encode ping result")
                .with_detail(error.to_string())
        })
    }
}

#[derive(Clone)]
pub struct App {
    edge_uid: Option<u32>,
    executor: Arc<dyn OperationExecutor>,
}

impl App {
    pub fn new(socket_path: &Path) -> Self {
        let socket_display: Arc<str> = socket_path.display().to_string().into();
        Self {
            edge_uid: None,
            executor: Arc::new(PingExecutor { socket_display }),
        }
    }

    pub fn with_executor(edge_uid: Option<u32>, executor: Arc<dyn OperationExecutor>) -> Self {
        Self { edge_uid, executor }
    }

    pub async fn dispatch(
        &self,
        request: RequestEnvelope,
        peer: PeerCredentials,
    ) -> ResponseEnvelope {
        let id = request.id.clone();
        let caller = match Caller::from_client(
            peer.pid,
            peer.uid,
            peer.gid,
            request.client,
            self.edge_uid,
        ) {
            Ok(caller) => caller,
            Err(error) => return ResponseEnvelope::failure(id, error),
        };
        let executor = Arc::clone(&self.executor);
        let operation = request.operation;
        let result = tokio::task::spawn_blocking(move || {
            executor.execute(&operation, request.params, &caller)
        })
        .await;
        match result {
            Ok(Ok(data)) => ResponseEnvelope::success(id.clone(), data)
                .unwrap_or_else(|error| ResponseEnvelope::failure(id, error)),
            Ok(Err(error)) => ResponseEnvelope::failure(id, error),
            Err(error) => ResponseEnvelope::failure(
                id,
                ProtocolError::new(ErrorCode::InternalError, "operation task failed")
                    .with_detail(error.to_string()),
            ),
        }
    }
}

pub async fn serve(socket_path: &Path, mut shutdown: watch::Receiver<bool>) -> std::io::Result<()> {
    let app = Arc::new(App::new(socket_path));
    serve_with_app(socket_path, &mut shutdown, app).await
}

pub async fn serve_with_app(
    socket_path: &Path,
    shutdown: &mut watch::Receiver<bool>,
    app: Arc<App>,
) -> std::io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    match tokio::fs::symlink_metadata(socket_path).await {
        Ok(metadata) if metadata.file_type().is_socket() => {
            tokio::fs::remove_file(socket_path).await?
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("refusing to replace non-socket {}", socket_path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(socket_path)?;
    set_socket_mode(socket_path)?;
    info!(socket = %socket_path.display(), "serving protocol 2");
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let app = Arc::clone(&app);
                connections.spawn(async move {
                    if let Err(error) = serve_connection(stream, app).await {
                        error!(%error, "connection failed");
                    }
                });
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = completed {
                    error!(%error, "connection task failed");
                }
            }
        }
    }
    drop(listener);
    let _ = tokio::fs::remove_file(socket_path).await;
    while let Some(completed) = connections.join_next().await {
        if let Err(error) = completed {
            error!(%error, "connection task failed during shutdown");
        }
    }
    Ok(())
}

async fn serve_connection(mut stream: UnixStream, app: Arc<App>) -> std::io::Result<()> {
    let credentials = stream.peer_cred()?;
    let peer = PeerCredentials {
        pid: credentials
            .pid()
            .and_then(|pid| u32::try_from(pid).ok())
            .unwrap_or(0),
        uid: credentials.uid(),
        gid: credentials.gid(),
    };
    let raw = timeout(READ_DEADLINE, read_frame(&mut stream))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "request read timed out")
        })??;
    let id = serde_json::from_slice::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| {
            value
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let response = if !raw.ends_with(b"\n") {
        ResponseEnvelope::failure(
            id,
            ProtocolError::new(
                ErrorCode::ProtocolInvalid,
                "request must be newline terminated",
            ),
        )
    } else {
        match parse_request(&raw) {
            Ok(request) => app.dispatch(request, peer).await,
            Err(error) => ResponseEnvelope::failure(id, error),
        }
    };
    timeout(
        WRITE_DEADLINE,
        stream.write_all(&encode_response(&response)),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "response write timed out"))??;
    timeout(WRITE_DEADLINE, stream.shutdown())
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "response shutdown timed out")
        })?
}

async fn read_frame(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    while raw.len() <= MAX_REQUEST_BYTES {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        if raw.ends_with(b"\n") {
            break;
        }
    }
    Ok(raw)
}

#[cfg(unix)]
fn set_socket_mode(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
}

#[cfg(test)]
mod tests {
    use super::*;
    use devcoordinator2_api::{ClientContext, ResponseEnvelope};
    use tempfile::tempdir;

    #[tokio::test]
    async fn ping_crosses_real_unix_socket() {
        let temporary = tempdir().expect("tempdir");
        let socket = temporary.path().join("daemon.sock");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_socket = socket.clone();
        let server = tokio::spawn(async move { serve(&server_socket, shutdown_rx).await });
        while !socket.exists() {
            assert!(
                !server.is_finished(),
                "server failed before binding its socket"
            );
            tokio::task::yield_now().await;
        }
        let response = crate::client::call(
            &socket,
            "ping",
            serde_json::json!({}),
            ClientContext::default(),
        )
        .await
        .expect("ping response");
        assert!(matches!(response, ResponseEnvelope::Success { .. }));
        let spoofed = crate::client::call(
            &socket,
            "ping",
            serde_json::json!({}),
            ClientContext {
                identity: Some("spoof@example.test".to_owned()),
                ..ClientContext::default()
            },
        )
        .await
        .expect("bounded denial");
        assert!(matches!(
            spoofed,
            ResponseEnvelope::Failure { error, .. }
                if error.code == ErrorCode::PermissionDenied
        ));
        shutdown_tx.send(true).expect("shutdown");
        server.await.expect("server task").expect("server result");
    }
}
