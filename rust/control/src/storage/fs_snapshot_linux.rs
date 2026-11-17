//! Content-bound checkout review using the same nofollow directory handles as
//! removal. File bytes are hashed locally and never become plan content.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq)]
pub struct CheckoutSnapshot {
    pub fingerprint: String,
    pub protected: bool,
    pub nested_git: bool,
}

pub fn checkout_snapshot(path: &Path) -> Result<CheckoutSnapshot, ProtocolError> {
    let root = open_directory(path)?;
    let metadata = root
        .metadata()
        .map_err(|_| unavailable("directory_metadata_unavailable"))?;
    let mut result = CheckoutSnapshot {
        fingerprint: String::new(),
        protected: false,
        nested_git: false,
    };
    let mut digest = Sha256::new();
    digest.update(metadata.dev().to_be_bytes());
    digest.update(metadata.ino().to_be_bytes());
    let mut count = 0;
    snapshot_directory(
        &root,
        Path::new(""),
        metadata.dev(),
        &mut digest,
        &mut count,
        &mut result,
        0,
    )?;
    result.fingerprint = crate::storage::hex(&digest.finalize());
    Ok(result)
}

fn snapshot_directory(
    directory: &File,
    relative: &Path,
    device: u64,
    digest: &mut Sha256,
    count: &mut usize,
    result: &mut CheckoutSnapshot,
    depth: usize,
) -> Result<(), ProtocolError> {
    if depth > MAX_DEPTH {
        return Err(blocked("directory_depth_limit"));
    }
    use std::os::unix::ffi::OsStrExt;
    let mut entries = names(directory)?;
    entries.sort();
    for name in &entries {
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err(blocked("directory_entry_limit"));
        }
        let child_relative = relative.join(name);
        let encoded = child_relative.as_os_str().as_bytes();
        digest.update((encoded.len() as u64).to_be_bytes());
        digest.update(encoded);
        let before = unix::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| blocked("checkout_contents_changed"))?;
        if before.st_dev != device {
            return Err(blocked("unexpected_filesystem"));
        }
        digest.update(before.st_ino.to_be_bytes());
        digest.update(before.st_mode.to_be_bytes());
        digest.update(before.st_size.to_be_bytes());
        digest.update(before.st_mtime.to_be_bytes());
        digest.update(before.st_mtime_nsec.to_be_bytes());
        digest.update(before.st_ctime.to_be_bytes());
        digest.update(before.st_ctime_nsec.to_be_bytes());
        let protected_name = super::super::protected_metadata_name(name)
            || super::super::checkout_entry_protected(
                &child_relative,
                unix::FileType::from_raw_mode(before.st_mode) == unix::FileType::Directory,
            );
        if protected_name {
            result.protected = true;
            // A protected scope cannot be approved for this operation. Its
            // contents need not be opened, hashed or disclosed to explain the
            // blocker; its exact metadata remains bound to this review.
            continue;
        }
        if name == ".git" && !relative.as_os_str().is_empty() {
            result.nested_git = true;
        }
        match unix::FileType::from_raw_mode(before.st_mode) {
            unix::FileType::Directory => {
                let child = File::from(
                    unix::openat2(
                        directory,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                        unix::ResolveFlags::BENEATH
                            | unix::ResolveFlags::NO_SYMLINKS
                            | unix::ResolveFlags::NO_XDEV,
                    )
                    .map_err(|_| blocked("checkout_contents_changed"))?,
                );
                let metadata = child
                    .metadata()
                    .map_err(|_| unavailable("directory_metadata_unavailable"))?;
                if (metadata.dev(), metadata.ino()) != (before.st_dev, before.st_ino) {
                    return Err(blocked("identity_changed"));
                }
                snapshot_directory(
                    &child,
                    &child_relative,
                    device,
                    digest,
                    count,
                    result,
                    depth + 1,
                )?;
            }
            unix::FileType::RegularFile => {
                let mut file = File::from(
                    unix::openat(
                        directory,
                        name,
                        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                        Mode::empty(),
                    )
                    .map_err(|_| blocked("checkout_contents_changed"))?,
                );
                let metadata = file
                    .metadata()
                    .map_err(|_| unavailable("file_metadata_unavailable"))?;
                if (metadata.dev(), metadata.ino()) != (before.st_dev, before.st_ino)
                    || !metadata.is_file()
                {
                    return Err(blocked("checkout_file_identity_changed"));
                }
                let mut bytes = [0; 64 * 1024];
                loop {
                    let length = file
                        .read(&mut bytes)
                        .map_err(|_| unavailable("checkout_file_unreadable"))?;
                    if length == 0 {
                        break;
                    }
                    digest.update(&bytes[..length]);
                }
                let after = file
                    .metadata()
                    .map_err(|_| unavailable("file_metadata_unavailable"))?;
                if (
                    metadata.len(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                ) != (
                    after.len(),
                    after.mtime(),
                    after.mtime_nsec(),
                    after.ctime(),
                    after.ctime_nsec(),
                ) {
                    return Err(blocked("checkout_contents_changed"));
                }
            }
            unix::FileType::Symlink => {
                let target = unix::readlinkat(directory, name, Vec::new())
                    .map_err(|_| blocked("checkout_contents_changed"))?;
                digest.update(target.as_bytes());
            }
            _ => return Err(blocked("checkout_special_file")),
        }
        let after = unix::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| blocked("checkout_contents_changed"))?;
        if (
            before.st_dev,
            before.st_ino,
            before.st_mode,
            before.st_size,
            before.st_mtime,
            before.st_mtime_nsec,
            before.st_ctime,
            before.st_ctime_nsec,
        ) != (
            after.st_dev,
            after.st_ino,
            after.st_mode,
            after.st_size,
            after.st_mtime,
            after.st_mtime_nsec,
            after.st_ctime,
            after.st_ctime_nsec,
        ) {
            return Err(blocked("checkout_contents_changed"));
        }
    }
    let mut after = names(directory)?;
    after.sort();
    if after != entries {
        return Err(blocked("checkout_contents_changed"));
    }
    Ok(())
}
