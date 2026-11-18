//! Filesystem cleanup is Linux-native; other clients keep the typed interface.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
    pub newest_modified_ns: i128,
    pub entries: usize,
    pub nested_git: bool,
    pub protected_metadata: bool,
    pub multiply_linked: bool,
}

pub(crate) fn protected_metadata_name(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            ".ssh"
                | ".aws"
                | ".gnupg"
                | "sessions"
                | "archived_sessions"
                | ".env"
                | ".netrc"
                | ".npmrc"
                | "auth.json"
                | "credentials.json"
                | "id_rsa"
                | "id_ed25519"
        )
    )
}

/// Reuse the existing ignored-file review scope for checkout removal. Parent
/// directories of allowed metadata remain traversable; retained logs, chat
/// histories and other data are protected. The root Git store is separately
/// reviewed through every ref/stash before an independent clone is removed.
pub(crate) fn checkout_entry_protected(relative: &std::path::Path, is_directory: bool) -> bool {
    use std::path::Component;
    let components = relative.components().collect::<Vec<_>>();
    if components
        .first()
        .is_some_and(|part| matches!(part,Component::Normal(name) if *name==".git"))
    {
        return false;
    }
    let metadata = components
        .iter()
        .position(|part| matches!(part,Component::Normal(name) if *name==".devcoordinator"));
    let scoped = metadata
        .map(|index| components[index..].iter().collect::<std::path::PathBuf>())
        .unwrap_or_else(|| relative.to_path_buf());
    if is_directory
        && matches!(
            scoped.to_str(),
            Some(".devcoordinator" | ".devcoordinator/test" | ".devcoordinator/test/current")
        )
    {
        return false;
    }
    !super::native::HostBackend::ignored_review_path_allowed(&scoped)
}

#[cfg(target_os = "linux")]
#[path = "fs_linux.rs"]
mod platform;
#[cfg(not(target_os = "linux"))]
#[path = "fs_unavailable.rs"]
mod platform;
pub use platform::*;
