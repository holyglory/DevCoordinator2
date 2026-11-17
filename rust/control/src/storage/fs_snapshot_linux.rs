//! Content-bound checkout review using the same nofollow directory handles as
//! removal. File bytes are hashed locally and never become plan content.
use super::*;
use crate::storage::fs;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq)]
pub struct CheckoutSnapshot {
    pub fingerprint: String,
    pub protected: bool,
    pub nested_git: bool,
    pub measurement: super::Measurement,
}

/// A linked worktree's index lives outside its working directory. Bind its
/// complete contents (including staged blob IDs and split-index linkage), and
/// reject either replacement or mutation while it is being read.
pub fn checkout_index_fingerprint(path: &Path) -> Result<Option<String>, ProtocolError> {
    let mut cursor = CheckoutIndexCursor::new(path)?;
    loop {
        if let Some(result) = cursor.step(256 * 1024, std::time::Duration::from_millis(25))? {
            return Ok(result);
        }
    }
}

pub struct CheckoutIndexCursor {
    parent: File,
    name: OsString,
    file: Option<File>,
    before: Option<std::fs::Metadata>,
    digest: Sha256,
    bytes: u64,
    reflog: Option<ReflogRoots>,
}
#[derive(Default)]
struct ReflogRoots {
    prefix: Vec<u8>,
    body: bool,
    roots: std::collections::BTreeSet<String>,
}
impl ReflogRoots {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        for byte in bytes {
            if *byte == b'\n' {
                if !self.body {
                    return Err(blocked("checkout_reflogs_unverified"));
                }
                self.body = false;
                self.prefix.clear();
            } else if !self.body {
                self.prefix.push(*byte);
                if self.prefix.len() > 130 {
                    return Err(blocked("checkout_reflogs_unverified"));
                }
                if self.prefix.iter().filter(|byte| **byte == b' ').count() == 2 {
                    let fields = self.prefix.split(|byte| *byte == b' ').collect::<Vec<_>>();
                    for oid in fields.into_iter().take(2) {
                        if !matches!(oid.len(), 40 | 64) || !oid.iter().all(u8::is_ascii_hexdigit) {
                            return Err(blocked("checkout_reflogs_unverified"));
                        }
                        if !oid.iter().all(|byte| *byte == b'0') {
                            self.roots.insert(
                                std::str::from_utf8(oid)
                                    .map_err(|_| blocked("checkout_reflogs_unverified"))?
                                    .to_owned(),
                            );
                            if self.roots.len() > 16_384 {
                                return Err(blocked(
                                    "checkout_reflog_history_exceeds_review_bound",
                                ));
                            }
                        }
                    }
                    self.body = true;
                    self.prefix.clear();
                }
            }
        }
        Ok(())
    }
    fn finish(&self) -> Result<(), ProtocolError> {
        if self.body || !self.prefix.is_empty() {
            return Err(blocked("checkout_reflogs_unverified"));
        }
        Ok(())
    }
}
impl CheckoutIndexCursor {
    pub fn new_reflog(path: &Path) -> Result<Self, ProtocolError> {
        let mut cursor = Self::new(path)?;
        cursor.reflog = Some(ReflogRoots::default());
        Ok(cursor)
    }
    pub fn reflog_roots(&self) -> Vec<String> {
        self.reflog
            .as_ref()
            .map(|log| log.roots.iter().cloned().collect())
            .unwrap_or_default()
    }
    pub fn new(path: &Path) -> Result<Self, ProtocolError> {
        let parent = open_directory(
            path.parent()
                .ok_or_else(|| blocked("checkout_index_unverified"))?,
        )?;
        let name = path
            .file_name()
            .ok_or_else(|| blocked("checkout_index_unverified"))?;
        let opened = unix::openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        );
        let file = match opened {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => {
                return Ok(Self {
                    parent,
                    name: name.to_owned(),
                    file: None,
                    before: None,
                    digest: Sha256::new(),
                    bytes: 0,
                    reflog: None,
                });
            }
            Err(_) => return Err(blocked("checkout_index_unverified")),
        };
        let before = file
            .metadata()
            .map_err(|_| blocked("checkout_index_unverified"))?;
        if !before.is_file() {
            return Err(blocked("checkout_index_unverified"));
        }
        let mut digest = Sha256::new();
        digest.update(before.dev().to_be_bytes());
        digest.update(before.ino().to_be_bytes());
        digest.update(before.mode().to_be_bytes());
        digest.update(before.len().to_be_bytes());
        digest.update(before.mtime().to_be_bytes());
        digest.update(before.mtime_nsec().to_be_bytes());
        digest.update(before.ctime().to_be_bytes());
        digest.update(before.ctime_nsec().to_be_bytes());
        Ok(Self {
            parent,
            name: name.to_owned(),
            file: Some(file),
            before: Some(before),
            digest,
            bytes: 0,
            reflog: None,
        })
    }

    pub fn bytes_hashed(&self) -> u64 {
        self.bytes
    }

    pub fn step(
        &mut self,
        max_bytes: usize,
        elapsed: std::time::Duration,
    ) -> Result<Option<Option<String>>, ProtocolError> {
        let Some(before) = self.before.as_ref() else {
            match unix::statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW) {
                Err(rustix::io::Errno::NOENT) => return Ok(Some(None)),
                _ => return Err(blocked("checkout_index_changed")),
            }
        };
        let started = std::time::Instant::now();
        let mut bytes = [0; 64 * 1024];
        let mut read = 0;
        while read < max_bytes && started.elapsed() < elapsed {
            let limit = bytes.len().min(max_bytes - read);
            let count = self
                .file
                .as_mut()
                .expect("opened checkout index")
                .read(&mut bytes[..limit])
                .map_err(|_| blocked("checkout_index_unverified"))?;
            if count == 0 {
                if let Some(reflog) = &self.reflog {
                    reflog.finish()?;
                }
                let after = unix::statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|_| blocked("checkout_index_changed"))?;
                if !same_file(before, &after) {
                    return Err(blocked("checkout_index_changed"));
                }
                return Ok(Some(Some(crate::storage::hex(
                    &self.digest.clone().finalize(),
                ))));
            }
            self.digest.update(&bytes[..count]);
            if let Some(reflog) = &mut self.reflog {
                reflog.feed(&bytes[..count])?;
            }
            self.bytes = self.bytes.saturating_add(count as u64);
            read += count;
        }
        Ok(None)
    }
}

struct SnapshotFrame {
    directory: File,
    relative: PathBuf,
    entries: Vec<OsString>,
    next: usize,
}
struct SnapshotFile {
    file: File,
    parent: File,
    name: OsString,
    before: std::fs::Metadata,
}

/// Resumable content proof: read a bounded number of bytes, then make a
/// separate metadata pass so writes to earlier files during a large read do
/// not escape the proof. No source bytes are retained.
pub struct CheckoutSnapshotCursor {
    path: PathBuf,
    root_identity: (u64, u64),
    frames: Vec<SnapshotFrame>,
    file: Option<SnapshotFile>,
    content: Sha256,
    metadata: Sha256,
    expected_metadata: Option<Vec<u8>>,
    protected: bool,
    nested_git: bool,
    entries: u64,
    bytes: u64,
    work: u64,
    phase_entries: usize,
    measurement: super::Measurement,
    seen: HashSet<(u64, u64)>,
    completed: bool,
}
impl CheckoutSnapshotCursor {
    pub fn new(path: &Path) -> Result<Self, ProtocolError> {
        let root = open_directory(path)?;
        let metadata = root
            .metadata()
            .map_err(|_| unavailable("directory_metadata_unavailable"))?;
        let root_identity = (metadata.dev(), metadata.ino());
        let mut content = Sha256::new();
        content.update(root_identity.0.to_be_bytes());
        content.update(root_identity.1.to_be_bytes());
        let mut proof = Sha256::new();
        proof.update(root_identity.0.to_be_bytes());
        proof.update(root_identity.1.to_be_bytes());
        let entries = sorted_names(&root)?;
        Ok(Self {
            path: path.to_path_buf(),
            root_identity,
            frames: vec![SnapshotFrame {
                directory: root,
                relative: PathBuf::new(),
                entries,
                next: 0,
            }],
            file: None,
            content,
            metadata: proof,
            expected_metadata: None,
            protected: false,
            nested_git: false,
            entries: 0,
            bytes: 0,
            work: 0,
            phase_entries: 0,
            measurement: super::Measurement {
                device: root_identity.0,
                inode: root_identity.1,
                bytes: metadata.blocks().saturating_mul(512),
                newest_modified_ns: i128::from(metadata.mtime()) * 1_000_000_000
                    + i128::from(metadata.mtime_nsec()),
                entries: 0,
                nested_git: false,
                protected_metadata: false,
                multiply_linked: false,
            },
            seen: HashSet::from([root_identity]),
            completed: false,
        })
    }
    pub fn progress(&self) -> (u64, u64, u64) {
        (self.entries, self.bytes, self.work)
    }
    pub fn step(
        &mut self,
        max_entries: usize,
        max_bytes: usize,
        elapsed: std::time::Duration,
    ) -> Result<Option<CheckoutSnapshot>, ProtocolError> {
        if self.completed {
            return Err(blocked("checkout_snapshot_already_complete"));
        }
        let started = std::time::Instant::now();
        let mut entries = 0;
        let mut bytes = 0;
        while entries < max_entries && bytes < max_bytes && started.elapsed() < elapsed {
            if let Some(pending) = &mut self.file {
                let mut buffer = [0; 64 * 1024];
                let limit = buffer.len().min(max_bytes - bytes);
                if limit == 0 {
                    break;
                }
                let count = pending
                    .file
                    .read(&mut buffer[..limit])
                    .map_err(|_| unavailable("checkout_file_unreadable"))?;
                if count > 0 {
                    self.content.update(&buffer[..count]);
                    self.bytes = self.bytes.saturating_add(count as u64);
                    bytes += count;
                    continue;
                }
                let after = unix::statat(&pending.parent, &pending.name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|_| blocked("checkout_contents_changed"))?;
                if !same_file(&pending.before, &after) {
                    return Err(blocked("checkout_contents_changed"));
                }
                self.file = None;
                continue;
            }
            let frame_depth = self.frames.len();
            let Some(frame) = self.frames.last_mut() else {
                if fs::identity(&self.path)? != self.root_identity {
                    return Err(blocked("identity_changed"));
                }
                if let Some(expected) = &self.expected_metadata {
                    if self.metadata.clone().finalize().as_slice() != expected {
                        return Err(blocked("checkout_contents_changed"));
                    }
                    self.completed = true;
                    self.measurement.entries = self.entries as usize;
                    self.measurement.nested_git = self.nested_git;
                    self.measurement.protected_metadata = self.protected;
                    return Ok(Some(CheckoutSnapshot {
                        fingerprint: crate::storage::hex(&self.content.clone().finalize()),
                        protected: self.protected,
                        nested_git: self.nested_git,
                        measurement: self.measurement.clone(),
                    }));
                }
                self.expected_metadata = Some(self.metadata.clone().finalize().to_vec());
                self.metadata = Sha256::new();
                self.metadata.update(self.root_identity.0.to_be_bytes());
                self.metadata.update(self.root_identity.1.to_be_bytes());
                self.phase_entries = 0;
                let root = open_directory(&self.path)?;
                let names = sorted_names(&root)?;
                self.frames.push(SnapshotFrame {
                    directory: root,
                    relative: PathBuf::new(),
                    entries: names,
                    next: 0,
                });
                continue;
            };
            if frame.next >= frame.entries.len() {
                if sorted_names(&frame.directory)? != frame.entries {
                    return Err(blocked("checkout_contents_changed"));
                }
                self.frames.pop();
                continue;
            }
            let name = frame.entries[frame.next].clone();
            frame.next += 1;
            let relative = frame.relative.join(&name);
            let stat = unix::statat(&frame.directory, &name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|_| blocked("checkout_contents_changed"))?;
            if stat.st_dev != self.root_identity.0 {
                return Err(blocked("unexpected_filesystem"));
            }
            self.phase_entries += 1;
            entries += 1;
            self.work += 1;
            if self.phase_entries > MAX_ENTRIES {
                return Err(blocked("directory_entry_limit"));
            }
            let content_phase = self.expected_metadata.is_none();
            proof_metadata(&mut self.metadata, &relative, &stat);
            if content_phase {
                self.entries += 1;
                proof_metadata(&mut self.content, &relative, &stat);
                self.measurement.newest_modified_ns = self.measurement.newest_modified_ns.max(
                    i128::from(stat.st_mtime) * 1_000_000_000 + i128::from(stat.st_mtime_nsec),
                );
                if self.seen.insert((stat.st_dev, stat.st_ino)) {
                    self.measurement.bytes = self
                        .measurement
                        .bytes
                        .checked_add(
                            u64::try_from(stat.st_blocks)
                                .map_err(|_| unavailable("invalid_allocated_size"))?
                                .saturating_mul(512),
                        )
                        .ok_or_else(|| unavailable("invalid_allocated_size"))?;
                }
                self.measurement.multiply_linked |= unix::FileType::from_raw_mode(stat.st_mode)
                    == unix::FileType::RegularFile
                    && stat.st_nlink > 1;
            }
            let kind = unix::FileType::from_raw_mode(stat.st_mode);
            let protected = super::super::protected_metadata_name(&name)
                || super::super::checkout_entry_protected(
                    &relative,
                    kind == unix::FileType::Directory,
                );
            if protected {
                self.protected = true;
                continue;
            }
            if name == ".git" && !frame.relative.as_os_str().is_empty() {
                self.nested_git = true;
            }
            match kind {
                unix::FileType::Directory => {
                    if frame_depth > MAX_DEPTH {
                        return Err(blocked("directory_depth_limit"));
                    }
                    let directory = File::from(
                        unix::openat2(
                            &frame.directory,
                            &name,
                            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                            unix::ResolveFlags::BENEATH
                                | unix::ResolveFlags::NO_SYMLINKS
                                | unix::ResolveFlags::NO_XDEV,
                        )
                        .map_err(|_| blocked("checkout_contents_changed"))?,
                    );
                    let actual = directory
                        .metadata()
                        .map_err(|_| unavailable("directory_metadata_unavailable"))?;
                    if (actual.dev(), actual.ino()) != (stat.st_dev, stat.st_ino) {
                        return Err(blocked("identity_changed"));
                    }
                    let names = sorted_names(&directory)?;
                    self.frames.push(SnapshotFrame {
                        directory,
                        relative,
                        entries: names,
                        next: 0,
                    });
                }
                unix::FileType::RegularFile if content_phase => {
                    let file = File::from(
                        unix::openat(
                            &frame.directory,
                            &name,
                            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                            Mode::empty(),
                        )
                        .map_err(|_| blocked("checkout_contents_changed"))?,
                    );
                    let before = file
                        .metadata()
                        .map_err(|_| unavailable("file_metadata_unavailable"))?;
                    if !before.is_file() || !same_file(&before, &stat) {
                        return Err(blocked("checkout_file_identity_changed"));
                    }
                    let parent = frame
                        .directory
                        .try_clone()
                        .map_err(|_| unavailable("directory_metadata_unavailable"))?;
                    self.file = Some(SnapshotFile {
                        file,
                        parent,
                        name,
                        before,
                    });
                }
                unix::FileType::RegularFile => {}
                unix::FileType::Symlink => {
                    let target = unix::readlinkat(&frame.directory, &name, Vec::new())
                        .map_err(|_| blocked("checkout_contents_changed"))?;
                    self.metadata.update(target.as_bytes());
                    if content_phase {
                        self.content.update(target.as_bytes());
                    }
                }
                _ => return Err(blocked("checkout_special_file")),
            }
        }
        Ok(None)
    }
}
fn sorted_names(directory: &File) -> Result<Vec<OsString>, ProtocolError> {
    let mut found = names(directory)?;
    found.sort();
    Ok(found)
}
fn proof_metadata(digest: &mut Sha256, relative: &Path, stat: &unix::Stat) {
    use std::os::unix::ffi::OsStrExt;
    let name = relative.as_os_str().as_bytes();
    digest.update((name.len() as u64).to_be_bytes());
    digest.update(name);
    digest.update(stat.st_ino.to_be_bytes());
    digest.update(stat.st_mode.to_be_bytes());
    digest.update(stat.st_size.to_be_bytes());
    digest.update(stat.st_mtime.to_be_bytes());
    digest.update(stat.st_mtime_nsec.to_be_bytes());
    digest.update(stat.st_ctime.to_be_bytes());
    digest.update(stat.st_ctime_nsec.to_be_bytes());
}
fn same_file(before: &std::fs::Metadata, after: &unix::Stat) -> bool {
    (
        before.dev(),
        before.ino(),
        before.mode(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) == (
        after.st_dev,
        after.st_ino,
        after.st_mode,
        after.st_size as u64,
        after.st_mtime,
        after.st_mtime_nsec as i64,
        after.st_ctime,
        after.st_ctime_nsec as i64,
    )
}
pub fn checkout_snapshot(path: &Path) -> Result<CheckoutSnapshot, ProtocolError> {
    let mut cursor = CheckoutSnapshotCursor::new(path)?;
    loop {
        if let Some(result) =
            cursor.step(256, 4 * 1024 * 1024, std::time::Duration::from_millis(25))?
        {
            return Ok(result);
        }
    }
}
