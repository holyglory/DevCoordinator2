use std::path::Path;
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

use devcoordinator2_api::{
    EmptyParams, ErrorCode, MAX_REQUEST_BYTES, PingData, ProtocolError, RequestEnvelope,
    ResponseEnvelope, encode_response, parse_request,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tracing::{error, info};

use crate::{DATABASE_SCHEMA_VERSION, SOURCE_COMMIT};

#[derive(Clone)]
pub struct App {
    socket_display: Arc<str>,
}

impl App {
    pub fn new(socket_path: &Path) -> Self {
        Self {
            socket_display: socket_path.display().to_string().into(),
        }
    }

    pub async fn dispatch(&self, request: RequestEnvelope) -> ResponseEnvelope {
        let id = request.id.clone();
        match request.operation.as_str() {
            "ping" => match serde_json::from_value::<EmptyParams>(request.params) {
                Ok(_) => match ResponseEnvelope::success(
                    id.clone(),
                    PingData {
                        daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
                        protocol_version: devcoordinator2_api::PROTOCOL_VERSION,
                        schema_version: DATABASE_SCHEMA_VERSION,
                        executor_schema: devcoordinator2_executor_protocol::EXECUTOR_SCHEMA,
                        source_commit: SOURCE_COMMIT.to_owned(),
                        socket: self.socket_display.to_string(),
                    },
                ) {
                    Ok(response) => response,
                    Err(error) => ResponseEnvelope::failure(id, error),
                },
                Err(error) => ResponseEnvelope::failure(
                    id,
                    ProtocolError::new(ErrorCode::ParamsInvalid, "ping parameters are invalid")
                        .with_detail(error.to_string()),
                ),
            },
            _ => ResponseEnvelope::failure(
                id,
                ProtocolError::new(ErrorCode::OperationUnknown, "unknown operation"),
            ),
        }
    }
}

pub async fn serve(socket_path: &Path, mut shutdown: watch::Receiver<bool>) -> std::io::Result<()> {
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
    let app = Arc::new(App::new(socket_path));
    info!(socket = %socket_path.display(), "serving protocol 2");

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
                tokio::spawn(async move {
                    if let Err(error) = serve_connection(stream, app).await {
                        error!(%error, "connection failed");
                    }
                });
            }
        }
    }
    drop(listener);
    let _ = tokio::fs::remove_file(socket_path).await;
    Ok(())
}

async fn serve_connection(mut stream: UnixStream, app: Arc<App>) -> std::io::Result<()> {
    let mut raw = Vec::new();
    (&mut stream)
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .await?;
    let id = serde_json::from_slice::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| {
            value
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let response = match parse_request(&raw) {
        Ok(request) => app.dispatch(request).await,
        Err(error) => ResponseEnvelope::failure(id, error),
    };
    stream.write_all(&encode_response(&response)).await?;
    stream.shutdown().await
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
        shutdown_tx.send(true).expect("shutdown");
        server.await.expect("server task").expect("server result");
    }
}
