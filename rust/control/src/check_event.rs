//! Exact inherited-descriptor event emission from governed check processes.

use std::os::fd::{BorrowedFd, RawFd};

use serde::Serialize;
use thiserror::Error;

use crate::cli::TestEventStatus;

#[derive(Debug, Error)]
pub enum CheckEventError {
    #[error("missing or invalid {0}")]
    Environment(&'static str),
    #[error("cannot encode governed-check event: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("cannot write governed-check event: {0}")]
    Write(#[from] std::io::Error),
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CheckEvent<'a> {
    schema: u8,
    run_id: &'a str,
    check: &'a str,
    status: &'a str,
}

pub fn emit_from_environment(status: TestEventStatus) -> Result<(), CheckEventError> {
    let descriptor = std::env::var("DEVCOORDINATOR_EVENT_FD")
        .ok()
        .and_then(|value| value.parse::<RawFd>().ok())
        .filter(|descriptor| *descriptor >= 0)
        .ok_or(CheckEventError::Environment("DEVCOORDINATOR_EVENT_FD"))?;
    let run_id = std::env::var("DEVCOORDINATOR_RUN_ID")
        .map_err(|_| CheckEventError::Environment("DEVCOORDINATOR_RUN_ID"))?;
    let check = std::env::var("DEVCOORDINATOR_CHECK_NAME")
        .map_err(|_| CheckEventError::Environment("DEVCOORDINATOR_CHECK_NAME"))?;
    emit_to_fd(descriptor, &run_id, &check, status)
}

pub fn emit_to_fd(
    descriptor: RawFd,
    run_id: &str,
    check: &str,
    status: TestEventStatus,
) -> Result<(), CheckEventError> {
    if descriptor < 0 {
        return Err(CheckEventError::Environment("DEVCOORDINATOR_EVENT_FD"));
    }
    if !valid_identifier(run_id, 128) {
        return Err(CheckEventError::Environment("DEVCOORDINATOR_RUN_ID"));
    }
    if !valid_check(check) {
        return Err(CheckEventError::Environment("DEVCOORDINATOR_CHECK_NAME"));
    }
    let mut payload = serde_json::to_vec(&CheckEvent {
        schema: 2,
        run_id,
        check,
        status: status.as_str(),
    })?;
    payload.push(b'\n');
    // SAFETY: the descriptor is inherited from the executor and borrowed only
    // for these writes; this function deliberately does not close it.
    let descriptor = unsafe { BorrowedFd::borrow_raw(descriptor) };
    let mut remaining = payload.as_slice();
    while !remaining.is_empty() {
        match rustix::io::write(descriptor, remaining) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "event descriptor accepted no bytes",
                )
                .into());
            }
            Ok(written) => remaining = &remaining[written..],
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
    }
    Ok(())
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && b"._-".contains(&byte))
        })
}

fn valid_check(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::AsRawFd;

    #[test]
    fn writes_one_schema_two_line_without_closing_the_inherited_descriptor() {
        let (reader, writer) = rustix::pipe::pipe().expect("pipe");
        emit_to_fd(
            writer.as_raw_fd(),
            "t20260903T120000Z-abcd",
            "unit-tests",
            TestEventStatus::Passed,
        )
        .expect("event");
        drop(writer);
        let mut reader = std::fs::File::from(reader);
        let mut payload = String::new();
        reader.read_to_string(&mut payload).expect("read");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&payload).expect("JSON"),
            serde_json::json!({
                "schema":2,
                "run_id":"t20260903T120000Z-abcd",
                "check":"unit-tests",
                "status":"passed"
            })
        );
    }

    #[test]
    fn invalid_identity_and_closed_channel_fail_truthfully() {
        assert!(emit_to_fd(-1, "run", "check", TestEventStatus::Failed).is_err());
        assert!(emit_to_fd(1, "../run", "check", TestEventStatus::Failed).is_err());
        assert!(emit_to_fd(1, "run", "Bad Check", TestEventStatus::Unsafe).is_err());
        let (reader, writer) = rustix::pipe::pipe().expect("pipe");
        drop(reader);
        assert!(emit_to_fd(writer.as_raw_fd(), "run", "check", TestEventStatus::Failed).is_err());
    }
}
