//! Private, bounded hashing for an already authorized worktree's retained data.
use super::{DiscoveryBudget, HostBackend};
use crate::storage::{
    blocked, fs, json,
    model::{Context, IgnoredFileStability, IgnoredReview, IgnoredReviewFile, JobIgnoredReview},
    unavailable,
};
use devcoordinator2_api::{ProtocolError, storage as api};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

pub(super) const MAX_JOB_REVIEW_FILES: usize = 1024;
pub(super) const MAX_JOB_REVIEW_BYTES: u64 = 64 * 1024 * 1024;
const MAX_JOB_MANIFEST_BYTES: usize = 48 * 1024;
const HASH_STEP_BYTES: usize = 256 * 1024;

pub(crate) fn validate_job_manifest(files: &[api::IgnoredFileReview]) -> Result<(), ProtocolError> {
    if files.len() > MAX_JOB_REVIEW_FILES {
        return Err(blocked("ignored_review_file_limit"));
    }
    if json(&files)?.len() > MAX_JOB_MANIFEST_BYTES {
        return Err(blocked("ignored_review_manifest_limit"));
    }
    let mut seen = BTreeSet::new();
    for file in files {
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(blocked("ignored_review_digest_invalid"));
        }
        let path = Path::new(&file.path);
        if !path.is_relative()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || !HostBackend::ignored_review_path_allowed(path)
            || !seen.insert(path.to_owned())
        {
            return Err(blocked("ignored_review_path_invalid"));
        }
    }
    Ok(())
}

pub(super) fn check_listing(
    files: &[api::IgnoredFileReview],
    ignored: &[u8],
) -> Result<(), ProtocolError> {
    if !ignored.is_empty() && ignored.last() != Some(&0) {
        return Err(blocked("ignored_review_listing_invalid"));
    }
    let listed = ignored
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            std::str::from_utf8(entry)
                .map(PathBuf::from)
                .map_err(|_| blocked("ignored_review_listing_invalid"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if listed != files.iter().map(|file| PathBuf::from(&file.path)).collect() {
        return Err(blocked("unreviewed_ignored_files"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use rustix::fs::{self as unix, Mode, OFlags};
    use sha2::Digest;
    use std::{fs::File, io::Read, time::Instant};

    fn identity(file: &File, path: &Path) -> Result<IgnoredReviewFile, ProtocolError> {
        let stat = unix::fstat(file).map_err(|_| unavailable("ignored_review_file_unavailable"))?;
        if unix::FileType::from_raw_mode(stat.st_mode) != unix::FileType::RegularFile {
            return Err(blocked("ignored_review_file_not_regular"));
        }
        Ok(IgnoredReviewFile {
            path: path.into(),
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
            size: u64::try_from(stat.st_size).map_err(|_| blocked("ignored_review_size_limit"))?,
            sha256: String::new(),
            stability: Some(IgnoredFileStability {
                modified_seconds: stat.st_mtime,
                modified_nanoseconds: stat.st_mtime_nsec as i64,
                changed_seconds: stat.st_ctime,
                changed_nanoseconds: stat.st_ctime_nsec as i64,
            }),
        })
    }

    fn open(root: &Path, relative: &Path) -> Result<(File, IgnoredReviewFile), ProtocolError> {
        let full = root.join(relative);
        let parent = fs::open_directory(
            full.parent()
                .ok_or_else(|| blocked("ignored_review_path_invalid"))?,
        )?;
        let file = File::from(
            unix::openat(
                &parent,
                full.file_name()
                    .ok_or_else(|| blocked("ignored_review_path_invalid"))?,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|_| blocked("ignored_review_file_unavailable"))?,
        );
        let snapshot = identity(&file, relative)?;
        Ok((file, snapshot))
    }

    fn same_file(left: &IgnoredReviewFile, right: &IgnoredReviewFile) -> bool {
        left.device == right.device
            && left.inode == right.inode
            && left.size == right.size
            && left.stability == right.stability
    }

    pub(crate) fn validate_snapshot(
        root: &Path,
        expected: &IgnoredReviewFile,
    ) -> Result<(), ProtocolError> {
        let (_, current) = open(root, &expected.path)?;
        if expected.stability.is_none() || !same_file(expected, &current) {
            return Err(blocked("ignored_review_file_changed"));
        }
        Ok(())
    }

    struct PendingFile {
        file: File,
        before: IgnoredReviewFile,
        digest: sha2::Sha256,
        read: u64,
    }

    pub(crate) struct JobReviewCursor {
        root: PathBuf,
        expected: Vec<api::IgnoredFileReview>,
        files: Vec<IgnoredReviewFile>,
        total: u64,
        current: Option<PendingFile>,
    }

    impl JobReviewCursor {
        pub(crate) fn new(
            root: &Path,
            expected: Vec<api::IgnoredFileReview>,
        ) -> Result<Self, ProtocolError> {
            validate_job_manifest(&expected)?;
            Ok(Self {
                root: root.into(),
                expected,
                files: vec![],
                total: 0,
                current: None,
            })
        }

        pub(crate) fn step(&mut self, budget: DiscoveryBudget) -> Result<bool, ProtocolError> {
            let start = Instant::now();
            let mut read = 0;
            let mut completed = 0;
            let mut buffer = [0_u8; 64 * 1024];
            while read < HASH_STEP_BYTES
                && completed < budget.entries.max(1)
                && (read == 0 && completed == 0 || start.elapsed() < budget.elapsed)
            {
                let index = self.files.len();
                if index == self.expected.len() {
                    return Ok(true);
                }
                if self.current.is_none() {
                    let (file, before) = open(&self.root, Path::new(&self.expected[index].path))?;
                    if before.size > MAX_JOB_REVIEW_BYTES.saturating_sub(self.total) {
                        return Err(blocked("ignored_review_size_limit"));
                    }
                    self.current = Some(PendingFile {
                        file,
                        before,
                        digest: sha2::Sha256::new(),
                        read: 0,
                    });
                }
                let current = self.current.as_mut().expect("file opened");
                let available = buffer.len().min(HASH_STEP_BYTES - read);
                let count = current
                    .file
                    .read(&mut buffer[..available])
                    .map_err(|_| unavailable("ignored_review_file_unavailable"))?;
                if count != 0 {
                    current.read = current.read.saturating_add(count as u64);
                    if current.read > current.before.size {
                        return Err(blocked("ignored_review_file_changed"));
                    }
                    current.digest.update(&buffer[..count]);
                    read += count;
                    continue;
                }
                let current = self.current.take().expect("file completed");
                let after = identity(&current.file, &current.before.path)?;
                if current.read != current.before.size || !same_file(&current.before, &after) {
                    return Err(blocked("ignored_review_file_changed"));
                }
                validate_snapshot(&self.root, &current.before)?;
                let digest = crate::storage::hex(&current.digest.finalize());
                if digest != self.expected[index].sha256.to_ascii_lowercase() {
                    return Err(blocked("ignored_review_digest_mismatch"));
                }
                self.total += current.before.size;
                self.files.push(IgnoredReviewFile {
                    sha256: digest,
                    ..current.before
                });
                completed += 1;
            }
            Ok(self.files.len() == self.expected.len() && self.current.is_none())
        }

        #[cfg(any(test, feature = "root-acceptance"))]
        pub(crate) fn bytes_hashed(&self) -> u64 {
            self.total + self.current.as_ref().map_or(0, |file| file.read)
        }

        pub(crate) fn finish(
            self,
            worktree: (u64, u64),
            baseline: String,
            observed_at: u64,
        ) -> IgnoredReview {
            IgnoredReview {
                worktree_device: worktree.0,
                worktree_inode: worktree.1,
                files: self.files,
                total_bytes: self.total,
                job: Some(JobIgnoredReview {
                    source_baseline: baseline,
                    verified_at_ms: observed_at,
                }),
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) use platform::{JobReviewCursor, validate_snapshot};

#[cfg(target_os = "linux")]
pub(crate) fn validate_job_files(
    root: &Path,
    review: &IgnoredReview,
    context: &Context,
) -> Result<(), ProtocolError> {
    if fs::identity(root)? != (review.worktree_device, review.worktree_inode) {
        return Err(blocked("ignored_review_identity_changed"));
    }
    if review.files.len() > MAX_JOB_REVIEW_FILES {
        return Err(blocked("ignored_review_file_limit"));
    }
    if review.total_bytes > MAX_JOB_REVIEW_BYTES {
        return Err(blocked("ignored_review_size_limit"));
    }
    if context.ignored_cleanup_permit.is_none() && !context.discovering {
        return Err(blocked("ignored_review_job_required"));
    }
    for file in &review.files {
        validate_snapshot(root, file)?;
    }
    let mut total = 0_u64;
    for file in &review.files {
        if context.ignored_cleanup_permit.is_some() {
            let current = HostBackend::hash_ignored_file(
                root,
                &file.path,
                MAX_JOB_REVIEW_BYTES.saturating_sub(total),
            )?;
            if current.device != file.device
                || current.inode != file.inode
                || current.size != file.size
                || current.sha256 != file.sha256
            {
                return Err(blocked("ignored_review_file_changed"));
            }
            validate_snapshot(root, file)?;
        }
        total = total.saturating_add(file.size);
        if total > MAX_JOB_REVIEW_BYTES {
            return Err(blocked("ignored_review_size_limit"));
        }
    }
    if total != review.total_bytes {
        return Err(blocked("ignored_review_file_changed"));
    }
    if fs::identity(root)? != (review.worktree_device, review.worktree_inode) {
        return Err(blocked("ignored_review_identity_changed"));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) struct JobReviewCursor;

#[cfg(not(target_os = "linux"))]
impl JobReviewCursor {
    pub(super) fn new(_: &Path, _: Vec<api::IgnoredFileReview>) -> Result<Self, ProtocolError> {
        Err(unavailable("ignored_review_unsupported"))
    }
    pub(super) fn step(&mut self, _: DiscoveryBudget) -> Result<bool, ProtocolError> {
        Err(unavailable("ignored_review_unsupported"))
    }
    #[cfg(any(test, feature = "root-acceptance"))]
    pub(super) fn bytes_hashed(&self) -> u64 {
        0
    }
    pub(super) fn finish(self, _: (u64, u64), _: String, _: u64) -> IgnoredReview {
        unreachable!("unsupported review never starts")
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        fs::{self as stdfs, OpenOptions},
        io::Write,
        time::Duration,
    };

    fn manifest(path: &str, bytes: &[u8]) -> api::IgnoredFileReview {
        api::IgnoredFileReview {
            path: path.into(),
            sha256: crate::storage::hash(bytes),
        }
    }

    fn complete(cursor: &mut JobReviewCursor) -> Result<usize, ProtocolError> {
        let mut steps = 0;
        loop {
            steps += 1;
            assert!(steps < 10_000, "bounded fixture never completed");
            let before = cursor.bytes_hashed();
            let done = cursor.step(DiscoveryBudget {
                entries: 4,
                elapsed: Duration::from_millis(25),
            })?;
            assert!(cursor.bytes_hashed() - before <= HASH_STEP_BYTES as u64);
            if done {
                return Ok(steps);
            }
        }
    }

    // The real root journey covers storage RPCs, persistence and removal. These
    // isolated checks exercise in-read changes and exact per-step limits that
    // cannot reliably be timed through an unprivileged daemon journey.
    #[test]
    fn job_review_hashes_beyond_both_small_limits_in_bounded_steps() {
        let root = tempfile::tempdir().unwrap();
        let body = vec![42_u8; 8 * 1024 * 1024 + 1];
        stdfs::write(root.path().join("large.bin"), &body).unwrap();
        let mut files = vec![manifest("large.bin", &body)];
        for index in 0..129 {
            let name = format!("small-{index}.bin");
            stdfs::write(root.path().join(&name), [index as u8]).unwrap();
            files.push(manifest(&name, &[index as u8]));
        }
        let mut cursor = JobReviewCursor::new(root.path(), files).unwrap();
        assert!(!cursor.step(DiscoveryBudget::default()).unwrap());
        assert!(complete(&mut cursor).unwrap() > 32);
        let review = cursor.finish(fs::identity(root.path()).unwrap(), "source".into(), 123);
        assert_eq!(review.files.len(), 130);
        assert_eq!(review.total_bytes, body.len() as u64 + 129);
        assert_eq!(review.job.as_ref().unwrap().verified_at_ms, 123);
        assert!(review.files.iter().all(|file| file.stability.is_some()));
        let mut context = Context {
            discovering: true,
            ..Context::default()
        };
        validate_job_files(root.path(), &review, &context).unwrap();
        context.discovering = false;
        assert_eq!(
            validate_job_files(root.path(), &review, &context)
                .unwrap_err()
                .message,
            "ignored_review_job_required"
        );
        assert_eq!(review.job.unwrap().verified_at_ms, 123);
    }

    #[test]
    fn job_review_rejects_growth_replacement_and_symlinked_parents() {
        for change in ["growth", "replacement", "symlink"] {
            let root = tempfile::tempdir().unwrap();
            let body = vec![7_u8; 1024 * 1024];
            let directory = root.path().join("captures");
            stdfs::create_dir(&directory).unwrap();
            let file = directory.join("image.bin");
            stdfs::write(&file, &body).unwrap();
            let mut cursor =
                JobReviewCursor::new(root.path(), vec![manifest("captures/image.bin", &body)])
                    .unwrap();
            assert!(!cursor.step(DiscoveryBudget::default()).unwrap());
            match change {
                "growth" => OpenOptions::new()
                    .append(true)
                    .open(&file)
                    .unwrap()
                    .write_all(b"growth")
                    .unwrap(),
                "replacement" => {
                    stdfs::rename(&file, directory.join("saved.bin")).unwrap();
                    stdfs::write(&file, &body).unwrap();
                }
                _ => {
                    stdfs::rename(&directory, root.path().join("saved")).unwrap();
                    std::os::unix::fs::symlink(root.path().join("saved"), &directory).unwrap();
                }
            }
            assert!(
                complete(&mut cursor).is_err(),
                "{change} passed a partial hash"
            );
        }
    }

    #[test]
    fn job_review_manifest_and_listing_fail_closed() {
        let legacy: IgnoredReview = serde_json::from_value(serde_json::json!({
            "worktree_device":1,"worktree_inode":2,"files":[],"total_bytes":0
        }))
        .unwrap();
        assert!(legacy.job.is_none());
        for path in [
            "../escape",
            "/absolute",
            ".env",
            "chat/messages.json",
            ".devcoordinator/test/logs/run/stdout",
        ] {
            assert!(
                validate_job_manifest(&[manifest(path, b"fixture")]).is_err(),
                "{path} was admitted"
            );
        }
        let file = manifest("captures/image.png", b"fixture");
        assert!(validate_job_manifest(&[file.clone(), file.clone()]).is_err());
        assert_eq!(
            validate_job_manifest(&vec![file.clone(); MAX_JOB_REVIEW_FILES + 1])
                .unwrap_err()
                .message,
            "ignored_review_file_limit"
        );
        let long = (0..100)
            .map(|index| manifest(&format!("{index}/{}", "x".repeat(450)), b"fixture"))
            .collect::<Vec<_>>();
        assert_eq!(
            validate_job_manifest(&long).unwrap_err().message,
            "ignored_review_manifest_limit"
        );
        check_listing(std::slice::from_ref(&file), b"captures/image.png\0").unwrap();
        for listed in [
            b"captures/image.png".as_slice(),
            b"captures/image.png\0new.png\0",
            b"\xff\0",
            b"",
        ] {
            assert!(check_listing(std::slice::from_ref(&file), listed).is_err());
        }
    }

    #[test]
    fn job_review_refuses_oversize_before_reading_and_checks_snapshot_changes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("large.bin");
        stdfs::File::create(&path)
            .unwrap()
            .set_len(MAX_JOB_REVIEW_BYTES + 1)
            .unwrap();
        let mut cursor =
            JobReviewCursor::new(root.path(), vec![manifest("large.bin", b"fixture")]).unwrap();
        assert_eq!(
            cursor.step(DiscoveryBudget::default()).unwrap_err().message,
            "ignored_review_size_limit"
        );
        stdfs::write(&path, b"original").unwrap();
        let mut cursor =
            JobReviewCursor::new(root.path(), vec![manifest("large.bin", b"original")]).unwrap();
        complete(&mut cursor).unwrap();
        let review = cursor.finish(fs::identity(root.path()).unwrap(), "source".into(), 123);
        let old_modified = stdfs::metadata(&path).unwrap().modified().unwrap();
        // Filesystem timestamps may share a clock tick. Observe a real ctime
        // change rather than assuming that every immediate write advances it.
        // The separate cleanup-worker test requires SHA verification even
        // when all supplied metadata matches changed content.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            stdfs::write(&path, b"modified").unwrap();
            stdfs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(stdfs::FileTimes::new().set_modified(old_modified))
                .unwrap();
            if validate_snapshot(root.path(), &review.files[0]).is_err() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fixture ctime never advanced"
            );
            std::thread::yield_now();
        }
        assert_eq!(
            validate_snapshot(root.path(), &review.files[0])
                .unwrap_err()
                .message,
            "ignored_review_file_changed"
        );
    }
}
