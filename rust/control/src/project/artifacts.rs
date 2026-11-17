//! Explicit ownership links between a project checkout and a storage artifact.
//!
//! A path or a matching name never creates one of these links.  Adoption binds
//! the storage record's native identity and revision, so later cleanup can
//! distinguish a moved or replaced build output from the output an operator
//! reviewed.
use super::*;
use crate::storage::model::{Locator, Record};
use devcoordinator2_api::{project::Location, project::artifacts as api};
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

fn native_path(record: &Record) -> Result<&Path, ProtocolError> {
    match &record.locator {
        Locator::Directory { path, .. } => Ok(path),
        Locator::Mount { target, .. } => Ok(target),
        _ => Err(invalid("only filesystem build artifacts can be adopted")),
    }
}

pub(super) fn binding_valid(record: &Record, device: &str, inode: &str) -> bool {
    record.artifact.removed_at_ms.is_none()
        && identity(record).is_ok_and(|pair| pair.0 == device && pair.1 == inode)
        && native_path(record)
            .and_then(crate::storage::fs::identity)
            .is_ok_and(|pair| pair.0.to_string() == device && pair.1.to_string() == inode)
}

/// Cleanup must inspect every explicit link, including allocations outside the
/// checkout. A stale link cannot silently become deletion authority.
pub(crate) fn active_records(
    connection: &Connection,
    project_id: &str,
    worktree_id: &str,
) -> Result<Vec<Record>, DatabaseError> {
    let rows = connection.prepare("SELECT artifact_id,bound_device,bound_inode FROM project_checkout_artifacts WHERE project_id=$1 AND worktree_id=$2 AND active ORDER BY association_id LIMIT 4097")?
        .query_map(params![project_id,worktree_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?
        .collect::<crate::database::Result<Vec<_>>>()?;
    if rows.len() > 4096 {
        return Err(unavailable_artifact(
            "checkout artifact associations exceed their fixed inspection bound",
        )
        .into());
    }
    rows.into_iter().map(|(id,device,inode)| {
        let record = record(connection,&id)?;
        if !binding_valid(&record,&device,&inode) {
            return Err(association_conflict("an associated artifact changed identity or is unavailable; review its ownership before cleanup").into());
        }
        Ok(record)
    }).collect()
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
            && selected.as_ref().is_none_or(|(_, _, current)| {
                root_path.components().count() > current.components().count()
            })
        {
            selected = Some((kind, root, root_path));
        }
    }
    let (root_kind, relative) = selected
        .and_then(|(kind, _, root)| {
            path.strip_prefix(root)
                .ok()
                .map(|p| (kind.to_owned(), p.to_path_buf()))
        })
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

fn roots(
    connection: &Connection,
    project_id: &str,
) -> Result<Vec<(String, String)>, DatabaseError> {
    connection
        .prepare("SELECT kind,absolute_path FROM project_roots WHERE project_id=$1 ORDER BY length(absolute_path) DESC,root_id")?
        .query_map([project_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<crate::database::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn record(connection: &Connection, artifact_id: &str) -> Result<Record, DatabaseError> {
    let raw = connection
        .query_row(
            "SELECT record_json::text FROM storage_artifacts WHERE artifact_id=$1",
            [artifact_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(not_found_db)?;
    serde_json::from_str(&raw).map_err(|_| {
        DatabaseError::Domain(unavailable_artifact("stored artifact record is invalid"))
    })
}

fn require_artifact_edit(record: &Record, scope: &Scope) -> Result<(), DatabaseError> {
    if !scope.admin
        && !record
            .artifact
            .repository_id
            .as_deref()
            .is_some_and(|id| scope.edits(id))
    {
        return Err(denied("operator access to the artifact's own source is required").into());
    }
    Ok(())
}

fn request_replay(
    connection: &Connection,
    project_id: &str,
    key: &str,
    body: &str,
) -> Result<Option<String>, DatabaseError> {
    let prior = connection.query_row("SELECT request_json::text,association_id FROM project_artifact_requests WHERE project_id=$1 AND idempotency_key=$2",params![project_id,key],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?;
    let Some((stored, id)) = prior else {
        return Ok(None);
    };
    if from_json::<serde_json::Value>(&stored)? != from_json::<serde_json::Value>(body)? {
        return Err(association_conflict(
            "idempotency key was already used for a different artifact request",
        )
        .into());
    }
    Ok(Some(id))
}

fn lock_project(connection: &Connection, project_id: &str) -> Result<(), DatabaseError> {
    connection
        .query_row(
            "SELECT project_id FROM projects WHERE project_id=$1 FOR UPDATE",
            [project_id],
            |_| Ok(()),
        )
        .optional()?
        .ok_or_else(not_found_db)
}

fn lock_source(connection: &Connection, physical: &str) -> Result<(), DatabaseError> {
    connection
        .query_row(
            "SELECT repository_id FROM repositories WHERE repository_id=$1 FOR UPDATE",
            [physical],
            |_| Ok(()),
        )
        .optional()?
        .ok_or_else(not_found_db)
}

fn ensure_ownership_editable(
    connection: &Connection,
    project_id: &str,
    worktree_id: &str,
    physical: &str,
    record: &Record,
) -> Result<(), DatabaseError> {
    crate::storage::ensure_artifact_ownership_editable(connection, record)?;
    let path = native_path(record)
        .map_err(DatabaseError::Domain)?
        .to_string_lossy()
        .into_owned();
    let reserved=connection.query_row("SELECT EXISTS(SELECT 1 FROM storage_jobs j JOIN storage_plans p ON p.plan_id=j.job_json->>'plan_id' WHERE j.kind='cleanup' AND j.state IN('queued','running','cancelling') AND (EXISTS(SELECT 1 FROM jsonb_array_elements(coalesce(p.plan_json#>'{checkout_removal,targets}','[]'::jsonb)) t WHERE t#>>'{binding,physical_repository_id}'=$3 AND ((t#>>'{binding,project_id}'=$1 AND t#>>'{binding,worktree_id}'=$2) OR t#>>'{binding,path}'=$4 OR strpos($4,(t#>>'{binding,path}')||'/')=1 OR strpos(t#>>'{binding,path}',$4||'/')=1)) OR EXISTS(SELECT 1 FROM jsonb_array_elements(coalesce(p.plan_json->'records','[]'::jsonb)) r WHERE r->>'resource_key'=$5 OR (r->'ancestor_keys') ? $5 OR $6::jsonb ? (r->>'resource_key'))))",params![project_id,worktree_id,physical,path,record.resource_key,json(&record.ancestor_keys)?],|row|row.get::<_,bool>(0))?;
    if reserved {
        return Err(ProtocolError::new(
            ErrorCode::Busy,
            "artifact ownership is reserved by an accepted cleanup; wait for that job to finish",
        )
        .into());
    }
    Ok(())
}

fn ownership_changed(
    connection: &Connection,
    project_id: &str,
    worktree_id: &str,
    actor: &str,
) -> Result<(), DatabaseError> {
    let now = timestamp();
    connection.execute("UPDATE project_checkouts SET revision=revision+1,record_json=jsonb_set(record_json,'{revision}',to_jsonb(revision+1)),updated_at=$3,updated_by=$4 WHERE project_id=$1 AND worktree_id=$2",params![project_id,worktree_id,now,actor])?;
    changed(connection, project_id, actor, &now)
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
            if for_edit {
                "SELECT physical_repository_id,repository_id,revision FROM project_checkouts WHERE project_id=$1 AND worktree_id=$2 FOR UPDATE"
            } else {
                "SELECT physical_repository_id,repository_id,revision FROM project_checkouts WHERE project_id=$1 AND worktree_id=$2"
            },
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
        return Err(DatabaseError::Domain(denied(
            "association is outside the caller's project scope",
        )));
    }
    let stored = record(connection, &artifact_id)?;
    if !scope.admin
        && !stored
            .artifact
            .repository_id
            .as_deref()
            .is_some_and(|id| scope.sees(id))
    {
        return Err(denied("artifact is outside the caller's source scope").into());
    }
    let roots = roots(connection, project_id)?;
    let other = connection
        .prepare("SELECT a.worktree_id,c.physical_repository_id FROM project_checkout_artifacts a JOIN project_checkouts c ON c.project_id=a.project_id AND c.worktree_id=a.worktree_id WHERE a.project_id=$1 AND a.artifact_id=$2 AND a.active AND a.association_id<>$3 ORDER BY a.worktree_id LIMIT 4097")?
        .query_map(params![project_id, artifact_id, association_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<crate::database::Result<Vec<_>>>()?;
    if other.len() > 4096 {
        return Err(
            unavailable_artifact("artifact owners exceed their fixed inspection bound").into(),
        );
    }
    let other: Vec<_> = other
        .into_iter()
        .filter(|(_, physical)| scope.sees(physical))
        .map(|(worktree, _)| worktree)
        .collect();
    let has_other_owners = connection.query_row("SELECT EXISTS(SELECT 1 FROM project_checkout_artifacts WHERE active AND association_id<>$1 AND (artifact_id=$2 OR (bound_device=$3 AND bound_inode=$4)))",params![association_id,artifact_id,device,inode],|row| row.get(0))?;
    let identity_valid = binding_valid(&stored, &device, &inode);
    let mut location = location(&stored, &roots);
    if !identity_valid {
        location.allocated_bytes = None;
    }
    Ok(api::Association {
        association_id: association_id.into(),
        project_id: project_id.into(),
        worktree_id,
        artifact_id,
        revision,
        artifact_revision: stored.artifact.revision,
        ownership,
        active,
        identity_valid,
        other_checkout_count: other.len() as u32,
        has_other_owners,
        other_checkout_ids: other.into_iter().take(64).collect(),
        location,
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
        if let Some(cursor) = &request.cursor {
            validate_id(cursor)?;
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
                if encoded_len(&associations)>PAGE_BYTES && associations.len()>1 {
                    associations.pop();
                    break;
                }
            }
            let next_cursor = (more || associations.len()<ids.len()).then(|| associations.last().map(|row|row.association_id.clone())).flatten();
            Ok(api::Page { associations, next_cursor })
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
        let request_body = json(&serde_json::json!({"operation":"adopt","request":request}))
            .map_err(database_error)?;
        let association = self.database.transaction(|connection| {
            let canonical = canonical_checkout_id(connection, &request.project_id, &request.worktree_id)?;
            let (physical, _, checkout_revision) = visible_checkout(connection, &request.project_id, &canonical, &scope, true)?;
            // Match metadata writers: checkout first, then project/source. A
            // background refresh may already hold this checkout while it
            // advances the project, so the reverse order could deadlock.
            lock_project(connection,&request.project_id)?;
            if let Some(id) = request_replay(connection,&request.project_id,&request.idempotency_key,&request_body)? {
                let prior=load(connection,&request.project_id,&id,&scope)?;
                require_artifact_edit(&record(connection,&prior.artifact_id)?,&scope)?;
                return Ok(prior);
            }
            if u64::from(checkout_revision) != u64::from(request.expected_checkout_revision) {
                return Err(association_conflict("checkout changed; refresh before adopting an artifact").into());
            }
            lock_source(connection,&physical)?;
            connection.query_row("SELECT artifact_id FROM storage_artifacts WHERE artifact_id=$1 FOR UPDATE",[&request.artifact_id],|_|Ok(())).optional()?.ok_or_else(not_found_db)?;
            let stored = record(connection, &request.artifact_id)?;
            require_artifact_edit(&stored,&scope)?;
            if stored.artifact.repository_id.as_deref()!=Some(physical.as_str()) {
                return Err(association_conflict("artifact and checkout must belong to the same physical source").into());
            }
            ensure_ownership_editable(connection,&request.project_id,&canonical,&physical,&stored)?;
            if stored.artifact.revision != request.expected_artifact_revision {
                return Err(association_conflict("artifact changed; refresh before adopting it").into());
            }
            if stored.artifact.kind != devcoordinator2_api::storage::Kind::BuildOutput
                && stored.artifact.kind != devcoordinator2_api::storage::Kind::DependencyCache
            {
                return Err(invalid("only build outputs and dependency caches may be adopted").into());
            }
            let (device, inode) = identity(&stored).map_err(DatabaseError::Domain)?;
            if !binding_valid(&stored,&device,&inode) {
                return Err(association_conflict("artifact directory changed identity or is unavailable; revalidate it before adoption").into());
            }
            connection.query_row("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))",[format!("project-artifact:{device}:{inode}")],|_|Ok(()))?;
            let incompatible = connection.query_row("SELECT EXISTS(SELECT 1 FROM project_checkout_artifacts WHERE active AND (artifact_id=$1 OR (bound_device=$4 AND bound_inode=$5)) AND NOT (project_id=$2 AND worktree_id=$3) AND ($6='exclusive' OR ownership='exclusive'))",params![request.artifact_id,request.project_id,canonical,device,inode,request.ownership],|row|row.get::<_,bool>(0))?;
            if incompatible { return Err(association_conflict("artifact has another active owner incompatible with the requested ownership").into()); }
            let prior = connection.query_row("SELECT association_id,revision FROM project_checkout_artifacts WHERE project_id=$1 AND worktree_id=$2 AND artifact_id=$3",params![request.project_id,canonical,request.artifact_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,u32>(1)?))).optional()?;
            if let Some((id, revision)) = prior {
                if revision != request.expected_revision { return Err(association_conflict("association changed; refresh before adopting it").into()); }
                connection.execute("UPDATE project_checkout_artifacts SET revision=revision+1,ownership=$4,bound_device=$5,bound_inode=$6,active=true,updated_at=$7,updated_by=$8 WHERE project_id=$1 AND worktree_id=$2 AND artifact_id=$3",params![request.project_id,canonical,request.artifact_id,request.ownership,device,inode,timestamp(),actor])?;
                connection.execute("INSERT INTO project_artifact_requests(project_id,idempotency_key,request_json,association_id,created_at) VALUES($1,$2,$3,$4,$5)",params![request.project_id,request.idempotency_key,request_body,id,timestamp()])?;
                ownership_changed(connection,&request.project_id,&canonical,&actor)?;
                return load(connection, &request.project_id, &id, &scope);
            }
            if request.expected_revision != 0 { return Err(association_conflict("new association must use revision zero").into()); }
            let id = new_id("association").map_err(DatabaseError::Domain)?;
            let now = timestamp();
            connection.execute("INSERT INTO project_checkout_artifacts(association_id,project_id,worktree_id,artifact_id,revision,ownership,bound_device,bound_inode,active,provenance,created_at,updated_at,updated_by) VALUES($1,$2,$3,$4,1,$5,$6,$7,true,'explicit',$8,$8,$9)",params![id,request.project_id,canonical,request.artifact_id,request.ownership,device,inode,now,actor])?;
            connection.execute("INSERT INTO project_artifact_requests(project_id,idempotency_key,request_json,association_id,created_at) VALUES($1,$2,$3,$4,$5)",params![request.project_id,request.idempotency_key,request_body,id,now])?;
            ownership_changed(connection,&request.project_id,&canonical,&actor)?;
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
        let request_body = json(&serde_json::json!({"operation":"detach","request":request}))
            .map_err(database_error)?;
        let result = self.database.transaction(|connection| {
            let canonical = canonical_checkout_id(connection, &request.project_id, &request.worktree_id)?;
            let (physical,_,_)=visible_checkout(connection, &request.project_id, &canonical, &scope, true)?;
            lock_project(connection,&request.project_id)?;
            if let Some(id) = request_replay(connection,&request.project_id,&request.idempotency_key,&request_body)? {
                let prior=load(connection,&request.project_id,&id,&scope)?;
                require_artifact_edit(&record(connection,&prior.artifact_id)?,&scope)?;
                return Ok(prior);
            }
            let association=load(connection,&request.project_id,&request.association_id,&scope)?;
            lock_source(connection,&physical)?;
            connection.query_row("SELECT artifact_id FROM storage_artifacts WHERE artifact_id=$1 FOR UPDATE",[&association.artifact_id],|_|Ok(())).optional()?.ok_or_else(not_found_db)?;
            let stored=record(connection,&association.artifact_id)?;
            require_artifact_edit(&stored,&scope)?;
            ensure_ownership_editable(connection,&request.project_id,&canonical,&physical,&stored)?;
            let current = connection.query_row("SELECT revision FROM project_checkout_artifacts WHERE project_id=$1 AND association_id=$2 AND worktree_id=$3 AND active FOR UPDATE",params![request.project_id,request.association_id,canonical],|row|row.get::<_,u32>(0)).optional()?.ok_or_else(not_found_db)?;
            if current != request.expected_revision { return Err(association_conflict("association changed; refresh before detaching it").into()); }
            let now = timestamp();
            connection.execute("UPDATE project_checkout_artifacts SET active=false,revision=revision+1,updated_at=$4,updated_by=$5 WHERE project_id=$1 AND association_id=$2 AND worktree_id=$3",params![request.project_id,request.association_id,canonical,now,actor])?;
            connection.execute("INSERT INTO project_artifact_requests(project_id,idempotency_key,request_json,association_id,created_at) VALUES($1,$2,$3,$4,$5)",params![request.project_id,request.idempotency_key,request_body,request.association_id,now])?;
            ownership_changed(connection,&request.project_id,&canonical,&actor)?;
            load(connection, &request.project_id, &request.association_id, &scope)
        }).map_err(database_error)?;
        self.publish_change(&request.project_id, "project.checkout.artifact.detached");
        Ok(result)
    }
}
