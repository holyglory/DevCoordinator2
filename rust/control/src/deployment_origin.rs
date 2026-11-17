//! The actual source of an explicit candidate generation, independent of the
//! deployment's declared worktree, ownership, data and route identities.
use std::path::PathBuf;

use devcoordinator2_api::{ErrorCode, ProtocolError};

use crate::deployment_state::GenerationSourceOrigin;

pub(crate) fn resolve(
    registry: &crate::repository::Registry,
    origin: &GenerationSourceOrigin,
    repository_id: &str,
    uid: u32,
    gid: u32,
) -> Result<PathBuf, ProtocolError> {
    resolve_origin(registry, origin, repository_id, uid, gid, false)
}

pub(crate) fn resolve_for_execution(
    registry: &crate::repository::Registry,
    origin: &GenerationSourceOrigin,
    repository_id: &str,
    uid: u32,
    gid: u32,
) -> Result<PathBuf, ProtocolError> {
    resolve_origin(registry, origin, repository_id, uid, gid, true)
}

fn resolve_origin(
    registry: &crate::repository::Registry,
    origin: &GenerationSourceOrigin,
    repository_id: &str,
    uid: u32,
    gid: u32,
    pin: bool,
) -> Result<PathBuf, ProtocolError> {
    let invalid = || {
        ProtocolError::new(
            ErrorCode::DeploymentApplyFailed,
            "recorded candidate source origin is unavailable or no longer matches its repository/worktree identity",
        )
    };
    let current = registry
        .current_origin_path(repository_id, &origin.worktree_id, &origin.path)
        .map_err(|_| invalid())?;
    let resolved =
        crate::repository::resolve_worktree(&current, Some((uid, gid))).map_err(|_| invalid())?;
    let identity = if pin {
        registry.identity_for_info(&resolved)
    } else {
        registry.observed_identity_for_info(&resolved)
    };
    let (registered_repository_id, registered_worktree_id) = identity.map_err(|_| invalid())?;
    if resolved.worktree_root != current
        || registered_repository_id != repository_id
        || registered_worktree_id != origin.worktree_id
    {
        return Err(invalid());
    }
    Ok(resolved.worktree_root)
}
