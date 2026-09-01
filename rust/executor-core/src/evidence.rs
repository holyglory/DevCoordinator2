use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use devcoordinator2_executor_protocol::{ArtifactReceipt, MAX_REPORT_BYTES};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ExecutorError;

const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const COPY_BLOCK_BYTES: usize = 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn source_digest(worktree_root: &Path) -> Result<String, ExecutorError> {
    let root = worktree_root
        .canonicalize()
        .map_err(|error| ExecutorError::new(format!("cannot resolve worktree root: {error}")))?;
    if !root.is_dir() {
        return Err(ExecutorError::new("worktree root is not a directory"));
    }
    let index = git_output(&root, &["ls-files", "--stage", "-z"])?;
    let modified = decode_paths(git_output(
        &root,
        &["diff-files", "--name-only", "--no-ext-diff", "-z"],
    )?)?;
    let untracked = decode_paths(git_output(
        &root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)?;

    let mut digest = Sha256::new();
    digest.update(b"devcoordinator2-source-v2\0index\0");
    digest.update(index);
    digest.update(b"\0");
    let paths: BTreeSet<String> = modified.into_iter().chain(untracked).collect();
    for relative in paths {
        if relative == ".devcoordinator" || relative.starts_with(".devcoordinator/") {
            continue;
        }
        hash_source_path(&root, &relative, &mut digest)?;
    }
    Ok(lower_hex(&digest.finalize()))
}

pub fn artifact_receipts(
    worktree_root: &Path,
    paths: &[String],
) -> Result<Vec<ArtifactReceipt>, ExecutorError> {
    let root = worktree_root
        .canonicalize()
        .map_err(|error| ExecutorError::new(format!("cannot resolve worktree root: {error}")))?;
    paths
        .iter()
        .map(|relative| artifact_receipt(&root, relative))
        .collect()
}

pub fn receipts_match(
    worktree_root: &Path,
    receipts: &[ArtifactReceipt],
) -> Result<bool, ExecutorError> {
    let paths: Vec<String> = receipts.iter().map(|row| row.path.clone()).collect();
    Ok(artifact_receipts(worktree_root, &paths)? == receipts)
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), ExecutorError> {
    let mut payload = serde_json::to_vec(value)
        .map_err(|error| ExecutorError::new(format!("cannot encode report: {error}")))?;
    payload.push(b'\n');
    write_bytes_atomic(path, &payload, MAX_REPORT_BYTES)
}

pub fn write_bytes_atomic(
    path: &Path,
    payload: &[u8],
    maximum: usize,
) -> Result<(), ExecutorError> {
    if payload.len() > maximum {
        return Err(ExecutorError::new(format!(
            "artifact {:?} exceeds its {} byte limit",
            path.file_name().unwrap_or_default(),
            maximum
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ExecutorError::new("artifact path has no parent"))?;
    fs::create_dir_all(parent).map_err(|error| {
        ExecutorError::new(format!("cannot create artifact directory: {error}"))
    })?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact");
    let temporary = parent.join(format!(".{filename}-{}-{sequence}.tmp", std::process::id()));
    let result = (|| -> Result<(), ExecutorError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| ExecutorError::new(format!("cannot create artifact: {error}")))?;
        file.write_all(payload)
            .and_then(|_| file.sync_all())
            .map_err(|error| ExecutorError::new(format!("cannot persist artifact: {error}")))?;
        fs::rename(&temporary, path)
            .map_err(|error| ExecutorError::new(format!("cannot publish artifact: {error}")))?;
        let directory = File::open(parent).map_err(|error| {
            ExecutorError::new(format!("cannot open artifact directory: {error}"))
        })?;
        directory.sync_all().map_err(|error| {
            ExecutorError::new(format!("cannot sync artifact directory: {error}"))
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>, ExecutorError> {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "safe.directory")
        .env("GIT_CONFIG_VALUE_0", "*")
        .output()
        .map_err(|error| ExecutorError::new(format!("cannot inventory source: {error}")))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(ExecutorError::new(format!(
            "cannot inventory source: {}",
            bounded(&detail, 512)
        )));
    }
    if output.stdout.len() > MAX_GIT_OUTPUT_BYTES {
        return Err(ExecutorError::new(
            "repository source inventory exceeds 16 MiB",
        ));
    }
    Ok(output.stdout)
}

fn decode_paths(payload: Vec<u8>) -> Result<Vec<String>, ExecutorError> {
    let mut paths = Vec::new();
    for raw in payload
        .split(|byte| *byte == 0)
        .filter(|row| !row.is_empty())
    {
        let value = std::str::from_utf8(raw)
            .map_err(|_| ExecutorError::new("repository contains a non-UTF-8 source path"))?;
        validate_relative(value)?;
        paths.push(value.to_owned());
    }
    let unique: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
    if unique.len() != paths.len() {
        return Err(ExecutorError::new(
            "repository source inventory contains duplicate paths",
        ));
    }
    Ok(paths)
}

fn hash_source_path(root: &Path, relative: &str, digest: &mut Sha256) -> Result<(), ExecutorError> {
    validate_relative(relative)?;
    digest.update(relative.as_bytes());
    digest.update(b"\0");
    let path = root.join(relative);
    let before = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            digest.update(b"missing\0");
            return Ok(());
        }
        Err(error) => {
            return Err(ExecutorError::new(format!(
                "cannot inspect source path {relative:?}: {error}"
            )));
        }
    };
    digest.update(format!("{:o}\0", before.permissions().mode() & 0o7777).as_bytes());
    if before.file_type().is_symlink() {
        let target = fs::read_link(&path).map_err(|error| {
            ExecutorError::new(format!("cannot read source symlink {relative:?}: {error}"))
        })?;
        digest.update(b"symlink\0");
        digest.update(target.as_os_str().as_encoded_bytes());
        digest.update(b"\0");
        return Ok(());
    }
    if before.is_dir() {
        digest.update(b"gitlink\0");
        return Ok(());
    }
    if !before.is_file() {
        return Err(ExecutorError::new(format!(
            "source path {relative:?} is not a regular file"
        )));
    }
    let mut file = File::open(&path).map_err(|error| {
        ExecutorError::new(format!("cannot open source path {relative:?}: {error}"))
    })?;
    let mut block = vec![0_u8; COPY_BLOCK_BYTES];
    loop {
        let read = file.read(&mut block).map_err(|error| {
            ExecutorError::new(format!("cannot read source path {relative:?}: {error}"))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&block[..read]);
    }
    let after = file.metadata().map_err(|error| {
        ExecutorError::new(format!("cannot restat source path {relative:?}: {error}"))
    })?;
    if identity(&before) != identity(&after) {
        return Err(ExecutorError::new(format!(
            "source path {relative:?} changed while hashing"
        )));
    }
    Ok(())
}

fn artifact_receipt(root: &Path, relative: &str) -> Result<ArtifactReceipt, ExecutorError> {
    validate_relative(relative)?;
    let path = root.join(relative);
    let before = fs::symlink_metadata(&path).map_err(|error| {
        ExecutorError::new(format!(
            "declared artifact {relative:?} is unavailable: {error}"
        ))
    })?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(ExecutorError::new(format!(
            "declared artifact {relative:?} is not a regular file"
        )));
    }
    let canonical = path.canonicalize().map_err(|error| {
        ExecutorError::new(format!("cannot resolve artifact {relative:?}: {error}"))
    })?;
    if !canonical.starts_with(root) {
        return Err(ExecutorError::new(format!(
            "declared artifact {relative:?} escapes the worktree"
        )));
    }
    let mut file = File::open(&canonical).map_err(|error| {
        ExecutorError::new(format!("cannot open artifact {relative:?}: {error}"))
    })?;
    let mut digest = Sha256::new();
    let mut block = vec![0_u8; COPY_BLOCK_BYTES];
    loop {
        let read = file.read(&mut block).map_err(|error| {
            ExecutorError::new(format!("cannot read artifact {relative:?}: {error}"))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&block[..read]);
    }
    let after = file.metadata().map_err(|error| {
        ExecutorError::new(format!("cannot restat artifact {relative:?}: {error}"))
    })?;
    if identity(&before) != identity(&after) {
        return Err(ExecutorError::new(format!(
            "declared artifact {relative:?} changed while hashing"
        )));
    }
    Ok(ArtifactReceipt {
        path: relative.into(),
        size: before.len(),
        sha256: lower_hex(&digest.finalize()),
    })
}

fn validate_relative(value: &str) -> Result<(), ExecutorError> {
    let path = PathBuf::from(value);
    if value.is_empty()
        || path.is_absolute()
        || value.contains('\\')
        || value.as_bytes().contains(&0)
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(ExecutorError::new(format!(
            "path {value:?} is not normalized repository-relative"
        )));
    }
    Ok(())
}

fn identity(metadata: &fs::Metadata) -> (u64, u64, i64, i64) {
    (
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    )
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}

fn bounded(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn temporary(name: &str) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "dc2-executor-evidence-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("temporary directory");
        path
    }

    #[test]
    fn artifact_receipt_hashes_regular_file_and_rejects_symlink() {
        let root = temporary("receipt");
        fs::write(root.join("artifact.txt"), b"evidence").expect("artifact");
        let receipts = artifact_receipts(&root, &["artifact.txt".into()]).expect("receipt");
        assert_eq!(receipts[0].size, 8);
        assert_eq!(receipts[0].sha256.len(), 64);
        symlink("artifact.txt", root.join("link.txt")).expect("symlink");
        assert!(artifact_receipts(&root, &["link.txt".into()]).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn atomic_json_write_is_bounded() {
        let root = temporary("atomic");
        let path = root.join("report.json");
        write_json_atomic(&path, &serde_json::json!({"schema": 2})).expect("write");
        assert_eq!(fs::read_to_string(&path).expect("read"), "{\"schema\":2}\n");
        let oversized = vec![b'x'; MAX_REPORT_BYTES + 1];
        assert!(write_bytes_atomic(&path, &oversized, MAX_REPORT_BYTES).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
