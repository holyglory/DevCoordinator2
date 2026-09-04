//! Explicit-argument, caller-identity helper boundary for governed tests.

use std::ffi::{CStr, OsString};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use devcoordinator2_executor_protocol::{ArtifactReceipt, MAX_REPORT_BYTES};
use serde::Deserialize;
use thiserror::Error;

const EXEC_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const OUTPUT_CAP: usize = 64 * 1024;
const ERROR_CAP: usize = 2 * 1024;
const POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Error)]
pub enum TestCommandError {
    #[error("invalid governed-test helper request: {0}")]
    Invalid(String),
    #[error("governed-test helper invocation failed: {0}")]
    Invocation(String),
    #[error("governed-test helper command failed: {0}")]
    Command(String),
}

pub trait TestCommand: Send + Sync + 'static {
    fn source_digest(
        &self,
        executor: &Path,
        worktree: &Path,
        uid: u32,
        gid: u32,
    ) -> Result<String, TestCommandError>;

    fn receipts_match(
        &self,
        executor: &Path,
        worktree: &Path,
        receipts: &[ArtifactReceipt],
        uid: u32,
        gid: u32,
    ) -> Result<bool, TestCommandError>;

    fn executable(&self, candidate: &Path, uid: u32, gid: u32) -> bool;

    fn executor_available(&self, executor: &Path) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostTestCommand;

impl TestCommand for HostTestCommand {
    fn source_digest(
        &self,
        executor: &Path,
        worktree: &Path,
        uid: u32,
        gid: u32,
    ) -> Result<String, TestCommandError> {
        validate_executor(executor)?;
        validate_worktree(worktree)?;
        let output = run_as(
            executor,
            &[
                OsString::from("source-digest"),
                OsString::from("--worktree"),
                worktree.as_os_str().to_owned(),
            ],
            worktree,
            uid,
            gid,
            None,
            Duration::from_secs(120),
        )?;
        require_success(&output, "source digest failed")?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct DigestResponse {
            schema: u8,
            sha256: String,
        }
        let response: DigestResponse = serde_json::from_slice(&output.stdout)
            .map_err(|_| TestCommandError::Command("source digest returned invalid JSON".into()))?;
        if response.schema != 2 || !valid_digest(&response.sha256) {
            return Err(TestCommandError::Command(
                "source digest returned an invalid schema-2 digest".into(),
            ));
        }
        Ok(response.sha256)
    }

    fn receipts_match(
        &self,
        executor: &Path,
        worktree: &Path,
        receipts: &[ArtifactReceipt],
        uid: u32,
        gid: u32,
    ) -> Result<bool, TestCommandError> {
        validate_executor(executor)?;
        validate_worktree(worktree)?;
        let payload = serde_json::to_vec(receipts)
            .map_err(|error| TestCommandError::Invalid(error.to_string()))?;
        if payload.len() > MAX_REPORT_BYTES {
            return Err(TestCommandError::Invalid(
                "artifact receipts exceed the executor input limit".into(),
            ));
        }
        let output = run_as(
            executor,
            &[
                OsString::from("receipts-match"),
                OsString::from("--worktree"),
                worktree.as_os_str().to_owned(),
                OsString::from("--receipts"),
                OsString::from("-"),
            ],
            worktree,
            uid,
            gid,
            Some(payload),
            Duration::from_secs(120),
        )?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct MatchResponse {
            schema: u8,
            matches: bool,
        }
        let response: MatchResponse = serde_json::from_slice(&output.stdout).map_err(|_| {
            TestCommandError::Command("receipt comparison returned invalid JSON".into())
        })?;
        if response.schema != 2 || !matches!(output.status.code(), Some(0 | 1)) {
            return Err(command_error(&output, "receipt comparison failed"));
        }
        if response.matches != output.status.success() {
            return Err(TestCommandError::Command(
                "receipt comparison exit status contradicted its result".into(),
            ));
        }
        Ok(response.matches)
    }

    fn executable(&self, candidate: &Path, uid: u32, gid: u32) -> bool {
        let test = Path::new("/usr/bin/test");
        if !candidate.is_absolute() || !test.is_file() {
            return false;
        }
        run_as(
            test,
            &[OsString::from("-x"), candidate.as_os_str().to_owned()],
            Path::new("/"),
            uid,
            gid,
            None,
            Duration::from_secs(5),
        )
        .is_ok_and(|output| output.status.success())
    }

    fn executor_available(&self, executor: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(executor).is_ok_and(|metadata| {
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.permissions().mode() & 0o111 != 0
        })
    }
}

struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

#[allow(clippy::too_many_arguments)]
fn run_as(
    program: &Path,
    arguments: &[OsString],
    cwd: &Path,
    uid: u32,
    gid: u32,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<Output, TestCommandError> {
    if !program.is_absolute() || !cwd.is_absolute() || timeout.is_zero() {
        return Err(TestCommandError::Invalid(
            "helper program, cwd, and timeout must be explicit".into(),
        ));
    }
    let mut command = if rustix::process::geteuid().as_raw() == 0 && uid != 0 {
        let home = passwd_home(uid).map_err(invocation_error)?;
        let setpriv = ["/usr/bin/setpriv", "/bin/setpriv"]
            .into_iter()
            .map(Path::new)
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| {
                TestCommandError::Invocation("setpriv is unavailable in /usr/bin:/bin".into())
            })?;
        let mut command = Command::new(setpriv);
        command
            .arg(format!("--reuid={uid}"))
            .arg(format!("--regid={gid}"))
            .arg("--init-groups")
            .arg("--")
            .arg(program)
            .env_clear()
            .env("PATH", EXEC_PATH)
            .env("HOME", home);
        command
    } else {
        let mut command = Command::new(program);
        command.env_clear().env("PATH", EXEC_PATH).env(
            "HOME",
            std::env::var_os("HOME").unwrap_or_else(|| OsString::from("/nonexistent")),
        );
        command
    };
    command
        .args(arguments)
        .current_dir(cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(invocation_error)?;
    let input = child.stdin.take();
    let writer = stdin.zip(input).map(|(payload, mut input)| {
        thread::spawn(move || -> io::Result<()> {
            input.write_all(&payload)?;
            input.flush()
        })
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| TestCommandError::Invocation("stdout pipe is unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| TestCommandError::Invocation("stderr pipe is unavailable".into()))?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(invocation_error)? {
            break status;
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TestCommandError::Invocation(
                "governed-test helper timed out".into(),
            ));
        }
        thread::sleep(POLL.min(deadline.saturating_duration_since(now)));
    };
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| TestCommandError::Invocation("stdin writer failed".into()))?
            .map_err(invocation_error)?;
    }
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| TestCommandError::Invocation("stdout reader failed".into()))?
        .map_err(invocation_error)?;
    let (stderr, stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| TestCommandError::Invocation("stderr reader failed".into()))?
        .map_err(invocation_error)?;
    Ok(Output {
        status,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

fn read_bounded(mut source: impl Read) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = OUTPUT_CAP.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        truncated |= count > remaining;
    }
    Ok((bytes, truncated))
}

fn require_success(output: &Output, fallback: &str) -> Result<(), TestCommandError> {
    if output.status.success() && !output.stdout_truncated && !output.stderr_truncated {
        Ok(())
    } else {
        Err(command_error(output, fallback))
    }
}

fn command_error(output: &Output, fallback: &str) -> TestCommandError {
    if output.stderr_truncated {
        return TestCommandError::Command(fallback.into());
    }
    let detail = bounded_tail(&String::from_utf8_lossy(&output.stderr), ERROR_CAP);
    TestCommandError::Command(if detail.is_empty() {
        fallback.into()
    } else {
        detail
    })
}

fn validate_executor(path: &Path) -> Result<(), TestCommandError> {
    if !path.is_absolute() {
        return Err(TestCommandError::Invalid(
            "executor path must be absolute".into(),
        ));
    }
    Ok(())
}

fn validate_worktree(path: &Path) -> Result<(), TestCommandError> {
    if !path.is_absolute() || !path.is_dir() {
        return Err(TestCommandError::Invalid(
            "worktree must be an existing absolute directory".into(),
        ));
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn bounded_tail(value: &str, limit: usize) -> String {
    let value = value.trim();
    if value.len() <= limit {
        return value.into();
    }
    let mut start = value.len() - limit;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].into()
}

fn passwd_home(uid: u32) -> io::Result<OsString> {
    let mut buffer = vec![0_u8; 16 * 1024];
    // SAFETY: A zeroed passwd value is a valid getpwuid_r output buffer.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    // SAFETY: Every pointer references writable storage for the stated length.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status));
    }
    if result.is_null() || entry.pw_dir.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("unknown caller uid {uid}"),
        ));
    }
    // SAFETY: getpwuid_r returned a NUL-terminated pointer backed by `buffer`.
    Ok(OsString::from_vec(
        unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes().to_vec(),
    ))
}

fn invocation_error(error: io::Error) -> TestCommandError {
    TestCommandError::Invocation(bounded_tail(&error.to_string(), ERROR_CAP))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_probe_respects_the_physical_caller() {
        let host = HostTestCommand;
        assert!(host.executable(
            Path::new("/bin/sh"),
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw(),
        ));
        assert!(!host.executable(
            Path::new("/definitely/missing"),
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw(),
        ));
    }

    #[test]
    fn helper_rejects_relative_executor_and_worktree_paths() {
        let error = HostTestCommand
            .source_digest(Path::new("executor"), Path::new("."), 1, 1)
            .unwrap_err();
        assert!(matches!(error, TestCommandError::Invalid(_)));
    }
}
