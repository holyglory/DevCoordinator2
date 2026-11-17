//! Permanent registered identities and explicit, inode-preserving relocation.
//! Paths locate a current source; neither historical paths nor Git config grant
//! authority. The external operator moves/repairs Git between prepare and verify.

use super::*;
use crate::access::Caller;
use devcoordinator2_api::project::relocation as api;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct NativeIdentity {
    repository_root: String,
    common: String,
    worktree_root: String,
    git: String,
    git_relative: String,
}

fn blocked(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::ConfigurationConflict, message)
}

fn db_error(error: DatabaseError) -> ProtocolError {
    match error {
        DatabaseError::Domain(error) => error,
        other => super::database_error(other),
    }
}

fn directory_identity(path: &Path) -> Result<String, ProtocolError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| blocked("registered source is unavailable"))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || path.canonicalize().ok().as_deref() != Some(path)
    {
        return Err(blocked(
            "source identity requires an exact physical directory without symlinks",
        ));
    }
    Ok(format!("{:x}:{:x}", metadata.dev(), metadata.ino()))
}

fn native_identity(info: &WorktreeInfo) -> Result<NativeIdentity, ProtocolError> {
    let repository_root = directory_identity(&info.repository_root)?;
    let common_path = info.repository_root.join(".git");
    let common = directory_identity(&common_path)?;
    let worktree_root = directory_identity(&info.worktree_root)?;
    let marker = info.worktree_root.join(".git");
    let git_path = if info.repository_root == info.worktree_root {
        common_path.clone()
    } else {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&marker)
            .map_err(|_| blocked("linked worktree Git identity cannot be verified"))?;
        if !file
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 4096)
        {
            return Err(blocked("linked worktree Git marker is invalid"));
        }
        let mut value = String::new();
        file.take(4097)
            .read_to_string(&mut value)
            .map_err(|_| blocked("linked worktree Git marker is invalid"))?;
        if value.len() > 4096 {
            return Err(blocked("linked worktree Git marker exceeds its bound"));
        }
        let relative = value
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or_else(|| blocked("linked worktree Git marker is invalid"))?;
        let git = info
            .worktree_root
            .join(relative)
            .canonicalize()
            .map_err(|_| blocked("linked worktree Git directory is unavailable"))?;
        if !git.starts_with(common_path.join("worktrees")) {
            return Err(blocked(
                "linked worktree does not use its registered Git common directory",
            ));
        }
        git
    };
    let git = directory_identity(&git_path)?;
    let git_relative = git_path
        .strip_prefix(&common_path)
        .map_err(|_| blocked("Git identity escaped its common directory"))?
        .to_string_lossy()
        .into_owned();
    Ok(NativeIdentity {
        repository_root,
        common,
        worktree_root,
        git,
        git_relative,
    })
}

fn pending(connection: &Connection, repository_id: &str) -> Result<bool, DatabaseError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM repository_relocations WHERE repository_id=$1 AND state IN ('prepared','verified'))",[repository_id],|row|row.get(0))?)
}

fn validate_native_bindings(
    connection: &Connection,
    repository_id: &str,
    worktree_id: &str,
    native: &NativeIdentity,
) -> Result<(bool, bool), DatabaseError> {
    let repository = connection.query_row("SELECT root_identity,common_identity FROM repository_native_identities WHERE repository_id=$1",[repository_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?;
    let repository_bound = repository.is_some();
    if let Some((root, common)) = repository {
        if root != native.repository_root || common != native.common {
            return Err(blocked("the registered Git repository was replaced; relocation cannot be inferred from a path").into());
        }
    } else {
        let elsewhere:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM repository_native_identities WHERE root_identity=$1 OR common_identity=$2)",(&native.repository_root,&native.common),|row|row.get(0))?;
        if elsewhere {
            return Err(blocked("this Git repository already has a registered identity; complete its explicit relocation").into());
        }
    }
    let worktree = connection.query_row("SELECT root_identity,git_identity FROM worktree_native_identities WHERE worktree_id=$1",[worktree_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?;
    let worktree_bound = worktree.is_some();
    if let Some((root, git)) = worktree {
        if root != native.worktree_root || git != native.git {
            return Err(blocked(
                "the registered checkout was replaced; its old path cannot grant its identity",
            )
            .into());
        }
    } else {
        let elsewhere:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM worktree_native_identities WHERE root_identity=$1 OR git_identity=$2)",(&native.worktree_root,&native.git),|row|row.get(0))?;
        if elsewhere {
            return Err(blocked(
                "this checkout already has an identity; complete its explicit relocation",
            )
            .into());
        }
    }
    Ok((repository_bound, worktree_bound))
}

fn bind_native(
    connection: &Connection,
    repository_id: &str,
    worktree_id: &str,
    native: &NativeIdentity,
    now: &str,
) -> Result<(), DatabaseError> {
    let (repository_bound, worktree_bound) =
        validate_native_bindings(connection, repository_id, worktree_id, native)?;
    if !repository_bound {
        connection.execute("INSERT INTO repository_native_identities(repository_id,root_identity,common_identity,bound_at) VALUES($1,$2,$3,$4)",(repository_id,&native.repository_root,&native.common,now))?;
    }
    if !worktree_bound {
        connection.execute("INSERT INTO worktree_native_identities(worktree_id,root_identity,git_identity,bound_at) VALUES($1,$2,$3,$4)",(worktree_id,&native.worktree_root,&native.git,now))?;
    }
    Ok(())
}

fn registration_for_info(
    connection: &Connection,
    info: &WorktreeInfo,
    native: &NativeIdentity,
    write: Option<(u32, &str)>,
) -> Result<RegisteredRepository, DatabaseError> {
    let root_path = path_text(&info.repository_root).map_err(DatabaseError::Domain)?;
    let worktree_path = path_text(&info.worktree_root).map_err(DatabaseError::Domain)?;
    let allocated_repository_id = ids::repository_id(&info.repository_root)
        .map_err(id_error)
        .map_err(DatabaseError::Domain)?;
    let allocated_worktree_id = ids::worktree_id(&info.worktree_root)
        .map_err(id_error)
        .map_err(DatabaseError::Domain)?;
    let display_name = info
        .repository_root
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_owned();
    let reserved:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM repository_relocations WHERE state IN ('prepared','verified') AND (new_path=$1 OR new_path=$2 OR strpos($2,new_path||'/')=1))",(&root_path,&worktree_path),|row|row.get(0))?;
    if reserved {
        return Err(blocked("relocation destination is reserved; verify and commit its original identity before registration").into());
    }
    let aliases: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM registry_path_aliases WHERE old_path=$1 OR old_path=$2)",
        (&root_path, &worktree_path),
        |row| row.get(0),
    )?;
    if aliases {
        return Err(
            blocked("a historical source path cannot register a replacement checkout").into(),
        );
    }
    let repository = connection
        .query_row(
            "SELECT repository_id,display_name,archived_at FROM repositories WHERE root_path=$1",
            [&root_path],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let (repository_id, display_name, registered) = if let Some((id, name, archived)) = repository {
        if archived.is_some() {
            return Err(ProtocolError::new(
                ErrorCode::RepositoryArchived,
                "repository is archived",
            )
            .into());
        }
        (id, name, false)
    } else {
        let collision:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM repositories WHERE repository_id=$1) OR EXISTS(SELECT 1 FROM repository_native_identities WHERE root_identity=$2 OR common_identity=$3)",(&allocated_repository_id,&native.repository_root,&native.common),|row|row.get(0))?;
        if collision {
            return Err(blocked(
                "registered repository identity requires an explicit relocation commit",
            )
            .into());
        }
        if let Some((uid, now)) = write {
            connection.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES($1,$2,$3,$4,$5,$4)",(&allocated_repository_id,&root_path,&display_name,now,i64::from(uid)))?;
        }
        (allocated_repository_id, display_name, true)
    };
    if pending(connection, &repository_id)? {
        return Err(blocked(
            "source relocation is prepared; finish or cancel it before starting execution",
        )
        .into());
    }
    let worktree = connection
        .query_row(
            "SELECT worktree_id,repository_id FROM worktrees WHERE worktree_path=$1",
            [&worktree_path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let worktree_id = if let Some((id, owner)) = worktree {
        if owner != repository_id {
            return Err(
                blocked("checkout path belongs to a different registered repository").into(),
            );
        }
        id
    } else {
        let collision:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE worktree_id=$1) OR EXISTS(SELECT 1 FROM worktree_native_identities WHERE root_identity=$2 OR git_identity=$3)",(&allocated_worktree_id,&native.worktree_root,&native.git),|row|row.get(0))?;
        if collision {
            return Err(blocked(
                "registered checkout identity requires an explicit relocation commit",
            )
            .into());
        }
        if let Some((_, now)) = write {
            connection.execute("INSERT INTO worktrees(worktree_id,repository_id,worktree_path,registered_at,last_seen_at) VALUES($1,$2,$3,$4,$4)",(&allocated_worktree_id,&repository_id,&worktree_path,now))?;
        }
        allocated_worktree_id
    };
    if let Some((_, now)) = write {
        bind_native(connection, &repository_id, &worktree_id, native, now)?;
        connection.execute(
            "UPDATE repositories SET last_seen_at=$1 WHERE repository_id=$2",
            (now, &repository_id),
        )?;
        connection.execute(
            "UPDATE worktrees SET last_seen_at=$1 WHERE worktree_id=$2",
            (now, &worktree_id),
        )?;
    } else {
        validate_native_bindings(connection, &repository_id, &worktree_id, native)?;
    }
    Ok(RegisteredRepository {
        repository_id,
        worktree_id,
        root_path,
        worktree_path,
        display_name,
        registered,
    })
}

pub(super) fn register(
    registry: &Registry,
    path: &Path,
    uid: u32,
    gid: u32,
) -> Result<RegisteredRepository, ProtocolError> {
    validate_absolute_path(path)?;
    let info = resolve_worktree(path, Some((uid, gid)))?;
    let native = native_identity(&info)?;
    let now = timestamp()?;
    registry
        .database
        .transaction(move |connection| {
            connection.query_row(
                "SELECT pg_advisory_xact_lock(29162844963183719)",
                [],
                |_| Ok(()),
            )?;
            registration_for_info(connection, &info, &native, Some((uid, &now)))
        })
        .map_err(db_error)
}

impl Registry {
    /// Preview desired-source IDs without registering a new checkout. Execution
    /// still registers it, and recorded generation origins remain strict.
    pub(crate) fn preview_source_identity(
        &self,
        info: &WorktreeInfo,
    ) -> Result<(String, String), ProtocolError> {
        let native = native_identity(info)?;
        self.database
            .read(|connection| {
                let candidate = registration_for_info(connection, info, &native, None)?;
                Ok((candidate.repository_id, candidate.worktree_id))
            })
            .map_err(db_error)
    }

    /// Resolve immutable generation provenance through an explicit committed
    /// alias. This returns a locator only; the caller still verifies native Git.
    pub(crate) fn current_origin_path(
        &self,
        repository_id: &str,
        worktree_id: &str,
        recorded: &Path,
    ) -> Result<PathBuf, ProtocolError> {
        let repository_id = repository_id.to_owned();
        let worktree_id = worktree_id.to_owned();
        let recorded = path_text(recorded)?;
        self.database.read(move|connection|{
            let current:Option<String>=connection.query_row("SELECT worktree_path FROM worktrees WHERE repository_id=$1 AND worktree_id=$2",(&repository_id,&worktree_id),|row|row.get(0)).optional()?;
            let current=current.ok_or_else(||DatabaseError::Domain(repository_not_found("recorded source identity is no longer registered")))?;
            if recorded!=current {
                let alias:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM registry_path_aliases WHERE repository_id=$1 AND worktree_id=$2 AND old_path=$3)",(&repository_id,&worktree_id,&recorded),|row|row.get(0))?;
                if !alias {return Err(blocked("recorded source path has no committed relocation binding").into());}
            }
            Ok(PathBuf::from(current))
        }).map_err(db_error)
    }

    /// Resolve opaque IDs from the verified current paths and their native Git
    /// identities. Path hashes allocate new registrations only.
    pub(crate) fn identity_for_info(
        &self,
        info: &WorktreeInfo,
    ) -> Result<(String, String), ProtocolError> {
        let native = native_identity(info)?;
        let root = path_text(&info.repository_root)?;
        let path = path_text(&info.worktree_root)?;
        let now = timestamp()?;
        self.database.transaction(move|connection|{
            let (repository_id,worktree_id)=connection.query_row("SELECT r.repository_id,w.worktree_id FROM repositories r JOIN worktrees w USING(repository_id) WHERE r.root_path=$1 AND w.worktree_path=$2",(&root,&path),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?.ok_or_else(||DatabaseError::Domain(repository_not_found("no current registered checkout matches this physical source")))?;
            if pending(connection,&repository_id)? {return Err(blocked("source relocation is prepared; finish or cancel it before starting execution").into());}
            bind_native(connection,&repository_id,&worktree_id,&native,&now)?;
            Ok((repository_id,worktree_id))
        }).map_err(db_error)
    }

    /// Historical paths support metadata links only. Existing replacement paths
    /// are rejected and this method is never an execution-source resolver.
    pub(crate) fn historical_path(
        &self,
        path: &Path,
    ) -> Result<Option<RegisteredRepository>, ProtocolError> {
        if std::fs::symlink_metadata(path).is_ok() {
            return Ok(None);
        }
        let path = path_text(path)?;
        self.database.read(move|connection|{
            connection.query_row("SELECT r.repository_id,r.root_path,r.display_name,w.worktree_id,w.worktree_path FROM registry_path_aliases a JOIN worktrees w USING(worktree_id) JOIN repositories r ON r.repository_id=w.repository_id WHERE a.old_path=$1",[path],|row|Ok(RegisteredRepository{repository_id:row.get(0)?,root_path:row.get(1)?,display_name:row.get(2)?,worktree_id:row.get(3)?,worktree_path:row.get(4)?,registered:false})).optional().map_err(DatabaseError::from)
        }).map_err(db_error)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct PreparedNative {
    identity: NativeIdentity,
    repository_root: PathBuf,
    owner_uid: u32,
    owner_gid: u32,
    moves_repository_root: bool,
    linked: Vec<(String, PathBuf, NativeIdentity)>,
    #[serde(default)]
    copied: bool,
    #[serde(default = "legacy_fingerprint_version")]
    fingerprint_version: u32,
    #[serde(default)]
    target: Option<VerifiedTarget>,
    #[serde(default)]
    consumers: Vec<RelocationConsumer>,
}

fn legacy_fingerprint_version() -> u32 {
    1
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct VerifiedTarget {
    identity: NativeIdentity,
    linked: Vec<(String, PathBuf, NativeIdentity)>,
}

mod consumers;
use consumers::RelocationConsumer;

#[derive(Clone)]
struct StoredRelocation {
    receipt: api::Receipt,
    native: PreparedNative,
}

fn require_local(caller: &Caller) -> Result<(), ProtocolError> {
    if !caller.is_local() || caller.via_edge {
        return Err(ProtocolError::new(
            ErrorCode::PermissionDenied,
            "checkout relocation requires a trusted local caller",
        ));
    }
    Ok(())
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn load_relocation(connection: &Connection, id: &str) -> Result<StoredRelocation, DatabaseError> {
    let (mut receipt,native):(api::Receipt,String)=connection.query_row("SELECT relocation_id,project_id,repository_id,worktree_id,old_path,new_path,fingerprint,state,checkout_revision,prepared_at,verified_at,committed_at,native_json FROM repository_relocations WHERE relocation_id=$1",[id],|row|Ok((api::Receipt{relocation_id:row.get(0)?,project_id:row.get(1)?,repository_id:row.get(2)?,worktree_id:row.get(3)?,old_path:row.get(4)?,new_path:row.get(5)?,fingerprint:row.get(6)?,method:String::new(),pending_deployments:Vec::new(),state:row.get(7)?,checkout_revision:row.get(8)?,prepared_at:row.get(9)?,verified_at:row.get(10)?,committed_at:row.get(11)?},row.get(12)?))).optional()?.ok_or_else(||DatabaseError::Domain(repository_not_found("relocation plan does not exist")))?;
    let native: PreparedNative = serde_json::from_str(&native)
        .map_err(|_| DatabaseError::Domain(blocked("stored relocation identity is invalid")))?;
    receipt.method = if native.copied {
        "verified_copy"
    } else {
        "rename"
    }
    .into();
    let mut query=connection.prepare("SELECT deployment_id FROM relocation_consumers WHERE relocation_id=$1 AND state<>'rebound' ORDER BY deployment_id")?;
    receipt.pending_deployments = query
        .query_map([id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StoredRelocation { receipt, native })
}

fn existing_request(
    connection: &Connection,
    request: &api::Prepare,
) -> Result<Option<api::Receipt>, DatabaseError> {
    let found:Option<(String,String)>=connection.query_row("SELECT relocation_id,request_json FROM repository_relocations WHERE project_id=$1 AND idempotency_key=$2",(&request.project_id,&request.idempotency_key),|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let Some((id, stored)) = found else {
        return Ok(None);
    };
    let stored: api::Prepare = serde_json::from_str(&stored)
        .map_err(|_| DatabaseError::Domain(blocked("stored relocation request is invalid")))?;
    if stored != *request {
        return Err(
            blocked("relocation idempotency key already names a different exact move").into(),
        );
    }
    Ok(Some(load_relocation(connection, &id)?.receipt))
}

fn inactive(connection: &Connection, repository_id: &str) -> Result<(), DatabaseError> {
    consumers::collect(connection, repository_id)?;
    let project_job:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM project_jobs j JOIN project_repository_memberships m USING(project_id) WHERE m.physical_repository_id=$1 AND j.state IN ('queued','running'))",[repository_id],|row|row.get(0))?;
    if project_job {
        return Err(
            blocked("a project refresh is still queued or running for this repository").into(),
        );
    }
    // Storage admission owns the detailed resource graph. Refuse overlapping
    // owners/ancestors, leases, intents and native bind mounts conservatively.
    let now = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let storage:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM storage_artifacts a WHERE a.repository_id=$1 AND a.removed_at_ms IS NULL AND (EXISTS(SELECT 1 FROM storage_resource_locks l WHERE l.resource_key=a.projection_resource_key OR a.projection_ancestors ? l.resource_key) OR EXISTS(SELECT 1 FROM storage_item_intents i WHERE i.artifact_id=a.artifact_id) OR EXISTS(SELECT 1 FROM dc2_storage_lease_artifacts x WHERE x.artifact_id=a.artifact_id AND x.expires_at_ms>$2)))",(repository_id,now as i64),|row|row.get(0))?;
    if storage {
        return Err(blocked(
            "storage cleanup or an active resource lease still uses this repository",
        )
        .into());
    }
    Ok(())
}

fn verify_git_worktrees(
    root: &Path,
    uid: u32,
    gid: u32,
    expected: &[PathBuf],
) -> Result<(), ProtocolError> {
    let output = run_git_arguments(
        root,
        Some((uid, gid)),
        &["worktree", "list", "--porcelain", "-z"],
        GIT_TIMEOUT,
    )
    .map_err(|_| blocked("complete Git worktree inventory is unavailable"))?;
    if !output.status.success() || expected.len() > 4096 {
        return Err(blocked("complete Git worktree inventory is unavailable"));
    }
    let actual = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|field| field.strip_prefix(b"worktree "))
        .map(|path| PathBuf::from(OsString::from_vec(path.to_vec())))
        .collect::<std::collections::BTreeSet<_>>();
    let expected = expected
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if actual != expected {
        return Err(blocked(
            "Git worktree membership differs from the registered relocation set; register or recover every linked checkout before moving",
        ));
    }
    Ok(())
}

impl Registry {
    pub(crate) fn relocate_prepare(
        &self,
        request: api::Prepare,
        caller: &Caller,
    ) -> Result<api::Receipt, ProtocolError> {
        require_local(caller)?;
        if !safe_identifier(&request.project_id)
            || !safe_identifier(&request.repository_id)
            || !safe_identifier(&request.worktree_id)
            || !safe_identifier(&request.idempotency_key)
            || request.expected_checkout_revision == 0
            || request.expected_checkout_revision >= u32::MAX as u64
        {
            return Err(ProtocolError::new(
                ErrorCode::ParamsInvalid,
                "relocation requires exact IDs, a safe idempotency key and current checkout revision",
            ));
        }
        if let Some(existing) = self
            .database
            .read(|connection| existing_request(connection, &request))
            .map_err(db_error)?
        {
            return Ok(existing);
        }
        let old = PathBuf::from(&request.old_path);
        let new = PathBuf::from(&request.new_path);
        validate_absolute_path(&old)?;
        validate_absolute_path(&new)?;
        directory_identity(&old)?;
        let parent = new
            .parent()
            .ok_or_else(|| blocked("relocation destination has no parent"))?;
        directory_identity(parent)?;
        if new == old
            || new.starts_with(&old)
            || old.starts_with(&new)
            || std::fs::symlink_metadata(&new).is_ok()
            || new.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err(blocked(
                "relocation requires distinct exact paths and an absent destination",
            ));
        }
        let copied = std::fs::metadata(&old)
            .map_err(|_| blocked("source metadata unavailable"))?
            .dev()
            != std::fs::metadata(parent)
                .map_err(|_| blocked("destination metadata unavailable"))?
                .dev();
        if copied {
            return Err(blocked(
                "cross-filesystem relocation is unavailable; preserve both source and copy until a content-pinned recovery workflow is implemented",
            ));
        }
        let (root,uid,revision)=self.database.read(|connection|{
            let row=connection.query_row("SELECT r.root_path,r.registered_by_uid,c.revision FROM project_checkouts c JOIN worktrees w USING(worktree_id) JOIN repositories r ON r.repository_id=w.repository_id WHERE c.project_id=$1 AND c.worktree_id=$2 AND c.physical_repository_id=$3 AND w.repository_id=$3 AND c.absolute_path=$4 AND w.worktree_path=$4 AND r.archived_at IS NULL",(&request.project_id,&request.worktree_id,&request.repository_id,&request.old_path),|row|Ok((row.get::<_,String>(0)?,row.get::<_,u32>(1)?,row.get::<_,u64>(2)?))).optional()?.ok_or_else(||DatabaseError::Domain(blocked("project checkout, physical repository and source path do not match")))?;
            if row.2!=request.expected_checkout_revision {return Err(blocked("checkout changed; refresh its revision before preparing relocation").into());}
            let roots={let mut query=connection.prepare("SELECT absolute_path FROM project_roots WHERE project_id=$1")?;query.query_map([&request.project_id],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?};
            if !roots.iter().any(|root|old.starts_with(Path::new(root)))||!roots.iter().any(|root|new.starts_with(Path::new(root))) {return Err(blocked("both relocation paths must be inside explicitly declared project roots").into());}
            inactive(connection,&request.repository_id)?;
            Ok(row)
        }).map_err(db_error)?;
        if uid == 0 {
            return Err(blocked(
                "repository Git verification requires its registered non-root execution account",
            ));
        }
        let gid = if caller.uid == uid {
            caller.gid
        } else {
            crate::systemd::primary_gid(uid)
                .map_err(|_| blocked("registered execution account is unavailable"))?
        };
        let info = resolve_worktree(&old, Some((uid, gid)))?;
        if info.worktree_root != old || info.repository_root != Path::new(&root) {
            return Err(blocked("registered native Git source changed"));
        }
        let (repository_id, worktree_id) = self.identity_for_info(&info)?;
        if repository_id != request.repository_id || worktree_id != request.worktree_id {
            return Err(blocked("registered IDs do not match the prepared source"));
        }
        if info.repository_root == Path::new("/home/DevCoordinator2") && old == info.repository_root
        {
            return Err(blocked(
                "the installed Coordinator canonical root requires its separate reviewed installer migration",
            ));
        }
        if self
            .additional_archive_blockers
            .first_blocker(&request.repository_id, &old)?
            .is_some()
        {
            return Err(blocked(
                "an active test or process still uses this repository",
            ));
        }
        let identity = native_identity(&info)?;
        let linked=self.database.read(|connection|{let mut query=connection.prepare("SELECT worktree_id,worktree_path FROM worktrees WHERE repository_id=$1 AND worktree_id<>$2 ORDER BY worktree_id")?;Ok(query.query_map((&request.repository_id,&request.worktree_id),|row|Ok((row.get::<_,String>(0)?,PathBuf::from(row.get::<_,String>(1)?))))?.collect::<Result<Vec<_>,_>>()?)}).map_err(db_error)?;
        let mut linked_native = Vec::new();
        for (id, path) in linked {
            if path.starts_with(&old) {
                return Err(blocked(
                    "nested registered checkouts require separate relocation plans",
                ));
            }
            let resolved = resolve_worktree(&path, Some((uid, gid)))?;
            if resolved.repository_root != info.repository_root {
                return Err(blocked("linked worktree membership changed"));
            }
            let verified = self.identity_for_info(&resolved)?;
            if verified != (repository_id.clone(), id.clone()) {
                return Err(blocked("linked worktree identity changed"));
            }
            if self
                .additional_archive_blockers
                .first_blocker(&repository_id, &path)?
                .is_some()
            {
                return Err(blocked("an active test or process uses a linked worktree"));
            }
            linked_native.push((id, path, native_identity(&resolved)?));
        }
        let consumer_snapshots = self
            .database
            .read(|connection| consumers::collect(connection, &repository_id))
            .map_err(db_error)?;
        let mut affected = vec![old.clone()];
        affected.extend(linked_native.iter().map(|(_, path, _)| path.clone()));
        verify_git_worktrees(&info.repository_root, uid, gid, &affected)?;
        affected.push(new.clone());
        crate::storage::ensure_relocation_inactive(&affected)?;
        let native = PreparedNative {
            identity,
            repository_root: info.repository_root.clone(),
            owner_uid: uid,
            owner_gid: gid,
            moves_repository_root: info.repository_root == old,
            linked: linked_native,
            copied,
            fingerprint_version: 2,
            target: None,
            consumers: consumer_snapshots,
        };
        let fingerprint = relocation_fingerprint(
            &old,
            &native.repository_root,
            &native.identity,
            native.fingerprint_version,
        )?;
        let relocation_id = format!("m{}", ids::retirement_receipt_id().map_err(id_error)?);
        let now = timestamp()?;
        self.database.transaction(|connection|{
            connection.query_row("SELECT pg_advisory_xact_lock(29162844963183719)",[],|_|Ok(()))?;
            if let Some(existing)=existing_request(connection,&request)? {return Ok(existing);}
            if pending(connection,&request.repository_id)? {return Err(blocked("another relocation already owns this repository").into());}
            inactive(connection,&request.repository_id)?;
            consumers::assert_unchanged(connection,&request.repository_id,&native.consumers)?;
            let changed=connection.execute("UPDATE project_checkouts SET revision=revision+1,record_json=jsonb_set(record_json,'{revision}',to_jsonb(revision+1)),updated_at=$1,updated_by=$2 WHERE project_id=$3 AND worktree_id=$4 AND absolute_path=$5 AND revision=$6",(&now,caller.actor(),&request.project_id,&request.worktree_id,&request.old_path,revision))?;
            if changed!=1 {return Err(blocked("checkout changed while relocation was being prepared").into());}
            let receipt=api::Receipt{relocation_id:relocation_id.clone(),project_id:request.project_id.clone(),repository_id:request.repository_id.clone(),worktree_id:request.worktree_id.clone(),old_path:request.old_path.clone(),new_path:request.new_path.clone(),fingerprint:fingerprint.clone(),method:if copied {"verified_copy"}else{"rename"}.into(),pending_deployments:native.consumers.iter().map(|consumer|consumer.deployment_id.clone()).collect(),state:"prepared".into(),checkout_revision:revision+1,prepared_at:now.clone(),verified_at:None,committed_at:None};
            connection.execute("INSERT INTO repository_relocations(relocation_id,project_id,repository_id,worktree_id,old_path,new_path,moves_repository_root,idempotency_key,request_json,fingerprint,native_json,state,checkout_revision,actor_uid,prepared_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'prepared',$12,$13,$14)",crate::database::params![relocation_id,request.project_id,request.repository_id,request.worktree_id,request.old_path,request.new_path,native.moves_repository_root,request.idempotency_key,serde_json::to_string(&request).map_err(|_|DatabaseError::Domain(blocked("relocation request cannot be retained")))?,fingerprint,serde_json::to_string(&native).map_err(|_|DatabaseError::Domain(blocked("relocation identity cannot be retained")))?,revision+1,i64::from(caller.uid),now])?;
            Ok(receipt)
        }).map_err(db_error)
    }

    fn relocation_reference(
        &self,
        request: &api::Reference,
        caller: &Caller,
    ) -> Result<StoredRelocation, ProtocolError> {
        require_local(caller)?;
        if !safe_identifier(&request.relocation_id)
            || request.expected_fingerprint.len() != 64
            || !request
                .expected_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ProtocolError::new(
                ErrorCode::ParamsInvalid,
                "relocation reference or expected fingerprint is invalid",
            ));
        }
        let stored = self
            .database
            .read(|connection| load_relocation(connection, &request.relocation_id))
            .map_err(db_error)?;
        if stored.receipt.fingerprint != request.expected_fingerprint {
            return Err(blocked(
                "relocation fingerprint changed; use the exact prepared plan",
            ));
        }
        Ok(stored)
    }

    fn verify_relocation_target(
        &self,
        stored: &StoredRelocation,
    ) -> Result<VerifiedTarget, ProtocolError> {
        let receipt = &stored.receipt;
        let native = &stored.native;
        if receipt.state == "cancelled" {
            return Err(blocked("relocation was cancelled"));
        }
        // Old persisted copy plans must not reach the former destructive path.
        // A root inode and a previous hash do not pin the surviving contents
        // while an interrupted recursive source removal is recovered.
        if native.copied {
            return Err(blocked(
                "cross-filesystem relocation is unavailable; preserve both source and copy until a content-pinned recovery workflow is implemented",
            ));
        }
        let old = Path::new(&receipt.old_path);
        let new = Path::new(&receipt.new_path);
        let mut affected = vec![old.to_owned(), new.to_owned()];
        affected.extend(native.linked.iter().map(|(_, path, _)| path.clone()));
        crate::storage::ensure_relocation_inactive(&affected)?;
        let old_absent = crate::storage::fs::absent(old)?;
        if !old_absent {
            return Err(blocked(
                "the old source path still exists or was replaced; no relocation is committed",
            ));
        }
        let info = resolve_worktree(new, Some((native.owner_uid, native.owner_gid)))?;
        let expected_root = if native.moves_repository_root {
            new.to_owned()
        } else {
            native.repository_root.clone()
        };
        if info.worktree_root != new || info.repository_root != expected_root {
            return Err(blocked(
                "destination Git links have not been repaired to the exact prepared source",
            ));
        }
        let mut expected_worktrees = vec![new.to_owned()];
        expected_worktrees.extend(native.linked.iter().map(|(_, path, _)| path.clone()));
        verify_git_worktrees(
            &expected_root,
            native.owner_uid,
            native.owner_gid,
            &expected_worktrees,
        )?;
        let identity = native_identity(&info)?;
        if identity != native.identity {
            return Err(blocked(
                "relocation destination is not the original physical checkout",
            ));
        }
        let mut linked_targets = Vec::new();
        for (id, path, original) in &native.linked {
            let linked = resolve_worktree(path, Some((native.owner_uid, native.owner_gid)))?;
            let current = native_identity(&linked)?;
            if linked.repository_root != expected_root
                || current.worktree_root != original.worktree_root
                || current.git_relative != original.git_relative
                || current.repository_root != identity.repository_root
                || current.common != identity.common
                || current != *original
            {
                return Err(blocked(
                    "linked worktree Git metadata is unrepaired or was replaced",
                ));
            }
            if self
                .additional_archive_blockers
                .first_blocker(&receipt.repository_id, path)?
                .is_some()
            {
                return Err(blocked("an active test uses a linked checkout"));
            }
            linked_targets.push((id.clone(), path.clone(), current));
        }
        if self
            .additional_archive_blockers
            .first_blocker(&receipt.repository_id, new)?
            .is_some()
        {
            return Err(blocked("an active test uses the relocation destination"));
        }
        if relocation_fingerprint(new, &expected_root, &identity, native.fingerprint_version)?
            != receipt.fingerprint
        {
            return Err(blocked(
                "destination files, dirty work, ignored data, index or Git refs do not match the prepared source",
            ));
        }
        let target = VerifiedTarget {
            identity,
            linked: linked_targets,
        };
        if native
            .target
            .as_ref()
            .is_some_and(|expected| expected != &target)
        {
            return Err(blocked(
                "the verified destination was replaced before relocation commit",
            ));
        }
        self.database
            .read(|connection| {
                inactive(connection, &receipt.repository_id)?;
                consumers::assert_unchanged(connection, &receipt.repository_id, &native.consumers)
            })
            .map_err(db_error)?;
        Ok(target)
    }

    pub(crate) fn relocate_verify(
        &self,
        request: api::Reference,
        caller: &Caller,
    ) -> Result<api::Receipt, ProtocolError> {
        let mut stored = self.relocation_reference(&request, caller)?;
        if stored.receipt.state == "committed" {
            return Ok(stored.receipt);
        }
        let observed = self
            .database
            .read(|connection| consumers::collect(connection, &stored.receipt.repository_id))
            .map_err(db_error)?;
        consumers::assert_supported(&observed, &stored.native.consumers)?;
        let target = self.verify_relocation_target(&stored)?;
        let now = timestamp()?;
        stored.native.target = Some(target);
        self.database.transaction(|connection|{
            inactive(connection,&stored.receipt.repository_id)?;
            consumers::assert_unchanged(connection,&stored.receipt.repository_id,&stored.native.consumers)?;
            connection.execute("UPDATE repository_relocations SET state='verified',verified_at=$1,native_json=$3 WHERE relocation_id=$2 AND state IN ('prepared','verified')",(&now,&request.relocation_id,encode_native(&stored.native)?))?;
            Ok(load_relocation(connection,&request.relocation_id)?.receipt)
        }).map_err(db_error)
    }

    pub(crate) fn relocate_commit(
        &self,
        request: api::Reference,
        caller: &Caller,
    ) -> Result<api::Receipt, ProtocolError> {
        let mut stored = self.relocation_reference(&request, caller)?;
        if stored.receipt.state == "committed" {
            return Ok(stored.receipt);
        }
        let observed = self
            .database
            .read(|connection| consumers::collect(connection, &stored.receipt.repository_id))
            .map_err(db_error)?;
        consumers::assert_supported(&observed, &stored.native.consumers)?;
        let target = self.verify_relocation_target(&stored)?;
        stored.native.target = Some(target.clone());
        let now = timestamp()?;
        if !crate::storage::fs::absent(Path::new(&stored.receipt.old_path))? {
            return Err(blocked("old source removal could not be verified"));
        }
        let receipt = &stored.receipt;
        self.database.transaction(|connection|{
            connection.query_row("SELECT pg_advisory_xact_lock(29162844963183719)",[],|_|Ok(()))?;
            let current=load_relocation(connection,&request.relocation_id)?;
            if current.receipt.state=="committed" {return Ok(current.receipt);}
            if !matches!(current.receipt.state.as_str(),"prepared"|"verified") {return Err(blocked("relocation is no longer pending").into());}
            inactive(connection,&receipt.repository_id)?;
            consumers::assert_unchanged(connection,&receipt.repository_id,&stored.native.consumers)?;
            validate_project_destinations(connection,receipt)?;
            let new=Path::new(&receipt.new_path);let old=Path::new(&receipt.old_path);
            let changed=connection.execute("UPDATE worktrees SET worktree_path=$1 WHERE worktree_id=$2 AND repository_id=$3 AND worktree_path=$4",(&receipt.new_path,&receipt.worktree_id,&receipt.repository_id,&receipt.old_path))?;
            if changed!=1 {return Err(blocked("registered source path changed before relocation commit").into());}
            if stored.native.moves_repository_root {
                let changed=connection.execute("UPDATE repositories SET root_path=$1 WHERE repository_id=$2 AND root_path=$3",(&receipt.new_path,&receipt.repository_id,&receipt.old_path))?;
                if changed!=1 {return Err(blocked("registered repository root changed before relocation commit").into());}
            }
            rebind_native(connection,receipt,&stored.native,&target)?;
            update_project_paths(connection,receipt,&now,&caller.actor())?;
            crate::storage::relocate_source_paths(connection,old,new,&receipt.relocation_id,OffsetDateTime::now_utc().unix_timestamp_nanos().max(0) as u64/1_000_000)?;
            connection.execute("INSERT INTO registry_path_aliases(old_path,repository_id,worktree_id,new_path,relocation_id,created_at) VALUES($1,$2,$3,$4,$5,$6)",(&receipt.old_path,&receipt.repository_id,&receipt.worktree_id,&receipt.new_path,&receipt.relocation_id,&now))?;
            connection.execute("UPDATE repository_relocations SET state='committed',verified_at=coalesce(verified_at,$1),committed_at=$1,checkout_revision=checkout_revision+1,native_json=$3 WHERE relocation_id=$2",(&now,&receipt.relocation_id,encode_native(&stored.native)?))?;
            Ok(load_relocation(connection,&receipt.relocation_id)?.receipt)
        }).map_err(db_error)
    }

    pub(crate) fn relocate_cancel(
        &self,
        request: api::Reference,
        caller: &Caller,
    ) -> Result<api::Receipt, ProtocolError> {
        let stored = self.relocation_reference(&request, caller)?;
        if stored.receipt.state == "committed" {
            return Err(blocked("a committed relocation cannot be cancelled"));
        }
        if stored.receipt.state == "cancelled" {
            return Ok(stored.receipt);
        }
        if std::fs::symlink_metadata(&stored.receipt.new_path).is_ok() {
            return Err(blocked(
                "relocation destination exists; recover or complete the move before cancellation",
            ));
        }
        let info = resolve_worktree(
            Path::new(&stored.receipt.old_path),
            Some((stored.native.owner_uid, stored.native.owner_gid)),
        )?;
        if native_identity(&info)? != stored.native.identity {
            return Err(blocked(
                "the original source no longer matches the prepared plan",
            ));
        }
        self.database.transaction(|connection|{
            connection.execute("UPDATE repository_relocations SET state='cancelled' WHERE relocation_id=$1 AND state IN ('prepared','verified')",[&request.relocation_id])?;
            Ok(load_relocation(connection,&request.relocation_id)?.receipt)
        }).map_err(db_error)
    }
}

fn encode_native(native: &PreparedNative) -> Result<String, DatabaseError> {
    serde_json::to_string(native)
        .map_err(|_| DatabaseError::Domain(blocked("relocation proof cannot be retained")))
}

fn rebind_native(
    connection: &Connection,
    receipt: &api::Receipt,
    native: &PreparedNative,
    target: &VerifiedTarget,
) -> Result<(), DatabaseError> {
    let changed=connection.execute("UPDATE repository_native_identities SET root_identity=$1,common_identity=$2 WHERE repository_id=$3 AND root_identity=$4 AND common_identity=$5",(&target.identity.repository_root,&target.identity.common,&receipt.repository_id,&native.identity.repository_root,&native.identity.common))?;
    if changed != 1 {
        return Err(blocked("registered repository native identity changed before commit").into());
    }
    let mut worktrees = vec![(&receipt.worktree_id, &native.identity, &target.identity)];
    for (id, _, identity) in &native.linked {
        let rebound = target
            .linked
            .iter()
            .find(|(candidate, _, _)| candidate == id)
            .map(|(_, _, identity)| identity)
            .ok_or_else(|| {
                DatabaseError::Domain(blocked("verified linked checkout identity is missing"))
            })?;
        worktrees.push((id, identity, rebound));
    }
    for (id, old, new) in worktrees {
        let changed=connection.execute("UPDATE worktree_native_identities SET root_identity=$1,git_identity=$2 WHERE worktree_id=$3 AND root_identity=$4 AND git_identity=$5",(&new.worktree_root,&new.git,id,&old.worktree_root,&old.git))?;
        if changed != 1 {
            return Err(
                blocked("registered checkout native identity changed before commit").into(),
            );
        }
    }
    Ok(())
}

fn validate_project_destinations(
    connection: &Connection,
    receipt: &api::Receipt,
) -> Result<(), DatabaseError> {
    let rows = {
        let mut query=connection.prepare("SELECT c.project_id,c.revision,EXISTS(SELECT 1 FROM project_roots r WHERE r.project_id=c.project_id AND ($3=r.absolute_path OR strpos($3,r.absolute_path||'/')=1)) FROM project_checkouts c WHERE c.worktree_id=$1 AND c.physical_repository_id=$2 ORDER BY c.project_id")?;
        query
            .query_map(
                (
                    &receipt.worktree_id,
                    &receipt.repository_id,
                    &receipt.new_path,
                ),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    if !rows.iter().any(|(project, revision, _)| {
        project == &receipt.project_id && *revision == receipt.checkout_revision
    }) || rows.iter().any(|(_, _, inside)| !inside)
    {
        return Err(blocked("checkout revision changed or the destination is outside an owning project's declared roots").into());
    }
    Ok(())
}

fn update_project_paths(
    connection: &Connection,
    receipt: &api::Receipt,
    now: &str,
    actor: &str,
) -> Result<(), DatabaseError> {
    let rows = {
        let mut query=connection.prepare("SELECT project_id,record_json,revision FROM project_checkouts WHERE worktree_id=$1 AND physical_repository_id=$2 AND absolute_path=$3 ORDER BY project_id")?;
        query
            .query_map(
                (
                    &receipt.worktree_id,
                    &receipt.repository_id,
                    &receipt.old_path,
                ),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    if !rows.iter().any(|(project, _, revision)| {
        project == &receipt.project_id && *revision == receipt.checkout_revision
    }) {
        return Err(blocked("project checkout revision changed before relocation commit").into());
    }
    for (project, document, revision) in rows {
        let roots = {
            let mut query=connection.prepare("SELECT absolute_path,kind FROM project_roots WHERE project_id=$1 ORDER BY length(absolute_path) DESC")?;
            query
                .query_map([&project], |row| {
                    Ok((
                        PathBuf::from(row.get::<_, String>(0)?),
                        row.get::<_, String>(1)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let (root, kind) = roots
            .into_iter()
            .find(|(root, _)| Path::new(&receipt.new_path).starts_with(root))
            .ok_or_else(|| {
                DatabaseError::Domain(blocked(
                    "destination is outside another owning project's declared roots",
                ))
            })?;
        let mut value: serde_json::Value = serde_json::from_str(&document)
            .map_err(|_| DatabaseError::Domain(blocked("project checkout cache is invalid")))?;
        value["relative_path"] = Path::new(&receipt.new_path)
            .strip_prefix(&root)
            .map_err(|_| DatabaseError::Domain(blocked("destination root changed")))?
            .to_string_lossy()
            .into_owned()
            .into();
        value["root_kind"] = kind.into();
        value["revision"] = (revision + 1).into();
        if value
            .get("absolute_path")
            .is_some_and(|value| !value.is_null())
        {
            value["absolute_path"] = receipt.new_path.clone().into();
        }
        connection.execute("UPDATE project_checkouts SET absolute_path=$1,revision=revision+1,record_json=$2,updated_at=$3,updated_by=$4 WHERE project_id=$5 AND worktree_id=$6",(&receipt.new_path,serde_json::to_string(&value).map_err(|_|DatabaseError::Domain(blocked("project checkout cache cannot be retained")))?,now,actor,&project,&receipt.worktree_id))?;
        connection.execute("UPDATE projects SET revision=revision+1,refreshed_at=NULL,updated_at=$1,updated_by=$2 WHERE project_id=$3",(now,actor,&project))?;
        connection.execute("INSERT INTO project_events(project_id,physical_repository_id,target_id,kind,summary,actor,at) VALUES($1,$2,$3,'checkout.relocated','Checkout moved with its identity and history preserved',$4,$5)",(&project,&receipt.repository_id,&receipt.worktree_id,actor,now))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn relocation_fingerprint(
    source: &Path,
    repository_root: &Path,
    native: &NativeIdentity,
    version: u32,
) -> Result<String, ProtocolError> {
    use rustix::fs::{self as unix, AtFlags, Dir, Mode, OFlags};
    use std::fs::File;
    struct Budget {
        entries: u64,
        bytes: u64,
        deadline: Instant,
    }
    struct Tree<'a> {
        device: u64,
        git: bool,
        repair_pointer: &'a Path,
    }
    fn names(directory: &File) -> Result<Vec<OsString>, ProtocolError> {
        let mut result = Vec::new();
        let mut entries = Dir::read_from(directory)
            .map_err(|_| blocked("relocation directory cannot be read"))?;
        for entry in &mut entries {
            let entry = entry.map_err(|_| blocked("relocation directory changed during review"))?;
            let bytes = entry.file_name().to_bytes();
            if bytes != b"." && bytes != b".." {
                result.push(OsString::from_vec(bytes.to_vec()));
            }
            if result.len() > 1_000_000 {
                return Err(blocked(
                    "relocation file-count bound exceeded; no plan was accepted",
                ));
            }
        }
        result.sort();
        Ok(result)
    }
    fn walk(
        directory: &File,
        relative: &Path,
        tree: &Tree<'_>,
        digest: &mut Sha256,
        budget: &mut Budget,
        depth: usize,
    ) -> Result<(), ProtocolError> {
        if depth > 128 || Instant::now() > budget.deadline {
            return Err(blocked(
                "relocation fingerprint reached its traversal deadline or depth limit",
            ));
        }
        let entries = names(directory)?;
        for name in &entries {
            budget.entries += 1;
            if budget.entries > 1_000_000 {
                return Err(blocked(
                    "relocation file-count bound exceeded; no plan was accepted",
                ));
            }
            let child = relative.join(name);
            if !tree.git && child == Path::new(".git") {
                continue;
            }
            if !tree.git && name == ".git" {
                return Err(blocked(
                    "nested Git repositories require separate reviewed relocation",
                ));
            }
            if tree.git && name.as_bytes().ends_with(b".lock") {
                return Err(blocked("a Git lock still uses this repository"));
            }
            let before = unix::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|_| blocked("relocation contents changed during review"))?;
            if before.st_dev != tree.device {
                return Err(blocked(
                    "relocation tree contains another mounted filesystem",
                ));
            }
            let encoded = child.as_os_str().as_bytes();
            digest.update((encoded.len() as u64).to_be_bytes());
            digest.update(encoded);
            digest.update(before.st_mode.to_be_bytes());
            digest.update(before.st_uid.to_be_bytes());
            digest.update(before.st_gid.to_be_bytes());
            // Git's one exact backpointer changes after worktree move/repair.
            // Its replacement is validated by native Git identities and paths.
            if tree.git
                && !tree.repair_pointer.as_os_str().is_empty()
                && child == tree.repair_pointer
            {
                continue;
            }
            match unix::FileType::from_raw_mode(before.st_mode) {
                unix::FileType::Directory => {
                    let file = File::from(
                        unix::openat2(
                            directory,
                            name,
                            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                            unix::ResolveFlags::BENEATH
                                | unix::ResolveFlags::NO_SYMLINKS
                                | unix::ResolveFlags::NO_XDEV,
                        )
                        .map_err(|_| blocked("relocation directory identity changed"))?,
                    );
                    let metadata = file
                        .metadata()
                        .map_err(|_| blocked("relocation metadata unavailable"))?;
                    if (metadata.dev(), metadata.ino()) != (before.st_dev, before.st_ino) {
                        return Err(blocked("relocation directory identity changed"));
                    }
                    walk(&file, &child, tree, digest, budget, depth + 1)?;
                }
                unix::FileType::RegularFile => {
                    let mut file = File::from(
                        unix::openat(
                            directory,
                            name,
                            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                            Mode::empty(),
                        )
                        .map_err(|_| blocked("relocation file identity changed"))?,
                    );
                    let metadata = file
                        .metadata()
                        .map_err(|_| blocked("relocation metadata unavailable"))?;
                    if !metadata.is_file()
                        || (metadata.dev(), metadata.ino()) != (before.st_dev, before.st_ino)
                    {
                        return Err(blocked("relocation file identity changed"));
                    }
                    digest.update(metadata.len().to_be_bytes());
                    let mut buffer = [0_u8; 64 * 1024];
                    loop {
                        if Instant::now() > budget.deadline {
                            return Err(blocked("relocation fingerprint reached its deadline"));
                        }
                        let count = file
                            .read(&mut buffer)
                            .map_err(|_| blocked("relocation source file cannot be read"))?;
                        if count == 0 {
                            break;
                        }
                        budget.bytes += count as u64;
                        if budget.bytes > 2 * 1024 * 1024 * 1024 {
                            return Err(blocked(
                                "relocation fingerprint exceeds the two-GiB review bound; use a separately reviewed migration workflow",
                            ));
                        }
                        digest.update(&buffer[..count]);
                    }
                    let after = file
                        .metadata()
                        .map_err(|_| blocked("relocation source file changed"))?;
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
                        return Err(blocked("relocation source file changed during review"));
                    }
                }
                unix::FileType::Symlink => {
                    let target = unix::readlinkat(directory, name, Vec::new())
                        .map_err(|_| blocked("relocation symlink changed"))?;
                    digest.update((target.as_bytes().len() as u64).to_be_bytes());
                    digest.update(target.as_bytes());
                }
                _ => {
                    return Err(blocked(
                        "relocation source contains a socket, pipe, device or another live special file",
                    ));
                }
            }
            let after = unix::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|_| blocked("relocation contents changed during review"))?;
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
                return Err(blocked("relocation contents changed during review"));
            }
        }
        if names(directory)? != entries {
            return Err(blocked(
                "relocation directory entries changed during review",
            ));
        }
        Ok(())
    }
    let source = crate::storage::fs::open_directory(source)?;
    let git = crate::storage::fs::open_directory(&repository_root.join(".git"))?;
    let source_device = source
        .metadata()
        .map_err(|_| blocked("relocation source metadata unavailable"))?
        .dev();
    let git_device = git
        .metadata()
        .map_err(|_| blocked("Git common metadata unavailable"))?
        .dev();
    let mut digest = Sha256::new();
    digest.update(b"dc2-inode-preserving-relocation-v1\0");
    if version >= 2 {
        for directory in [&source, &git] {
            let metadata = directory
                .metadata()
                .map_err(|_| blocked("relocation root metadata unavailable"))?;
            digest.update(metadata.mode().to_be_bytes());
            digest.update(metadata.uid().to_be_bytes());
            digest.update(metadata.gid().to_be_bytes());
        }
    }
    let mut budget = Budget {
        entries: 0,
        bytes: 0,
        deadline: Instant::now() + Duration::from_secs(60),
    };
    walk(
        &source,
        Path::new(""),
        &Tree {
            device: source_device,
            git: false,
            repair_pointer: Path::new(""),
        },
        &mut digest,
        &mut budget,
        0,
    )?;
    digest.update(b"\0git-common\0");
    let pointer = if native.git_relative.is_empty() {
        PathBuf::new()
    } else {
        Path::new(&native.git_relative).join("gitdir")
    };
    walk(
        &git,
        Path::new(""),
        &Tree {
            device: git_device,
            git: true,
            repair_pointer: &pointer,
        },
        &mut digest,
        &mut budget,
        0,
    )?;
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(not(target_os = "linux"))]
fn relocation_fingerprint(
    _: &Path,
    _: &Path,
    _: &NativeIdentity,
    _: u32,
) -> Result<String, ProtocolError> {
    Err(blocked(
        "verified relocation is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests;
