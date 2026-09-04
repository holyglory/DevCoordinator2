use std::path::Path;
use std::time::Duration;

use devcoordinator2_api::{
    ClientContext, ErrorCode, MAX_RESPONSE_BYTES, PROTOCOL_VERSION, ProtocolError, RequestEnvelope,
    ResponseEnvelope,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

pub async fn call(
    socket_path: &Path,
    operation: impl Into<String>,
    params: Value,
    client: ClientContext,
) -> Result<ResponseEnvelope, ProtocolError> {
    let mut stream = timeout(Duration::from_secs(5), UnixStream::connect(socket_path))
        .await
        .map_err(|_| {
            ProtocolError::new(ErrorCode::DaemonUnavailable, "daemon connection timed out")
        })?
        .map_err(|error| {
            ProtocolError::new(
                ErrorCode::DaemonUnavailable,
                format!("cannot reach daemon at {}", socket_path.display()),
            )
            .with_detail(error.to_string())
        })?;
    let request = RequestEnvelope {
        protocol: PROTOCOL_VERSION,
        id: request_id(),
        operation: operation.into(),
        params,
        client,
    };
    let mut encoded = serde_json::to_vec(&request).map_err(|error| {
        ProtocolError::new(ErrorCode::ProtocolInvalid, "cannot encode request")
            .with_detail(error.to_string())
    })?;
    encoded.push(b'\n');
    timeout(Duration::from_secs(5), stream.write_all(&encoded))
        .await
        .map_err(|_| {
            ProtocolError::new(
                ErrorCode::DaemonUnavailable,
                "daemon request write timed out",
            )
        })?
        .map_err(transport_error)?;
    timeout(Duration::from_secs(5), stream.shutdown())
        .await
        .map_err(|_| {
            ProtocolError::new(
                ErrorCode::DaemonUnavailable,
                "daemon request shutdown timed out",
            )
        })?
        .map_err(transport_error)?;

    let mut response = Vec::new();
    timeout(
        Duration::from_secs(10),
        stream
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut response),
    )
    .await
    .map_err(|_| ProtocolError::new(ErrorCode::DaemonUnavailable, "daemon response timed out"))?
    .map_err(transport_error)?;
    if response.len() > MAX_RESPONSE_BYTES {
        return Err(ProtocolError::new(
            ErrorCode::ProtocolInvalid,
            "daemon response exceeds size cap",
        ));
    }
    serde_json::from_slice(&response).map_err(|error| {
        ProtocolError::new(ErrorCode::ProtocolInvalid, "daemon response is invalid")
            .with_detail(error.to_string())
    })
}

fn transport_error(error: std::io::Error) -> ProtocolError {
    ProtocolError::new(ErrorCode::DaemonUnavailable, "daemon connection failed")
        .with_detail(error.to_string())
}

fn request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}{:x}", SEQUENCE.fetch_add(1, Ordering::Relaxed))
        .chars()
        .take(24)
        .collect()
}
