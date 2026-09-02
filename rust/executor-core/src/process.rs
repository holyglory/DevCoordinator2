use std::collections::BTreeMap;
use std::future::{Future, pending};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use devcoordinator2_executor_protocol::{
    CompletionEvent, CompletionMode, EventStatus, LeafStatus, MAX_EVENT_BYTES, MAX_MANIFEST_BYTES,
    OutputStats,
};
use rustix::fd::AsFd;
use rustix::io::{FdFlags, fcntl_setfd};
use rustix::pipe::{PipeFlags, pipe_with};
use rustix::process::test_kill_process_group;
use rustix::process::{Pid, Signal, kill_process_group};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

use crate::ExecutorError;
use crate::capacity::{AcquiredPermit, PermitProvider, PermitRequest};

pub(crate) const CHECK_LOG_CAP_BYTES: usize = 4 * 1024 * 1024;
const EVENT_FD: i32 = 198;
const MANIFEST_FD: i32 = 199;
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const GROUP_OBSERVATION_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub struct Cancellation {
    sender: watch::Sender<bool>,
}

impl Default for Cancellation {
    fn default() -> Self {
        let (sender, _) = watch::channel(false);
        Self { sender }
    }
}

impl Cancellation {
    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

pub(crate) struct ProcessRequest {
    pub run_id: String,
    pub check_name: String,
    pub leaf_id: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub scratch: PathBuf,
    pub shared_artifacts: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub aggregate_stdout: Arc<AggregateSink>,
    pub aggregate_stderr: Arc<AggregateSink>,
    pub stdout_cap: usize,
    pub stderr_cap: usize,
    pub timeout_seconds: Option<u64>,
    pub completion: CompletionMode,
    pub capture_manifest: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessStatus {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    Unsafe,
}

impl From<ProcessStatus> for LeafStatus {
    fn from(value: ProcessStatus) -> Self {
        match value {
            ProcessStatus::Passed => Self::Passed,
            ProcessStatus::Failed => Self::Failed,
            ProcessStatus::TimedOut => Self::TimedOut,
            ProcessStatus::Cancelled => Self::Cancelled,
            ProcessStatus::Unsafe => Self::Unsafe,
        }
    }
}

pub(crate) struct ProcessResult {
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub output: OutputStats,
    pub service: Option<EventService>,
    pub manifest: Option<ManifestCapture>,
    pub capacity: crate::capacity::CapacityObservation,
}

pub(crate) struct ManifestCapture {
    pub payload: Vec<u8>,
    pub observed: u64,
    pub truncated: bool,
}

pub(crate) struct EventService {
    pub pgid: i32,
    pub future: Pin<Box<dyn Future<Output = ServiceExit> + Send>>,
}

pub(crate) struct ServiceExit {
    pub exit_code: Option<i32>,
    pub output: Result<OutputStats, ExecutorError>,
}

struct SpawnedProcess {
    child: Child,
    pgid: i32,
    stdout: JoinHandle<Result<StreamStats, ExecutorError>>,
    stderr: JoinHandle<Result<StreamStats, ExecutorError>>,
    event_reader: Option<File>,
    manifest: Option<JoinHandle<Result<ManifestCapture, ExecutorError>>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct StreamStats {
    observed: u64,
    retained: u64,
    truncated: bool,
}

pub(crate) struct AggregateSink {
    file: Mutex<File>,
    retained: AtomicU64,
    cap: u64,
}

impl AggregateSink {
    pub(crate) fn create(path: &Path, cap: usize) -> Result<Arc<Self>, ExecutorError> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| ExecutorError::new(format!("cannot open aggregate log: {error}")))?;
        Ok(Arc::new(Self {
            file: Mutex::new(File::from_std(file)),
            retained: AtomicU64::new(0),
            cap: u64::try_from(cap).unwrap_or(u64::MAX),
        }))
    }

    async fn append(&self, block: &[u8]) -> Result<(), ExecutorError> {
        let reserved = loop {
            let retained = self.retained.load(Ordering::Acquire);
            let remaining = self.cap.saturating_sub(retained);
            if remaining == 0 {
                return Ok(());
            }
            let reserved = remaining.min(u64::try_from(block.len()).unwrap_or(u64::MAX));
            if self
                .retained
                .compare_exchange_weak(
                    retained,
                    retained.saturating_add(reserved),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                break usize::try_from(reserved).unwrap_or(block.len());
            }
        };
        self.file
            .lock()
            .await
            .write_all(&block[..reserved])
            .await
            .map_err(|error| ExecutorError::new(format!("cannot write aggregate log: {error}")))
    }
}

pub(crate) async fn run_process(
    request: ProcessRequest,
    permits: Arc<dyn PermitProvider>,
    cancellation: Cancellation,
) -> ProcessResult {
    let acquire = permits.acquire(PermitRequest {
        run_id: request.run_id.clone(),
        leaf_id: request.leaf_id.clone(),
    });
    let permit = tokio::select! {
        result = acquire => match result {
            Ok(permit) => permit,
            Err(error) => return failure(ProcessStatus::Unsafe, None, error.to_string()),
        },
        () = cancellation.cancelled() => {
            return failure(ProcessStatus::Cancelled, None, "run cancelled before capacity grant");
        }
    };
    let capacity = permit.observation;
    let spawned = match spawn_process(&request).await {
        Ok(spawned) => spawned,
        Err(error) => {
            return ProcessResult {
                capacity,
                ..failure(
                    ProcessStatus::Failed,
                    None,
                    format!("cannot start leaf: {error}"),
                )
            };
        }
    };
    match request.completion {
        CompletionMode::Process => {
            run_to_exit(
                spawned,
                permit,
                request.timeout_seconds,
                cancellation,
                capacity,
            )
            .await
        }
        CompletionMode::Event => {
            run_to_event(
                spawned,
                permit,
                request.timeout_seconds,
                cancellation,
                &request.run_id,
                &request.check_name,
                capacity,
            )
            .await
        }
    }
}

async fn spawn_process(request: &ProcessRequest) -> Result<SpawnedProcess, ExecutorError> {
    if request.command.is_empty() {
        return Err(ExecutorError::new("leaf command is empty"));
    }
    tokio::fs::create_dir_all(&request.scratch)
        .await
        .map_err(|error| {
            ExecutorError::new(format!("cannot create leaf scratch directory: {error}"))
        })?;
    if let Some(parent) = request.stdout_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ExecutorError::new(format!("cannot create leaf log directory: {error}"))
        })?;
    }
    let mut command = Command::new(&request.command[0]);
    command
        .args(&request.command[1..])
        .current_dir(&request.cwd)
        .envs(&request.env)
        .env("DEVCOORDINATOR_RUN_ID", &request.run_id)
        .env("DEVCOORDINATOR_CHECK_NAME", &request.check_name)
        .env("DEVCOORDINATOR_CHECK_SCRATCH", &request.scratch)
        .env("DEVCOORDINATOR_SHARED_ARTIFACTS", &request.shared_artifacts)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(false);

    let mut event_read = None;
    let mut event_write = None;
    if request.completion == CompletionMode::Event {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC)
            .map_err(|error| ExecutorError::new(format!("cannot create event pipe: {error}")))?;
        let write_raw = write.as_raw_fd();
        command.env("DEVCOORDINATOR_EVENT_FD", EVENT_FD.to_string());
        // SAFETY: pre_exec invokes only async-signal-safe descriptor operations.
        // The target descriptor is deliberately leaked into the child and is
        // closed by exec or process termination.
        unsafe {
            command.pre_exec(move || {
                let source = BorrowedFd::borrow_raw(write_raw);
                let mut target = OwnedFd::from_raw_fd(EVENT_FD);
                if write_raw != EVENT_FD {
                    rustix::io::dup2(source, &mut target)?;
                }
                fcntl_setfd(target.as_fd(), FdFlags::empty())?;
                std::mem::forget(target);
                Ok(())
            });
        }
        event_read = Some(read);
        event_write = Some(write);
    }
    let mut manifest_read = None;
    let mut manifest_write = None;
    if request.capture_manifest {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC)
            .map_err(|error| ExecutorError::new(format!("cannot create manifest pipe: {error}")))?;
        let write_raw = write.as_raw_fd();
        command.env("DEVCOORDINATOR_CASE_MANIFEST_FD", MANIFEST_FD.to_string());
        // SAFETY: see the event descriptor setup above. This is a distinct
        // inherited descriptor and uses only async-signal-safe operations.
        unsafe {
            command.pre_exec(move || {
                let source = BorrowedFd::borrow_raw(write_raw);
                let mut target = OwnedFd::from_raw_fd(MANIFEST_FD);
                if write_raw != MANIFEST_FD {
                    rustix::io::dup2(source, &mut target)?;
                }
                fcntl_setfd(target.as_fd(), FdFlags::empty())?;
                std::mem::forget(target);
                Ok(())
            });
        }
        manifest_read = Some(read);
        manifest_write = Some(write);
    }

    let mut child = command
        .spawn()
        .map_err(|error| ExecutorError::new(error.to_string()))?;
    drop(event_write);
    drop(manifest_write);
    let pgid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .ok_or_else(|| ExecutorError::new("spawned leaf has no valid process id"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExecutorError::new("spawned leaf has no stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ExecutorError::new("spawned leaf has no stderr pipe"))?;
    let stdout_path = request.stdout_path.clone();
    let stderr_path = request.stderr_path.clone();
    let stdout_cap = request.stdout_cap;
    let stderr_cap = request.stderr_cap;
    let aggregate_stdout = request.aggregate_stdout.clone();
    let aggregate_stderr = request.aggregate_stderr.clone();
    let stdout =
        tokio::spawn(async move { pump(stdout, &stdout_path, stdout_cap, aggregate_stdout).await });
    let stderr =
        tokio::spawn(async move { pump(stderr, &stderr_path, stderr_cap, aggregate_stderr).await });
    let event_reader = event_read.map(|fd| File::from_std(std::fs::File::from(fd)));
    let manifest = manifest_read.map(|fd| {
        tokio::spawn(async move { capture_manifest(File::from_std(std::fs::File::from(fd))).await })
    });
    Ok(SpawnedProcess {
        child,
        pgid,
        stdout,
        stderr,
        event_reader,
        manifest,
    })
}

async fn run_to_exit(
    mut process: SpawnedProcess,
    _permit: AcquiredPermit,
    timeout_seconds: Option<u64>,
    cancellation: Cancellation,
    capacity: crate::capacity::CapacityObservation,
) -> ProcessResult {
    let deadline = leaf_deadline(timeout_seconds);
    tokio::pin!(deadline);
    let outcome = tokio::select! {
        status = process.child.wait() => match status {
            Ok(status) => {
                let code = exit_code(status);
                if status.success() {
                    (ProcessStatus::Passed, code, None)
                } else {
                    (ProcessStatus::Failed, code, Some(format!("process exited {}", display_exit(status))))
                }
            }
            Err(error) => (ProcessStatus::Unsafe, None, Some(format!("cannot wait for leaf: {error}"))),
        },
        () = &mut deadline => {
            let detail = terminate(&mut process.child, process.pgid).await.err().map(|error| error.to_string());
            (ProcessStatus::TimedOut, process.child.try_wait().ok().flatten().and_then(exit_code),
             Some(detail.unwrap_or_else(|| deadline_reason(timeout_seconds, "leaf"))))
        },
        () = cancellation.cancelled() => {
            let detail = terminate(&mut process.child, process.pgid).await.err().map(|error| error.to_string());
            (ProcessStatus::Cancelled, process.child.try_wait().ok().flatten().and_then(exit_code),
             Some(detail.unwrap_or_else(|| "run cancelled".into())))
        },
    };
    let cleanup_error = cleanup_process_group(process.pgid).await.err();
    let output = finish_pumps(process.stdout, process.stderr).await;
    let manifest = finish_manifest(process.manifest).await;
    match (output, manifest, cleanup_error) {
        (Ok(output), Ok(manifest), None) => ProcessResult {
            status: outcome.0,
            exit_code: outcome.1,
            reason: outcome.2,
            output,
            service: None,
            manifest,
            capacity,
        },
        (output, manifest, cleanup_error) => {
            let detail = cleanup_error
                .map(|error| error.to_string())
                .or_else(|| output.err().map(|error| error.to_string()))
                .or_else(|| manifest.err().map(|error| error.to_string()))
                .unwrap_or_else(|| "leaf cleanup failed".into());
            ProcessResult {
                capacity,
                ..failure(ProcessStatus::Unsafe, outcome.1, detail)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_to_event(
    mut process: SpawnedProcess,
    permit: AcquiredPermit,
    timeout_seconds: Option<u64>,
    cancellation: Cancellation,
    run_id: &str,
    check_name: &str,
    capacity: crate::capacity::CapacityObservation,
) -> ProcessResult {
    let Some(mut event_reader) = process.event_reader.take() else {
        return ProcessResult {
            capacity,
            ..failure(
                ProcessStatus::Unsafe,
                None,
                "event leaf has no event descriptor",
            )
        };
    };
    let deadline = leaf_deadline(timeout_seconds);
    tokio::pin!(deadline);
    enum First {
        Event(Result<CompletionEvent, ExecutorError>),
        Exit(Result<std::process::ExitStatus, std::io::Error>),
        Timeout,
        Cancel,
    }
    let first = tokio::select! {
        event = read_event(&mut event_reader) => First::Event(event),
        status = process.child.wait() => First::Exit(status),
        () = &mut deadline => First::Timeout,
        () = cancellation.cancelled() => First::Cancel,
    };
    match first {
        First::Exit(status) => {
            let (code, reason) = match status {
                Ok(status) => (
                    exit_code(status),
                    format!(
                        "process exited {} before its completion event",
                        display_exit(status)
                    ),
                ),
                Err(error) => (None, format!("cannot wait for event leaf: {error}")),
            };
            let _ = cleanup_process_group(process.pgid).await;
            let output = finish_pumps(process.stdout, process.stderr)
                .await
                .unwrap_or_default();
            ProcessResult {
                status: ProcessStatus::Failed,
                exit_code: code,
                reason: Some(reason),
                output,
                service: None,
                manifest: None,
                capacity,
            }
        }
        First::Timeout => {
            let _ = terminate(&mut process.child, process.pgid).await;
            let output = finish_pumps(process.stdout, process.stderr)
                .await
                .unwrap_or_default();
            ProcessResult {
                status: ProcessStatus::TimedOut,
                exit_code: process.child.try_wait().ok().flatten().and_then(exit_code),
                reason: Some(deadline_reason(timeout_seconds, "event leaf")),
                output,
                service: None,
                manifest: None,
                capacity,
            }
        }
        First::Cancel => {
            let _ = terminate(&mut process.child, process.pgid).await;
            let output = finish_pumps(process.stdout, process.stderr)
                .await
                .unwrap_or_default();
            ProcessResult {
                status: ProcessStatus::Cancelled,
                exit_code: process.child.try_wait().ok().flatten().and_then(exit_code),
                reason: Some("run cancelled".into()),
                output,
                service: None,
                manifest: None,
                capacity,
            }
        }
        First::Event(Err(error)) => {
            let _ = terminate(&mut process.child, process.pgid).await;
            let output = finish_pumps(process.stdout, process.stderr)
                .await
                .unwrap_or_default();
            ProcessResult {
                status: ProcessStatus::Unsafe,
                exit_code: process.child.try_wait().ok().flatten().and_then(exit_code),
                reason: Some(error.to_string()),
                output,
                service: None,
                manifest: None,
                capacity,
            }
        }
        First::Event(Ok(event)) => {
            if event.run_id != run_id || event.check != check_name {
                let _ = terminate(&mut process.child, process.pgid).await;
                let output = finish_pumps(process.stdout, process.stderr)
                    .await
                    .unwrap_or_default();
                return ProcessResult {
                    status: ProcessStatus::Unsafe,
                    exit_code: process.child.try_wait().ok().flatten().and_then(exit_code),
                    reason: Some("completion event carried the wrong run or check identity".into()),
                    output,
                    service: None,
                    manifest: None,
                    capacity,
                };
            }
            if event.status != EventStatus::Passed {
                let _ = terminate(&mut process.child, process.pgid).await;
                let output = finish_pumps(process.stdout, process.stderr)
                    .await
                    .unwrap_or_default();
                return ProcessResult {
                    status: if event.status == EventStatus::Unsafe {
                        ProcessStatus::Unsafe
                    } else {
                        ProcessStatus::Failed
                    },
                    exit_code: process.child.try_wait().ok().flatten().and_then(exit_code),
                    reason: Some(
                        event
                            .reason
                            .unwrap_or_else(|| "completion event reported failure".into()),
                    ),
                    output,
                    service: None,
                    manifest: None,
                    capacity,
                };
            }
            match process.child.try_wait() {
                Ok(Some(status)) => {
                    let _ = cleanup_process_group(process.pgid).await;
                    let output = finish_pumps(process.stdout, process.stderr)
                        .await
                        .unwrap_or_default();
                    ProcessResult {
                        status: if status.success() {
                            ProcessStatus::Passed
                        } else {
                            ProcessStatus::Failed
                        },
                        exit_code: exit_code(status),
                        reason: (!status.success())
                            .then(|| format!("event process exited {}", display_exit(status))),
                        output,
                        service: None,
                        manifest: None,
                        capacity,
                    }
                }
                Ok(None) => {
                    let pgid = process.pgid;
                    let future = Box::pin(async move {
                        let _permit = permit;
                        let status = process.child.wait().await;
                        ServiceExit {
                            exit_code: status.ok().and_then(exit_code),
                            output: finish_pumps(process.stdout, process.stderr).await,
                        }
                    });
                    ProcessResult {
                        status: ProcessStatus::Passed,
                        exit_code: None,
                        reason: None,
                        output: OutputStats::default(),
                        service: Some(EventService { pgid, future }),
                        manifest: None,
                        capacity,
                    }
                }
                Err(error) => {
                    let _ = terminate(&mut process.child, process.pgid).await;
                    let output = finish_pumps(process.stdout, process.stderr)
                        .await
                        .unwrap_or_default();
                    ProcessResult {
                        status: ProcessStatus::Unsafe,
                        exit_code: None,
                        reason: Some(format!("cannot inspect event process: {error}")),
                        output,
                        service: None,
                        manifest: None,
                        capacity,
                    }
                }
            }
        }
    }
}

pub(crate) fn signal_group(pgid: i32, signal: Signal) -> Result<(), ExecutorError> {
    let pid =
        Pid::from_raw(pgid).ok_or_else(|| ExecutorError::new("process group id is invalid"))?;
    match kill_process_group(pid, signal) {
        Ok(()) => Ok(()),
        Err(error) if error == rustix::io::Errno::SRCH => Ok(()),
        Err(error) => Err(ExecutorError::new(format!(
            "cannot signal process group {pgid}: {error}"
        ))),
    }
}

async fn terminate(child: &mut Child, pgid: i32) -> Result<(), ExecutorError> {
    if child.try_wait()?.is_some() {
        return cleanup_process_group(pgid).await;
    }
    signal_group(pgid, Signal::TERM)?;
    if timeout(TERMINATION_GRACE, child.wait()).await.is_err() {
        signal_group(pgid, Signal::KILL)?;
        child.wait().await?;
    }
    cleanup_process_group(pgid).await
}

async fn cleanup_process_group(pgid: i32) -> Result<(), ExecutorError> {
    if !process_group_exists(pgid)? {
        return Ok(());
    }
    signal_group(pgid, Signal::TERM)?;
    let deadline = Instant::now() + TERMINATION_GRACE;
    while process_group_exists(pgid)? && Instant::now() < deadline {
        tokio::time::sleep(GROUP_OBSERVATION_INTERVAL).await;
    }
    if process_group_exists(pgid)? {
        signal_group(pgid, Signal::KILL)?;
    }
    Ok(())
}

fn process_group_exists(pgid: i32) -> Result<bool, ExecutorError> {
    let pid =
        Pid::from_raw(pgid).ok_or_else(|| ExecutorError::new("process group id is invalid"))?;
    match test_kill_process_group(pid) {
        Ok(()) => Ok(true),
        Err(error) if error == rustix::io::Errno::SRCH => Ok(false),
        Err(error) => Err(ExecutorError::new(format!(
            "cannot inspect process group {pgid}: {error}"
        ))),
    }
}

async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    path: &Path,
    cap: usize,
    aggregate: Arc<AggregateSink>,
) -> Result<StreamStats, ExecutorError> {
    let standard = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| ExecutorError::new(format!("cannot open leaf log: {error}")))?;
    let mut output = File::from_std(standard);
    let mut stats = StreamStats::default();
    let mut block = vec![0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut block)
            .await
            .map_err(|error| ExecutorError::new(format!("cannot read leaf output: {error}")))?;
        if read == 0 {
            break;
        }
        stats.observed = stats.observed.saturating_add(read as u64);
        aggregate.append(&block[..read]).await?;
        let remaining = cap.saturating_sub(stats.retained as usize);
        if remaining > 0 {
            let keep = read.min(remaining);
            output.write_all(&block[..keep]).await.map_err(|error| {
                ExecutorError::new(format!("cannot retain leaf output: {error}"))
            })?;
            stats.retained = stats.retained.saturating_add(keep as u64);
        }
    }
    output
        .sync_all()
        .await
        .map_err(|error| ExecutorError::new(format!("cannot sync leaf output: {error}")))?;
    stats.truncated = stats.observed > stats.retained;
    Ok(stats)
}

async fn finish_pumps(
    stdout: JoinHandle<Result<StreamStats, ExecutorError>>,
    stderr: JoinHandle<Result<StreamStats, ExecutorError>>,
) -> Result<OutputStats, ExecutorError> {
    let stdout = stdout
        .await
        .map_err(|error| ExecutorError::new(format!("stdout task failed: {error}")))??;
    let stderr = stderr
        .await
        .map_err(|error| ExecutorError::new(format!("stderr task failed: {error}")))??;
    Ok(OutputStats {
        stdout_bytes_observed: stdout.observed,
        stdout_bytes_retained: stdout.retained,
        stdout_truncated: stdout.truncated,
        stderr_bytes_observed: stderr.observed,
        stderr_bytes_retained: stderr.retained,
        stderr_truncated: stderr.truncated,
    })
}

async fn capture_manifest(mut reader: File) -> Result<ManifestCapture, ExecutorError> {
    let mut payload = Vec::new();
    let mut observed = 0_u64;
    let mut block = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut block).await.map_err(|error| {
            ExecutorError::new(format!("cannot read case manifest descriptor: {error}"))
        })?;
        if read == 0 {
            break;
        }
        observed = observed.saturating_add(read as u64);
        let remaining = (MAX_MANIFEST_BYTES + 1).saturating_sub(payload.len());
        if remaining > 0 {
            payload.extend_from_slice(&block[..read.min(remaining)]);
        }
    }
    Ok(ManifestCapture {
        truncated: observed > MAX_MANIFEST_BYTES as u64,
        observed,
        payload,
    })
}

async fn finish_manifest(
    manifest: Option<JoinHandle<Result<ManifestCapture, ExecutorError>>>,
) -> Result<Option<ManifestCapture>, ExecutorError> {
    match manifest {
        Some(task) => task
            .await
            .map_err(|error| ExecutorError::new(format!("manifest task failed: {error}")))?
            .map(Some),
        None => Ok(None),
    }
}

async fn read_event(reader: &mut File) -> Result<CompletionEvent, ExecutorError> {
    let mut payload = Vec::new();
    let mut block = [0_u8; 512];
    loop {
        let read = reader.read(&mut block).await.map_err(|error| {
            ExecutorError::new(format!("cannot read completion event: {error}"))
        })?;
        if read == 0 {
            return Err(ExecutorError::new(
                "completion event descriptor closed without an event",
            ));
        }
        if let Some(newline) = block[..read].iter().position(|byte| *byte == b'\n') {
            if newline + 1 != read {
                return Err(ExecutorError::new(
                    "completion event contains trailing data",
                ));
            }
            payload.extend_from_slice(&block[..newline]);
            break;
        }
        payload.extend_from_slice(&block[..read]);
        if payload.len() > MAX_EVENT_BYTES {
            return Err(ExecutorError::new("completion event exceeds 4096 bytes"));
        }
    }
    if payload.len() > MAX_EVENT_BYTES {
        return Err(ExecutorError::new("completion event exceeds 4096 bytes"));
    }
    serde_json::from_slice(&payload)
        .map_err(|error| ExecutorError::new(format!("invalid completion event: {error}")))
}

fn failure(
    status: ProcessStatus,
    exit_code: Option<i32>,
    reason: impl Into<String>,
) -> ProcessResult {
    ProcessResult {
        status,
        exit_code,
        reason: Some(reason.into()),
        output: OutputStats::default(),
        service: None,
        manifest: None,
        capacity: Default::default(),
    }
}

fn exit_code(status: std::process::ExitStatus) -> Option<i32> {
    status
        .code()
        .or_else(|| status.signal().map(|signal| -signal))
}

fn display_exit(status: std::process::ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .or_else(|| {
            status
                .signal()
                .map(|signal| format!("from signal {signal}"))
        })
        .unwrap_or_else(|| "without a status".into())
}

async fn leaf_deadline(timeout_seconds: Option<u64>) {
    match timeout_seconds {
        Some(seconds) => tokio::time::sleep(Duration::from_secs(seconds)).await,
        None => pending::<()>().await,
    }
}

fn deadline_reason(timeout_seconds: Option<u64>, kind: &str) -> String {
    match timeout_seconds {
        Some(seconds) => format!("{kind} exceeded {seconds} second deadline"),
        None => format!("{kind} deadline elapsed"),
    }
}
