//! Byte-complete, per-leaf governed-test log storage.
//!
//! The caller supplies a prevalidated log-store directory. Everything below
//! that directory is selected by validated identifiers and opened relative to
//! directory descriptors. Stream bytes are never copied into aggregate files
//! or metadata.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::{self as unix_fs, AtFlags, FlockOperation, Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use devcoordinator2_executor_protocol::{LogPhase, LogStream};

/// One sparse index entry is retained for every 256th logical line.
pub const LINE_INDEX_STRIDE: u64 = 256;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A validated selector for exactly one executor, check, discovery, or case
/// folder. Callers cannot construct path components through this type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeafSelector {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    pub phase: LogPhase,
    #[serde(rename = "case", skip_serializing_if = "Option::is_none")]
    pub case_id: Option<String>,
}

impl LeafSelector {
    pub fn executor() -> Self {
        Self {
            check: None,
            phase: LogPhase::Executor,
            case_id: None,
        }
    }

    pub fn check(check: impl Into<String>) -> Result<Self, LogStoreError> {
        Self::new(Some(check.into()), LogPhase::Check, None)
    }

    pub fn discovery(check: impl Into<String>) -> Result<Self, LogStoreError> {
        Self::new(Some(check.into()), LogPhase::Discovery, None)
    }

    pub fn case(
        check: impl Into<String>,
        case_id: impl Into<String>,
    ) -> Result<Self, LogStoreError> {
        Self::new(Some(check.into()), LogPhase::Case, Some(case_id.into()))
    }

    pub fn new(
        check: Option<String>,
        phase: LogPhase,
        case_id: Option<String>,
    ) -> Result<Self, LogStoreError> {
        match phase {
            LogPhase::Executor if check.is_none() && case_id.is_none() => {}
            LogPhase::Check | LogPhase::Discovery if check.is_some() && case_id.is_none() => {}
            LogPhase::Case if check.is_some() && case_id.is_some() => {}
            _ => {
                return Err(LogStoreError::InvalidSelector(
                    "phase, check, and case do not identify one log leaf",
                ));
            }
        }
        if let Some(value) = check.as_deref() {
            validate_check(value)?;
        }
        if let Some(value) = case_id.as_deref() {
            validate_case(value)?;
        }
        Ok(Self {
            check,
            phase,
            case_id,
        })
    }
}

/// Compact metadata written beside a stream after it is sealed or found
/// incomplete. It deliberately contains neither stream bytes nor command,
/// environment, caller, or arbitrary error text.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamMetadata {
    pub schema: u8,
    pub selector: LeafSelector,
    pub stream: LogStream,
    pub bytes: u64,
    pub lines: u64,
    pub first_write_epoch_ms: Option<u64>,
    pub last_write_epoch_ms: Option<u64>,
    pub complete: bool,
    pub truncated: bool,
    pub sha256: String,
    pub line_index_stride: u64,
    pub line_index_format: String,
}

/// The exact storage operation whose failure invalidated a stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IncompleteOperation {
    StreamWrite,
    IndexWrite,
    StreamSync,
    IndexSync,
    MetadataPublish,
}

impl IncompleteOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StreamWrite => "stream_write",
            Self::IndexWrite => "index_write",
            Self::StreamSync => "stream_sync",
            Self::IndexSync => "index_sync",
            Self::MetadataPublish => "metadata_publish",
        }
    }
}

/// Typed failures allow the executor to terminate a process group whenever
/// byte-complete storage can no longer be guaranteed.
#[derive(Debug)]
pub enum LogStoreError {
    InvalidRunId,
    InvalidComponent(&'static str),
    InvalidSelector(&'static str),
    LeaseContended,
    Io {
        operation: &'static str,
        source: io::Error,
    },
    StorageIncomplete {
        operation: IncompleteOperation,
        source: io::Error,
    },
}

impl LogStoreError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRunId | Self::InvalidComponent(_) | Self::InvalidSelector(_) => {
                "args_invalid"
            }
            Self::LeaseContended => "test_log_run_active",
            Self::Io { .. } => "test_log_unavailable",
            Self::StorageIncomplete { .. } => "test_log_storage_incomplete",
        }
    }

    pub const fn is_storage_incomplete(&self) -> bool {
        matches!(self, Self::StorageIncomplete { .. })
    }
}

impl fmt::Display for LogStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRunId => formatter.write_str("invalid governed-test run identifier"),
            Self::InvalidComponent(component) => {
                write!(formatter, "invalid governed-test {component} component")
            }
            Self::InvalidSelector(reason) => write!(formatter, "invalid log selector: {reason}"),
            Self::LeaseContended => formatter.write_str("governed-test run is already active"),
            Self::Io { operation, source } => write!(formatter, "{operation} failed: {source}"),
            Self::StorageIncomplete { operation, source } => write!(
                formatter,
                "log storage became incomplete during {}: {source}",
                operation.as_str()
            ),
        }
    }
}

impl Error for LogStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::StorageIncomplete { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// An exclusive lifetime lease for one stable run folder.
///
/// The `active.lock` inode remains in the run directory after the lease is
/// dropped; the advisory lock itself is held by `_lease` for this value's
/// lifetime.
pub struct RunLogLease {
    run_dir: File,
    _lease: File,
}

impl RunLogLease {
    /// Acquire a lease on the prevalidated, existing run `log_dir`.
    pub fn acquire(log_dir: &Path, run_id: &str) -> Result<Self, LogStoreError> {
        validate_run_id(run_id)?;
        if log_dir.file_name().and_then(|value| value.to_str()) != Some(run_id) {
            return Err(LogStoreError::InvalidSelector(
                "log directory basename does not match its run identifier",
            ));
        }
        let run_dir = open_directory_path(log_dir, "open run log directory")?;
        let lease = open_relative_file(
            &run_dir,
            "active.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        )?;
        match unix_fs::flock(&lease, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(error)
                if error == rustix::io::Errno::AGAIN || error == rustix::io::Errno::WOULDBLOCK =>
            {
                return Err(LogStoreError::LeaseContended);
            }
            Err(error) => {
                return Err(LogStoreError::Io {
                    operation: "lock active run",
                    source: error.into(),
                });
            }
        }
        run_dir.sync_all().map_err(|source| LogStoreError::Io {
            operation: "sync run directory",
            source,
        })?;
        Ok(Self {
            run_dir,
            _lease: lease,
        })
    }

    /// Create a new 0600 stream and its new 0600 sparse index in one leaf.
    /// Existing files, including symlinks, are never opened or truncated.
    pub fn create_stream(
        &self,
        selector: LeafSelector,
        stream: LogStream,
    ) -> Result<CompleteLogWriter, LogStoreError> {
        // Deserialized selectors receive the same validation as constructed
        // selectors before any component is used.
        let selector = LeafSelector::new(selector.check, selector.phase, selector.case_id)?;
        let leaf_dir = self.open_leaf(&selector)?;
        let log_name = format!("{}.log", stream_name(stream));
        let index_name = format!("{}.lines", stream_name(stream));
        let stream_file = open_new_relative_file(&leaf_dir, &log_name)?;
        let index_file = match open_new_relative_file(&leaf_dir, &index_name) {
            Ok(file) => file,
            Err(error) => {
                let _ = unix_fs::unlinkat(&leaf_dir, log_name.as_str(), AtFlags::empty());
                return Err(error);
            }
        };
        leaf_dir.sync_all().map_err(|source| LogStoreError::Io {
            operation: "sync leaf directory",
            source,
        })?;
        Ok(CompleteLogWriter {
            leaf_dir,
            selector,
            stream,
            stream_file,
            index_file,
            hasher: Sha256::new(),
            bytes: 0,
            lf_lines: 0,
            at_line_start: true,
            first_write_epoch_ms: None,
            last_write_epoch_ms: None,
            poisoned: false,
            sealed: false,
        })
    }

    fn open_leaf(&self, selector: &LeafSelector) -> Result<File, LogStoreError> {
        match selector.phase {
            LogPhase::Executor => ensure_directory(&self.run_dir, "executor"),
            LogPhase::Check | LogPhase::Discovery => {
                let checks = ensure_directory(&self.run_dir, "checks")?;
                let check = ensure_directory(
                    &checks,
                    selector.check.as_deref().expect("validated check selector"),
                )?;
                ensure_directory(&check, phase_name(selector.phase))
            }
            LogPhase::Case => {
                let checks = ensure_directory(&self.run_dir, "checks")?;
                let check = ensure_directory(
                    &checks,
                    selector.check.as_deref().expect("validated case selector"),
                )?;
                let cases = ensure_directory(&check, "cases")?;
                ensure_directory(
                    &cases,
                    selector
                        .case_id
                        .as_deref()
                        .expect("validated case selector"),
                )
            }
        }
    }
}

/// A byte-complete stream writer. It has no byte limit.
pub struct CompleteLogWriter {
    leaf_dir: File,
    selector: LeafSelector,
    stream: LogStream,
    stream_file: File,
    index_file: File,
    hasher: Sha256,
    bytes: u64,
    lf_lines: u64,
    at_line_start: bool,
    first_write_epoch_ms: Option<u64>,
    last_write_epoch_ms: Option<u64>,
    poisoned: bool,
    sealed: bool,
}

impl CompleteLogWriter {
    /// Persist all supplied bytes, updating the digest and sparse line index
    /// only for portions the operating system confirms were written.
    pub fn write_all(&mut self, mut payload: &[u8]) -> Result<(), LogStoreError> {
        if self.poisoned {
            return Err(self.incomplete_error(
                IncompleteOperation::StreamWrite,
                io::Error::other("stream was already incomplete"),
            ));
        }
        while !payload.is_empty() {
            let written = match self.stream_file.write(payload) {
                Ok(0) => {
                    return Err(self.fail(
                        IncompleteOperation::StreamWrite,
                        io::Error::from(io::ErrorKind::WriteZero),
                    ));
                }
                Ok(written) => written,
                Err(source) => return Err(self.fail(IncompleteOperation::StreamWrite, source)),
            };
            let stored = &payload[..written];
            let now = epoch_ms();
            self.first_write_epoch_ms.get_or_insert(now);
            self.last_write_epoch_ms = Some(now);
            self.hasher.update(stored);
            let base = self.bytes;
            self.bytes = self.bytes.saturating_add(written as u64);
            if let Err(source) = self.index_stored_bytes(stored, base) {
                return Err(self.fail(IncompleteOperation::IndexWrite, source));
            }
            payload = &payload[written..];
        }
        Ok(())
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes
    }

    pub fn line_count(&self) -> u64 {
        self.lf_lines + u64::from(!self.at_line_start)
    }

    /// Flush both cold-evidence files, then atomically publish compact stream
    /// metadata and fsync the leaf directory.
    pub fn seal(mut self) -> Result<StreamMetadata, LogStoreError> {
        if self.poisoned {
            return Err(self.incomplete_error(
                IncompleteOperation::StreamSync,
                io::Error::other("stream was already incomplete"),
            ));
        }
        if let Err(source) = self.stream_file.sync_all() {
            return Err(self.fail(IncompleteOperation::StreamSync, source));
        }
        if let Err(source) = self.index_file.sync_all() {
            return Err(self.fail(IncompleteOperation::IndexSync, source));
        }
        let metadata = self.metadata(true);
        if let Err(source) = self.publish_metadata(&metadata) {
            self.poisoned = true;
            return Err(self.incomplete_error(IncompleteOperation::MetadataPublish, source));
        }
        self.sealed = true;
        Ok(metadata)
    }

    fn index_stored_bytes(&mut self, stored: &[u8], base: u64) -> io::Result<()> {
        let mut records = Vec::new();
        for (position, byte) in stored.iter().enumerate() {
            if self.at_line_start {
                let line = self.lf_lines + 1;
                if (line - 1) % LINE_INDEX_STRIDE == 0 {
                    records.extend_from_slice(&line.to_le_bytes());
                    records.extend_from_slice(&(base + position as u64).to_le_bytes());
                }
                self.at_line_start = false;
            }
            if *byte == b'\n' {
                self.lf_lines = self.lf_lines.saturating_add(1);
                self.at_line_start = true;
            }
        }
        self.index_file.write_all(&records)
    }

    fn metadata(&self, complete: bool) -> StreamMetadata {
        StreamMetadata {
            schema: 2,
            selector: self.selector.clone(),
            stream: self.stream,
            bytes: self.bytes,
            lines: self.line_count(),
            first_write_epoch_ms: self.first_write_epoch_ms,
            last_write_epoch_ms: self.last_write_epoch_ms,
            complete,
            truncated: false,
            sha256: lower_hex(&self.hasher.clone().finalize()),
            line_index_stride: LINE_INDEX_STRIDE,
            line_index_format: "u64le-line-byte-v1".into(),
        }
    }

    fn publish_metadata(&self, metadata: &StreamMetadata) -> io::Result<()> {
        let mut payload = serde_json::to_vec(metadata).map_err(io::Error::other)?;
        payload.push(b'\n');
        atomic_write_relative(
            &self.leaf_dir,
            &format!("{}.meta.json", stream_name(self.stream)),
            &payload,
        )
    }

    fn fail(&mut self, operation: IncompleteOperation, source: io::Error) -> LogStoreError {
        self.poisoned = true;
        let _ = self.stream_file.sync_all();
        let _ = self.index_file.sync_all();
        let _ = self.publish_metadata(&self.metadata(false));
        self.incomplete_error(operation, source)
    }

    fn incomplete_error(&self, operation: IncompleteOperation, source: io::Error) -> LogStoreError {
        LogStoreError::StorageIncomplete { operation, source }
    }
}

impl Drop for CompleteLogWriter {
    fn drop(&mut self) {
        if self.sealed {
            return;
        }
        let _ = self.stream_file.sync_all();
        let _ = self.index_file.sync_all();
        let _ = self.publish_metadata(&self.metadata(false));
    }
}

fn open_directory_path(path: &Path, operation: &'static str) -> Result<File, LogStoreError> {
    unix_fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| LogStoreError::Io {
        operation,
        source: error.into(),
    })
}

const fn phase_name(phase: LogPhase) -> &'static str {
    match phase {
        LogPhase::Executor => "executor",
        LogPhase::Check => "check",
        LogPhase::Discovery => "discovery",
        LogPhase::Case => "case",
    }
}

const fn stream_name(stream: LogStream) -> &'static str {
    match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
    }
}

fn ensure_directory(parent: &File, component: &str) -> Result<File, LogStoreError> {
    match unix_fs::mkdirat(parent, component, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(error) => {
            return Err(LogStoreError::Io {
                operation: "create log directory",
                source: error.into(),
            });
        }
    }
    unix_fs::openat(
        parent,
        component,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| LogStoreError::Io {
        operation: "open log directory",
        source: error.into(),
    })
}

fn open_relative_file(parent: &File, name: &str, flags: OFlags) -> Result<File, LogStoreError> {
    unix_fs::openat(parent, name, flags, Mode::from_raw_mode(0o600))
        .map(File::from)
        .map_err(|error| LogStoreError::Io {
            operation: "open log file",
            source: error.into(),
        })
}

fn open_new_relative_file(parent: &File, name: &str) -> Result<File, LogStoreError> {
    open_relative_file(
        parent,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
    )
}

fn atomic_write_relative(parent: &File, name: &str, payload: &[u8]) -> io::Result<()> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = format!(".{name}-{}-{sequence}.tmp", std::process::id());
    let result = (|| -> io::Result<()> {
        let descriptor = unix_fs::openat(
            parent,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?;
        let mut file = File::from(descriptor);
        file.write_all(payload)?;
        file.sync_all()?;
        unix_fs::renameat(parent, temporary.as_str(), parent, name).map_err(io::Error::from)?;
        parent.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = unix_fs::unlinkat(parent, temporary.as_str(), AtFlags::empty());
    }
    result
}

fn validate_run_id(value: &str) -> Result<(), LogStoreError> {
    let bytes = value.as_bytes();
    let valid = bytes.len() == 24
        && bytes[0] == b't'
        && bytes[1..9].iter().all(u8::is_ascii_digit)
        && bytes[9] == b'T'
        && bytes[10..16].iter().all(u8::is_ascii_digit)
        && bytes[16] == b'Z'
        && bytes[17] == b'-'
        && bytes[18..]
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    if valid {
        Ok(())
    } else {
        Err(LogStoreError::InvalidRunId)
    }
}

fn validate_check(value: &str) -> Result<(), LogStoreError> {
    let bytes = value.as_bytes();
    if !(1..=64).contains(&bytes.len())
        || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(LogStoreError::InvalidComponent("check"));
    }
    Ok(())
}

fn validate_case(value: &str) -> Result<(), LogStoreError> {
    let bytes = value.as_bytes();
    if !(1..=128).contains(&bytes.len())
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
    {
        return Err(LogStoreError::InvalidComponent("case"));
    }
    Ok(())
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn lower_hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::path::PathBuf;

    use super::*;

    fn temporary(name: &str) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dc2-log-store-{name}-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary log root");
        path
    }

    fn run_id(sequence: u32) -> String {
        format!("t20260902T120000Z-{sequence:06x}")
    }

    fn stream_path(root: &Path, run: &str, check: &str, case_id: &str, stream: &str) -> PathBuf {
        root.join("runs")
            .join(run)
            .join("checks")
            .join(check)
            .join("cases")
            .join(case_id)
            .join(stream)
    }

    fn acquire(root: &Path, run: &str) -> RunLogLease {
        let run_dir = root.join("runs").join(run);
        fs::create_dir_all(&run_dir).expect("run directory");
        RunLogLease::acquire(&run_dir, run).expect("lease")
    }

    #[test]
    fn retains_more_than_four_mibibytes_with_exact_hash_and_sentinel() {
        let root = temporary("large");
        let run = run_id(1);
        let lease = acquire(&root, &run);
        let mut writer = lease
            .create_stream(
                LeafSelector::case("integration", "large-output").expect("selector"),
                LogStream::Stdout,
            )
            .expect("stream");
        let mut payload = vec![b'x'; 4 * 1024 * 1024 + 73];
        payload.extend_from_slice(b"\nFINAL-SENTINEL-WITHOUT-LF");
        writer.write_all(&payload).expect("write without cap");
        let metadata = writer.seal().expect("seal");
        let stored = fs::read(stream_path(
            &root,
            &run,
            "integration",
            "large-output",
            "stdout.log",
        ))
        .expect("stored stream");
        assert_eq!(stored, payload);
        assert!(stored.ends_with(b"FINAL-SENTINEL-WITHOUT-LF"));
        assert_eq!(metadata.bytes, payload.len() as u64);
        assert_eq!(metadata.lines, 2);
        assert_eq!(metadata.sha256, lower_hex(&Sha256::digest(&payload)));
        assert!(!metadata.truncated);
        assert_eq!(
            fs::metadata(stream_path(
                &root,
                &run,
                "integration",
                "large-output",
                "stdout.log",
            ))
            .expect("mode")
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
        drop(lease);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn counts_empty_unterminated_binary_and_huge_lines_exactly() {
        let root = temporary("line-shapes");
        let run = run_id(2);
        let lease = acquire(&root, &run);
        let samples: [(&str, Vec<u8>, u64); 4] = [
            ("empty", vec![], 0),
            ("unterminated", b"one line".to_vec(), 1),
            ("binary", vec![0, 0xff, b'\n', 0x80], 2),
            ("huge", vec![b'z'; 2 * 1024 * 1024], 1),
        ];
        for (case_id, payload, expected_lines) in samples {
            let mut writer = lease
                .create_stream(
                    LeafSelector::case("unit", case_id).expect("selector"),
                    LogStream::Stderr,
                )
                .expect("stream");
            writer.write_all(&payload).expect("write");
            let metadata = writer.seal().expect("seal");
            assert_eq!(metadata.lines, expected_lines, "{case_id}");
            assert_eq!(metadata.bytes, payload.len() as u64, "{case_id}");
            assert_eq!(metadata.first_write_epoch_ms.is_some(), !payload.is_empty());
            assert_eq!(metadata.last_write_epoch_ms.is_some(), !payload.is_empty());
            assert_eq!(metadata.sha256, lower_hex(&Sha256::digest(&payload)));
        }
        drop(lease);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn check_and_case_streams_are_isolated_without_an_aggregate_copy() {
        let root = temporary("isolation");
        let run = run_id(3);
        let lease = acquire(&root, &run);
        for (case_id, payload) in [("parser-1", b"alpha".as_slice()), ("parser-2", b"beta")] {
            let mut writer = lease
                .create_stream(
                    LeafSelector::case("unit", case_id).expect("selector"),
                    LogStream::Stdout,
                )
                .expect("stream");
            writer.write_all(payload).expect("write");
            writer.seal().expect("seal");
        }
        assert_eq!(
            fs::read(stream_path(&root, &run, "unit", "parser-1", "stdout.log")).expect("first"),
            b"alpha"
        );
        assert_eq!(
            fs::read(stream_path(&root, &run, "unit", "parser-2", "stdout.log")).expect("second"),
            b"beta"
        );
        assert!(!root.join("runs").join(&run).join("stdout.log").exists());
        assert!(!root.join("runs").join(&run).join("aggregate.log").exists());
        drop(lease);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejects_invalid_components_and_preexisting_symlinks() {
        for invalid in ["", "../unit", "unit/test", "UPPER", ".hidden"] {
            assert!(LeafSelector::check(invalid).is_err(), "{invalid:?}");
        }
        for invalid in ["", "../case", "case/name", ".hidden", "space here"] {
            assert!(LeafSelector::case("unit", invalid).is_err(), "{invalid:?}");
        }
        assert!(RunLogLease::acquire(Path::new("/nonexistent"), "../escape").is_err());

        let root = temporary("symlink");
        let run = run_id(4);
        let lease = acquire(&root, &run);
        let leaf = stream_path(&root, &run, "unit", "linked", "stdout.log");
        fs::create_dir_all(leaf.parent().expect("parent")).expect("leaf directory");
        let target = root.join("outside");
        fs::write(&target, b"must survive").expect("target");
        symlink(&target, &leaf).expect("symlink");
        assert!(
            lease
                .create_stream(
                    LeafSelector::case("unit", "linked").expect("selector"),
                    LogStream::Stdout,
                )
                .is_err()
        );
        assert_eq!(fs::read(target).expect("target"), b"must survive");
        drop(lease);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn metadata_contains_no_raw_output_or_private_execution_fields() {
        let root = temporary("privacy");
        let run = run_id(5);
        let lease = acquire(&root, &run);
        let secret = b"raw-private-sentinel";
        let mut writer = lease
            .create_stream(
                LeafSelector::case("unit", "privacy").expect("selector"),
                LogStream::Stderr,
            )
            .expect("stream");
        writer.write_all(secret).expect("write");
        writer.seal().expect("seal");
        let metadata_path = stream_path(&root, &run, "unit", "privacy", "stderr.meta.json");
        let encoded = fs::read_to_string(metadata_path).expect("metadata");
        assert!(!encoded.contains("raw-private-sentinel"));
        for forbidden in [
            "command",
            "environment",
            "caller",
            "absolute_path",
            "error_text",
        ] {
            assert!(!encoded.contains(forbidden), "unexpected field {forbidden}");
        }
        let decoded: StreamMetadata = serde_json::from_str(&encoded).expect("typed metadata");
        assert!(decoded.complete);
        assert_eq!(decoded.bytes, secret.len() as u64);
        drop(lease);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn active_run_lease_is_exclusive() {
        let root = temporary("lease");
        let run = run_id(6);
        let run_dir = root.join("runs").join(&run);
        fs::create_dir_all(&run_dir).expect("run directory");
        let first = RunLogLease::acquire(&run_dir, &run).expect("first lease");
        assert!(matches!(
            RunLogLease::acquire(&run_dir, &run),
            Err(LogStoreError::LeaseContended)
        ));
        drop(first);
        let reacquired = RunLogLease::acquire(&run_dir, &run).expect("released lease");
        drop(reacquired);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn sparse_index_records_stable_one_based_lines_and_byte_offsets() {
        let root = temporary("index");
        let run = run_id(7);
        let lease = acquire(&root, &run);
        let mut payload = Vec::new();
        for _ in 0..=LINE_INDEX_STRIDE {
            payload.extend_from_slice(b"x\n");
        }
        let mut writer = lease
            .create_stream(
                LeafSelector::case("unit", "indexed").expect("selector"),
                LogStream::Stdout,
            )
            .expect("stream");
        // Split on a line boundary to prove indexing is independent of writes.
        writer.write_all(&payload[..2]).expect("first write");
        writer.write_all(&payload[2..]).expect("second write");
        writer.seal().expect("seal");
        let index =
            fs::read(stream_path(&root, &run, "unit", "indexed", "stdout.lines")).expect("index");
        let records: Vec<(u64, u64)> = index
            .chunks_exact(16)
            .map(|row| {
                let line = u64::from_le_bytes(row[..8].try_into().expect("line"));
                let offset = u64::from_le_bytes(row[8..].try_into().expect("offset"));
                (line, offset)
            })
            .collect();
        assert_eq!(records, vec![(1, 0), (257, 512)]);
        drop(lease);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn dropped_unsealed_stream_is_marked_incomplete() {
        let root = temporary("incomplete");
        let run = run_id(8);
        let lease = acquire(&root, &run);
        let mut writer = lease
            .create_stream(
                LeafSelector::case("unit", "dropped").expect("selector"),
                LogStream::Stdout,
            )
            .expect("stream");
        writer.write_all(b"partial").expect("write");
        drop(writer);
        let encoded = fs::read_to_string(stream_path(
            &root,
            &run,
            "unit",
            "dropped",
            "stdout.meta.json",
        ))
        .expect("metadata");
        let metadata: StreamMetadata = serde_json::from_str(&encoded).expect("decode");
        assert!(!metadata.complete);
        assert_eq!(metadata.bytes, 7);
        drop(lease);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn stream_and_index_identity_are_regular_private_files() {
        let root = temporary("identity");
        let run = run_id(9);
        let lease = acquire(&root, &run);
        let writer = lease
            .create_stream(
                LeafSelector::case("unit", "identity").expect("selector"),
                LogStream::Stdout,
            )
            .expect("stream");
        writer.seal().expect("seal");
        for name in ["stdout.log", "stdout.lines", "stdout.meta.json"] {
            let metadata = fs::symlink_metadata(stream_path(&root, &run, "unit", "identity", name))
                .expect("metadata");
            assert!(metadata.file_type().is_file());
            assert!(!metadata.file_type().is_symlink());
            assert_eq!(metadata.mode() & 0o777, 0o600);
        }
        drop(lease);
        fs::remove_dir_all(root).expect("cleanup");
    }
}
