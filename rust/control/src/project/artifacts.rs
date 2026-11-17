//! Explicit ownership links between a project checkout and a storage artifact.
//!
//! A path or a matching name never creates one of these links.  Adoption binds
//! the storage record's native identity and revision, so later cleanup can
//! distinguish a moved or replaced build output from the output an operator
//! reviewed.
use super::*;
use crate::storage::model::{Locator, Record};
use devcoordinator2_api::{project::artifacts as api, project::Location};
use std::path::{Path, PathBuf};

fn identity(record: &Record) -> Result<(String, String), ProtocolError> {
    let pair = match &record.locator {
        Locator::Directory { device, inode, .. } | Locator::Mount { device, inode, .. } => {
            (*device, *inode)
        }
        _ => return Err(invalid("only filesystem build artifacts can be adopted")),
    };
    Ok((pair.0.to_string(), pair.1.to_string()))
}

fn location(record: &Record, roots: &[(String, String)]) -> Location {
    let fallback = PathBuf::new();
    let path = match &record.locator {
        Locator::Directory { path, .. } => path,
        Locator::Mount { target, .. } => target,
        Locator::Evidence { worktree, .. } => worktree,
        Locator::Docker { .. } => &fallback,
    };
    let mut selected: Option<(&str, &str, &Path)> = None;
    for (kind, root) in roots {
        let root_path = Path::new(root);
        if path.starts_with(root_path)
            && selected
                .as_ref()
                .is_none_or(|(_, _, current)| root_path.components().count() > current.components().count())
        {
            selected = Some((kind, root, root_path));
        }
    }
    let (root_kind, relative) = selected
        .and_then(|(kind, _, root)| path.strip_prefix(root).ok().map(|p| (kind.to_owned(), p.to_path_buf())))
        .unwrap_or_else(|| {
            (
                "outside".to_owned(),
                path.file_name().map(PathBuf::from).unwrap_or_default(),
            )
        });
    Location {
        kind: "artifact".into(),
        relative_path: relative.to_string_lossy().into_owned(),
        root_kind,
        allocated_bytes: record.artifact.allocated_bytes,
        artifact_id: Some(record.artifact.artifact_id.clone()),
    }
}

fn roots(connection: &Connection, project_id: &str) -> Result<Vec<(String, String)>, DatabaseError> {
    connection
        .prepare("SELECT kind,absolute_path FROM project_roots WHERE project_id=$1 ORDER BY length(absolute_path) DESC,root_id")?
        .query_map([project_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<crate::database::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn record(connection: &Connection, artifact_id: &str) -> Result<Record, DatabaseError> {
    let raw = connection
        .query_row(
            "SELECT record_json::text FROM storage_artifacts WHERE artifact_id=$1 AND removed_at_ms IS NULL",
            [artifact_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(not_found_db)?;
    serde_json::from_str(&raw)
        .map_err(|_| DatabaseError::Domain(unavailable_artifact("stored artifact record is invalid")))
}

fn unavailable_artifact(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::StorageUnavailable, message)
}

fn association_conflict(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::ConfigurationConflict, message)
}

fn visible_checkout(
    connection: &Connection,
    project_id: &str,
    worktree_id: &str,
    scope: &Scope,
    for_edit: bool,
) -> Result<(String, String, u32), DatabaseError> {
    let (physical, repository, revision) = connection
        .query_row(
            "SELECT physical_repository_id,repository_id,revision FROM project_checkouts WHERE project_id=$1 AND worktree_id=$2",
            params![project_id, worktree_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, u32>(2)?)),
        )
        .optional()?
        .ok_or_else(not_found_db)?;
    if !scope.sees(&physical) || (for_edit && !scope.edits(&physical)) {
        return Err(DatabaseError::Domain(denied(if for_edit {
            "operator access to this checkout's source is required"
        } else {
            "checkout is outside the caller's project scope"
        })));
    }
    Ok((physical, repository, revision))
}

fn load(
    connection: &Connection,
    project_id: &str,
    association_id: &str,
    scope: &Scope,
) -> Result<api::Association, DatabaseError> {
    let (worktree_id, artifact_id, revision, ownership, device, inode, active, provenance, created_at, updated_at, physical) = connection
        .query_row("SELECT a.worktree_id,a.artifact_id,a.revision,a.ownership,a.bound_device,a.bound_inode,a.active,a.provenance,a.created_at,a.updated_at,c.physical_repository_id FROM project_checkout_artifacts a JOIN project_checkouts c ON c.project_id=a.project_id AND c.worktree_id=a.worktree_id WHERE a.project_id=$1 AND a.association_id=$2",params![project_id,association_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,u32>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?,row.get::<_,bool>(6)?,row.get::<_,String>(7)?,row.get::<_,String>(8)?,row.get::<_,String>(9)?,row.get::<_,String>(10)?))).optional()?.ok_or_else(not_found_db)?;
    if !scope.sees(&physical) {
        return Err(DatabaseError::Domain(denied("association is outside the caller's project scope")));
    }
    let stored = record(connection, &artifact_id)?;
    let roots = roots(connection, project_id)?;
    let other = connection
        .prepare("SELECT a.worktree_id,c.physical_repository_id FROM project_checkout_artifacts a JOIN project_checkouts c ON c.project_id=a.project_id AND c.worktree_id=a.worktree_id WHERE a.project_id=$1 AND a.artifact_id=$2 AND a.active AND a.association_id<>$3 ORDER BY a.worktree_id")?
        .query_map(params![project_id, artifact_id, association_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .filter_map(|row| row.ok().filter(|(_, physical)| scope.sees(physical)).map(|(worktree, _)| worktree))
        .collect::<Vec<_>>();
    Ok(api::Association {
        association_id: association_id.into(),
        project_id: project_id.into(),
        worktree_id,
        artifact_id,
        revision,
        artifact_revision: stored.artifact.revision,
        ownership,
        active,
        other_checkout_count: other.len() as u32,
        has_other_owners: !other.is_empty(),
        other_checkout_ids: other,
        location: location(&stored, &roots),
        provenance,
        created_at,
        updated_at,
    })
}

impl ProjectService {
    pub fn artifact_associations(
        &self,
        request: api::List,
        caller: &Caller,
    ) -> Result<api::Page, ProtocolError> {
        validate_id(&request.project_id)?;
        validate_id(&request.worktree_id)?;
        if request.limit == 0 || request.limit > 200 {
            return Err(invalid("association page limit must be between 1 and 200"));
        }
        let scope = self.scope(caller)?;
        self.database.read_transaction(|connection| {
            let canonical = canonical_checkout_id(connection, &request.project_id, &request.worktree_id)?;
            visible_checkout(connection, &request.project_id, &canonical, &scope, false)?;
            let after = request.cursor.unwrap_or_default();
            let rows = connection
                .prepare("SELECT association_id FROM project_checkout_artifacts WHERE project_id=$1 AND worktree_id=$2 AND active AND association_id>$3 ORDER BY association_id LIMIT $4")?
                .query_map(params![request.project_id, canonical, after, i64::from(request.limit) + 1], |row| row.get::<_, String>(0))?
                .collect::<crate::database::Result<Vec<_>>>()?;
            let more = rows.len() > usize::from(request.limit);
            let ids: Vec<_> = rows.into_iter().take(usize::from(request.limit)).collect();
            let mut associations = Vec::with_capacity(ids.len());
            for id in &ids {
                associations.push(load(connection, &request.project_id, id, &scope)?);
            }
            Ok(api::Page { associations, next_cursor: more.then(|| ids.last().cloned()).flatten() })
        }).map_err(database_error)
    }

    pub fn adopt_artifact(
        &self,
        request: api::Adopt,
        caller: &Caller,
    ) -> Result<api::Association, ProtocolError> {
        validate_id(&request.project_id)?;
        validate_id(&request.worktree_id)?;
        validate_id(&request.artifact_id)?;
        validate_id(&request.idempotency_key)?;
        if !matches!(request.ownership.as_str(), "exclusive" | "shared") {
            return Err(invalid("artifact ownership must be exclusive or shared"));
        }
        let scope = self.scope(caller)?;
        let actor = caller.actor();
        let association = self.database.transaction(|connection| {
            let canonical = canonical_checkout_id(connection, &request.project_id, &request.worktree_id)?;
            let (physical, _, checkout_revision) = visible_checkout(connection, &request.project_id, &canonical, &scope, true)?;
            if u64::from(checkout_revision) != u64::from(request.expected_checkout_revision) {
                return Err(association_conflict("checkout changed; refresh before adopting an artifact").into());
            }
            let existing_request = connection.query_row("SELECT association_id FROM project_artifact_requests WHERE project_id=$1 AND idempotency_key=$2",params![request.project_id,request.idempotency_key],|row|row.get::<_,String>(0)).optional()?;
            if let Some(id) = existing_request {
                return load(connection, &request.project_id, &id, &scope);
            }
            let stored = record(connection, &request.artifact_id)?;
            if stored.artifact.revision != request.expected_artifact_revision {
                return Err(association_conflict("artifact changed; refresh before adopting it").into());
            }
            if stored.artifact.kind != devcoordinator2_api::storage::Kind::BuildOutput
                && stored.artifact.kind != devcoordinator2_api::storage::Kind::DependencyCache
            {
                return Err(invalid("only build outputs and dependency caches may be adopted").into());
            }
            let (device, inode) = identity(&stored).map_err(DatabaseError::Domain)?;
            let prior = connection.query_row("SELECT association_id,revision FROM project_checkout_artifacts WHERE project_id=$1 AND worktree_id=$2 AND artifact_id=$3",params![request.project_id,canonical,request.artifact_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,u32>(1)?))).optional()?;
            if let Some((id, revision)) = prior {
                if revision != request.expected_revision { return Err(association_conflict("association changed; refresh before adopting it").into()); }
                connection.execute("UPDATE project_checkout_artifacts SET revision=revision+1,ownership=$4,bound_device=$5,bound_inode=$6,active=true,updated_at=$7,updated_by=$8 WHERE project_id=$1 AND worktree_id=$2 AND artifact_id=$3",params![request.project_id,canonical,request.artifact_id,request.ownership,device,inode,timestamp(),actor])?;
                connection.execute("INSERT INTO project_artifact_requests(project_id,idempotency_key,request_json,association_id,created_at) VALUES($1,$2,$3,$4,$5)",params![request.project_id,request.idempotency_key,json(&request)?,id,timestamp()])?;
                return load(connection, &request.project_id, &id, &scope);
            }
            if request.expected_revision != 0 { return Err(association_conflict("new association must use revision zero").into()); }
            if request.ownership == "exclusive" {
                let owned = connection.query_row("SELECT EXISTS(SELECT 1 FROM project_checkout_artifacts WHERE artifact_id=$1 AND active AND NOT (project_id=$2 AND worktree_id=$3))",params![request.artifact_id,request.project_id,canonical],|row|row.get::<_,bool>(0))?;
                if owned { return Err(association_conflict("artifact already has another active owner").into()); }
            }
            let id = new_id("association").map_err(DatabaseError::Domain)?;
            let now = timestamp();
            connection.execute("INSERT INTO project_checkout_artifacts(association_id,project_id,worktree_id,artifact_id,revision,ownership,bound_device,bound_inode,active,provenance,created_at,updated_at,updated_by) VALUES($1,$2,$3,$4,1,$5,$6,$7,true,'explicit',$8,$8,$9)",params![id,request.project_id,canonical,request.artifact_id,request.ownership,device,inode,now,actor])?;
            connection.execute("INSERT INTO project_artifact_requests(project_id,idempotency_key,request_json,association_id,created_at) VALUES($1,$2,$3,$4,$5)",params![request.project_id,request.idempotency_key,json(&request)?,id,now])?;
            load(connection, &request.project_id, &id, &scope)
        }).map_err(database_error)?;
        self.publish_change(&request.project_id, "project.checkout.artifact.adopted");
        Ok(association)
    }

    pub fn detach_artifact(
        &self,
        request: api::Detach,
        caller: &Caller,
    ) -> Result<api::Association, ProtocolError> {
        validate_id(&request.project_id)?;
        validate_id(&request.worktree_id)?;
        validate_id(&request.association_id)?;
        validate_id(&request.idempotency_key)?;
        let scope = self.scope(caller)?;
        let actor = caller.actor();
        let result = self.database.transaction(|connection| {
            let canonical = canonical_checkout_id(connection, &request.project_id, &request.worktree_id)?;
            let (physical, _, _) = visible_checkout(connection, &request.project_id, &canonical, &scope, true)?;
            let current = connection.query_row("SELECT revision FROM project_checkout_artifacts WHERE project_id=$1 AND association_id=$2 AND worktree_id=$3 AND active FOR UPDATE",params![request.project_id,request.association_id,canonical],|row|row.get::<_,u32>(0)).optional()?.ok_or_else(not_found_db)?;
            if current != request.expected_revision { return Err(association_conflict("association changed; refresh before detaching it").into()); }
            let now = timestamp();
            connection.execute("UPDATE project_checkout_artifacts SET active=false,revision=revision+1,updated_at=$4,updated_by=$5 WHERE project_id=$1 AND association_id=$2 AND worktree_id=$3",params![request.project_id,request.association_id,canonical,now,actor])?;
            let _ = physical;
            load(connection, &request.project_id, &request.association_id, &scope)
        }).map_err(database_error)?;
        self.publish_change(&request.project_id, "project.checkout.artifact.detached");
        Ok(result)
    }
}
