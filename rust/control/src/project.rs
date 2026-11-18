//! Persisted Project Explorer authority. Request reads use PostgreSQL only.
use crate::{
    access::{Access, Caller},
    database::{Connection, Database, DatabaseError, OptionalExtension, params},
    repository::Registry,
    storage::StorageService,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use devcoordinator2_api::{ErrorCode, ProtocolError, project as api};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};
pub(crate) mod artifacts;
pub mod cleanup;
pub mod github;
mod refresh;
mod retirement;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub struct ProjectService {
    database: Database,
    registry: Registry,
    storage: Option<StorageService>,
    access: Option<Access>,
    github: Option<github::GithubClient>,
    events: Option<crate::events::EventService>,
    wake: Arc<tokio::sync::Notify>,
}
#[derive(Clone, Debug)]
pub(super) struct Scope {
    admin: bool,
    visible: Option<BTreeSet<String>>,
    operators: BTreeSet<String>,
}
impl Scope {
    fn visible_json(&self) -> Option<String> {
        self.visible
            .as_ref()
            .map(|ids| serde_json::to_string(ids).expect("string set"))
    }
    fn sees(&self, id: &str) -> bool {
        self.visible.as_ref().is_none_or(|ids| ids.contains(id))
    }
    fn edits(&self, id: &str) -> bool {
        self.admin || self.operators.contains(id)
    }
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    project: String,
    kind: String,
    after: String,
    query: Option<String>,
    lifecycle: Option<String>,
}
const SOURCE_VISIBLE: &str = "EXISTS(SELECT 1 FROM project_repository_memberships m WHERE m.project_id=s.project_id AND m.repository_id=s.repository_id AND ($2::jsonb IS NULL OR m.physical_repository_id IN (SELECT jsonb_array_elements_text($2::jsonb))))";
const BRANCH_VISIBLE: &str = "EXISTS(SELECT 1 FROM project_repository_memberships m WHERE m.project_id=b.project_id AND m.repository_id=b.repository_id AND ($2::jsonb IS NULL OR m.physical_repository_id IN (SELECT jsonb_array_elements_text($2::jsonb)))) AND (b.remote_present IS TRUE OR EXISTS(SELECT 1 FROM project_branch_observations o WHERE o.project_id=b.project_id AND o.branch_id=b.branch_id AND ($2::jsonb IS NULL OR o.physical_repository_id IN (SELECT jsonb_array_elements_text($2::jsonb)))))";
const BRANCH_HEAD: &str = "coalesce((SELECT o.head FROM project_branch_observations o WHERE o.project_id=b.project_id AND o.branch_id=b.branch_id AND ($2::jsonb IS NULL OR o.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))) ORDER BY o.local_present DESC,o.observed_at DESC,o.physical_repository_id LIMIT 1),b.remote_head)";

impl ProjectService {
    pub fn new(database: Database, registry: Registry) -> Self {
        Self {
            database,
            registry,
            storage: None,
            access: None,
            github: None,
            events: None,
            wake: Arc::new(tokio::sync::Notify::new()),
        }
    }
    pub fn with_storage(mut self, storage: StorageService) -> Self {
        self.storage = Some(storage);
        self
    }
    pub fn with_access(mut self, access: Access) -> Self {
        self.access = Some(access);
        self
    }
    pub fn with_github(mut self, github: github::GithubClient) -> Self {
        self.github = Some(github);
        self
    }
    pub fn with_events(mut self, events: crate::events::EventService) -> Self {
        self.events = Some(events);
        self
    }
    fn publish_change(&self, project: &str, kind: &str) {
        let Some(events) = &self.events else {
            return;
        };
        let repositories=self.database.read(|c|c.prepare("SELECT physical_repository_id FROM project_repository_memberships WHERE project_id=$1 ORDER BY physical_repository_id LIMIT 256")?.query_map([project],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>().map_err(DatabaseError::from));
        if let Ok(repositories) = repositories {
            for repository in repositories {
                let _ = events.publish(crate::events::NewEvent {
                    occurred_at: timestamp(),
                    event: devcoordinator2_api::results::OwnedEvent::Other(
                        devcoordinator2_api::results::OtherOwnedEvent {
                            review: None,
                            kind: kind.into(),
                            repository_id: Some(repository),
                            deployment_id: None,
                            subject_kind: "project".into(),
                            subject_id: project.into(),
                        },
                    ),
                    dedupe_key: None,
                });
            }
        }
    }
    pub fn refresh_signal(&self) -> Arc<tokio::sync::Notify> {
        self.wake.clone()
    }
    fn scope(&self, caller: &Caller) -> Result<Scope, ProtocolError> {
        if caller.is_local() {
            return Ok(Scope {
                admin: true,
                visible: None,
                operators: BTreeSet::new(),
            });
        }
        let access = self
            .access
            .as_ref()
            .ok_or_else(|| denied("project authorization is unavailable"))?;
        let principal = access.principal(caller)?;
        if principal.administrator {
            return Ok(Scope {
                admin: true,
                visible: None,
                operators: BTreeSet::new(),
            });
        }
        if principal.user_id.is_none() {
            return Err(denied("an admitted identity is required"));
        }
        let (visible, operators) = access.project_repositories(&principal)?;
        Ok(Scope {
            admin: false,
            visible: Some(visible),
            operators,
        })
    }
    pub fn list(&self, p: api::List, caller: &Caller) -> Result<api::ProjectList, ProtocolError> {
        validate_limit(p.limit)?;
        validate_query(p.query.as_deref())?;
        validate_optional_id(p.after_project_id.as_deref())?;
        let scope = self.scope(caller)?;
        let visible = scope.visible_json();
        self.database.read_transaction(|c| {
            let ids=c.prepare("SELECT p.project_id FROM projects p WHERE ($1::text IS NULL OR p.project_id>$1) AND ($3::text IS NULL OR strpos(lower(p.name),lower($3))>0 OR strpos(lower(p.slug),lower($3))>0) AND ($2::jsonb IS NULL OR EXISTS(SELECT 1 FROM project_repository_memberships m WHERE m.project_id=p.project_id AND m.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb)))) ORDER BY p.project_id LIMIT $4")?.query_map(params![p.after_project_id,visible,p.query,i64::from(p.limit)+1],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?;
            let more=ids.len()>usize::from(p.limit); let mut projects=Vec::new();
            for id in ids.into_iter().take(usize::from(p.limit)) { projects.push(summary(c,&id,&scope)?); }
            Ok(api::ProjectList { next_project_id:more.then(||projects.last().unwrap().project_id.clone()),projects })
        }).map_err(database_error)
    }
    pub fn status(&self, p: api::Reference, caller: &Caller) -> Result<api::Status, ProtocolError> {
        validate_id(&p.project_id)?;
        let scope = self.scope(caller)?;
        self.database.read_transaction(|c| {
            let project=summary(c,&p.project_id,&scope)?;
            let repositories=load_sources(c,&p.project_id,&scope)?;
            let roots=load_roots(c,&p.project_id,&scope)?;
            let page=api::Page {project_id:p.project_id.clone(),cursor:None,limit:50,query:None,lifecycle:None};
            let branches=branch_page(c,&page,&scope,None)?; let checkouts=checkout_page(c,&page,&scope,None)?;
            let storage=storage_summary(c,&p.project_id,&scope)?;
            let activity=history_page(c,&api::Page {limit:20,..page},&scope)?.events;
            let edit=!repositories.is_empty() && repositories.iter().all(|r|r.permissions.edit);
            let (refreshing,refreshed_at)=c.query_row("SELECT EXISTS(SELECT 1 FROM project_jobs j WHERE j.project_id=p.project_id AND j.state IN('queued','running')),p.refreshed_at FROM projects p WHERE p.project_id=$1",[&p.project_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let mut status=api::Status {revision:project.revision,project,roots,repositories,branches:branches.branches,checkouts:checkouts.checkouts,next_branch_cursor:branches.next_cursor,next_checkout_cursor:checkouts.next_cursor,storage,activity,permissions:api::Permissions {edit,cleanup:edit,admin:scope.admin},refreshing,refreshed_at};
            bound_status(&mut status);Ok(status)
        }).map_err(database_error)
    }
    pub fn branches(
        &self,
        p: api::Page,
        caller: &Caller,
    ) -> Result<api::BranchPage, ProtocolError> {
        validate_page(&p)?;
        let scope = self.scope(caller)?;
        self.database
            .read_transaction(|c| {
                summary(c, &p.project_id, &scope)?;
                branch_page(c, &p, &scope, None)
            })
            .map_err(database_error)
    }
    pub fn checkouts(
        &self,
        p: api::Page,
        caller: &Caller,
    ) -> Result<api::CheckoutPage, ProtocolError> {
        validate_page(&p)?;
        let scope = self.scope(caller)?;
        self.database
            .read_transaction(|c| {
                summary(c, &p.project_id, &scope)?;
                checkout_page(c, &p, &scope, None)
            })
            .map_err(database_error)
    }
    pub fn history(&self, p: api::Page, caller: &Caller) -> Result<api::History, ProtocolError> {
        validate_page(&p)?;
        let scope = self.scope(caller)?;
        self.database
            .read_transaction(|c| {
                summary(c, &p.project_id, &scope)?;
                history_page(c, &p, &scope)
            })
            .map_err(database_error)
    }
    pub fn branch(
        &self,
        p: api::BranchItem,
        caller: &Caller,
    ) -> Result<api::Branch, ProtocolError> {
        validate_id(&p.project_id)?;
        validate_id(&p.branch_id)?;
        let scope = self.scope(caller)?;
        self.database
            .read_transaction(|c| {
                let page = api::Page {
                    project_id: p.project_id,
                    cursor: None,
                    limit: 1,
                    query: None,
                    lifecycle: None,
                };
                branch_page(c, &page, &scope, Some(&p.branch_id))?
                    .branches
                    .into_iter()
                    .next()
                    .ok_or_else(not_found_db)
            })
            .map_err(database_error)
    }
    pub fn checkout(
        &self,
        p: api::CheckoutItem,
        caller: &Caller,
    ) -> Result<api::Checkout, ProtocolError> {
        validate_id(&p.project_id)?;
        validate_id(&p.worktree_id)?;
        let scope = self.scope(caller)?;
        self.database
            .read_transaction(|c| {
                let worktree_id = canonical_checkout_id(c, &p.project_id, &p.worktree_id)?;
                let page = api::Page {
                    project_id: p.project_id,
                    cursor: None,
                    limit: 1,
                    query: None,
                    lifecycle: None,
                };
                checkout_page(c, &page, &scope, Some(&worktree_id))?
                    .checkouts
                    .into_iter()
                    .next()
                    .ok_or_else(not_found_db)
            })
            .map_err(database_error)
    }
    pub fn context(
        &self,
        p: api::Context,
        caller: &Caller,
    ) -> Result<api::Memberships, ProtocolError> {
        if p.repository_ids.len() > 200 {
            return Err(invalid("at most 200 repository identities may be mapped"));
        }
        for id in &p.repository_ids {
            validate_id(id)?;
        }
        let scope = self.scope(caller)?;
        let requested = serde_json::to_string(&p.repository_ids)
            .map_err(|_| invalid("invalid repository identities"))?;
        let visible = scope.visible_json();
        self.database.read(|c|{let memberships=c.prepare("SELECT p.project_id,p.name,p.slug,m.physical_repository_id,m.repository_id FROM project_repository_memberships m JOIN projects p USING(project_id) WHERE m.physical_repository_id IN(SELECT jsonb_array_elements_text($1::jsonb)) AND ($2::jsonb IS NULL OR m.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))) ORDER BY p.project_id,m.physical_repository_id LIMIT 2000")?.query_map(params![requested,visible],|r|Ok(api::Membership{project_id:r.get(0)?,project_name:r.get(1)?,project_slug:r.get(2)?,repository_id:r.get(3)?,source_repository_id:r.get(4)?}))?.collect::<crate::database::Result<_>>()?;Ok(api::Memberships{memberships})}).map_err(database_error)
    }
    pub fn configure(
        &self,
        p: api::Configure,
        caller: &Caller,
    ) -> Result<api::Status, ProtocolError> {
        if !self.scope(caller)?.admin {
            return Err(denied("a Console administrator configures projects"));
        }
        validate_configuration(&p)?;
        let id = p.project_id.clone().unwrap_or(new_id("project")?);
        let now = timestamp();
        let actor = caller.actor();
        self.database.transaction(|c|{
            let old=c.query_row("SELECT revision FROM projects WHERE project_id=$1 FOR UPDATE",[&id],|r|r.get::<_,u32>(0)).optional()?;
            if old.unwrap_or(0)!=p.expected_revision {return Err(conflict().into());}
            c.execute("INSERT INTO projects(project_id,name,slug,created_at,updated_at,updated_by) VALUES($1,$2,$3,$4,$4,$5) ON CONFLICT(project_id) DO UPDATE SET name=excluded.name,slug=excluded.slug,revision=projects.revision+1,updated_at=excluded.updated_at,updated_by=excluded.updated_by",params![id,p.name,p.slug,now,actor])?;
            c.execute("DELETE FROM project_roots WHERE project_id=$1",[&id])?;
            for root in &p.roots {c.execute("INSERT INTO project_roots(project_id,root_id,kind,label,absolute_path) VALUES($1,$2,$3,$4,$5)",params![id,new_id("root").map_err(DatabaseError::Domain)?,root.kind,root.label,root.path])?;}
            let mut physicals=BTreeSet::new();
            for source in &p.repositories {
                let registered=c.query_row("SELECT EXISTS(SELECT 1 FROM repositories WHERE repository_id=$1)",[&source.repository_id],|r|r.get::<_,bool>(0))?;
                if !registered{return Err(invalid("each source must adopt an existing registered repository identity").into());}
                c.execute("INSERT INTO project_sources(project_id,repository_id,name,github_url,default_branch) VALUES($1,$2,$3,$4,$5) ON CONFLICT(project_id,repository_id) DO UPDATE SET name=excluded.name,github_url=excluded.github_url,default_branch=excluded.default_branch",params![id,source.repository_id,source.name,source.github_url,source.default_branch])?;
                for physical in &source.physical_repository_ids {
                    if !physicals.insert(physical.clone()){return Err(invalid("a physical repository may belong to one source in a project").into());}
                    let previous=c.query_row("SELECT repository_id FROM project_repository_memberships WHERE project_id=$1 AND physical_repository_id=$2",params![id,physical],|r|r.get::<_,String>(0)).optional()?;
                    if previous.as_ref().is_some_and(|old|old!=&source.repository_id){return Err(invalid("reassigning an adopted clone requires an explicit migration preserving its history").into());}
                    let root=c.query_row("SELECT root_path FROM repositories WHERE repository_id=$1",[physical],|r|r.get::<_,String>(0)).optional()?.ok_or_else(||DatabaseError::Domain(invalid("physical source is not registered")))?;
                    c.execute("INSERT INTO project_repository_memberships(project_id,repository_id,physical_repository_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",params![id,source.repository_id,physical])?;
                    let worktrees=c.prepare("SELECT worktree_id,worktree_path FROM worktrees WHERE repository_id=$1 ORDER BY worktree_id LIMIT 4097")?.query_map([physical],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<crate::database::Result<Vec<_>>>()?;
                    if worktrees.len()>4096{return Err(invalid("physical repository has too many registered checkouts").into());}
                    for (wid,path) in worktrees {
                        let role=if path==root {if physical==&source.repository_id {"primary"}else{"clone"}}else{"worktree"};
                        let (relative_path,root_kind)=display_path(&p.roots,&path);
                        let mut row=empty_checkout(&wid,&source.repository_id,&path,relative_path,root_kind,role);row.registration_state="registered".into();
                        c.execute("INSERT INTO project_checkouts(project_id,worktree_id,repository_id,physical_repository_id,absolute_path,role,protected_role,lifecycle,record_json,updated_at,updated_by,registration_state) VALUES($1,$2,$3,$4,$5,$6,$7,'active',$8::jsonb,$9,$10,'registered') ON CONFLICT(project_id,worktree_id) DO NOTHING",params![id,wid,source.repository_id,physical,path,role,(role=="primary").then_some(role),json(&row)?,now,actor])?;
                        if role=="primary" {c.execute("UPDATE project_sources SET primary_checkout_id=$3 WHERE project_id=$1 AND repository_id=$2",params![id,source.repository_id,wid])?;}
                    }
                }
            }
            // An adopted source is permanent until an explicit retirement/migration.
            let configured=serde_json::to_string(&physicals).map_err(|_|DatabaseError::Domain(invalid("invalid memberships")))?;
            let omitted=c.query_row("SELECT EXISTS(SELECT 1 FROM project_repository_memberships WHERE project_id=$1 AND physical_repository_id NOT IN(SELECT jsonb_array_elements_text($2::jsonb)))",params![id,configured],|r|r.get::<_,bool>(0))?;
            if omitted{return Err(invalid("configuration must retain adopted members; use retirement or migration to remove them").into());}
            event(c,&id,None,None,None,"project.configured","Project sources and roots were configured",&actor,&now)?;Ok(())
        }).map_err(database_error)?;
        self.publish_change(&id, "project.updated");
        self.status(api::Reference { project_id: id }, caller)
    }
    pub fn update_branch(
        &self,
        p: api::BranchUpdate,
        caller: &Caller,
    ) -> Result<api::Branch, ProtocolError> {
        validate_id(&p.project_id)?;
        validate_id(&p.branch_id)?;
        validate_metadata(
            &[&p.intent, &p.outcome, &p.owner, &p.next_action, &p.task_id],
            p.lifecycle.as_deref(),
        )?;
        let scope = self.scope(caller)?;
        let now = timestamp();
        let actor = caller.actor();
        let result=self.database.transaction(|c|{
            let (revision,source,raw)=c.query_row("SELECT revision,repository_id,record_json::text FROM project_branches WHERE project_id=$1 AND branch_id=$2 FOR UPDATE",params![p.project_id,p.branch_id],|r|Ok((r.get::<_,u32>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional()?.ok_or_else(not_found_db)?;
            require_source_edit(c,&p.project_id,&source,&scope)?;if revision!=p.expected_revision{return Err(conflict().into());}
            let mut row:api::Branch=from_json(&raw)?;apply_patch(&mut row.intent,&p.intent);apply_patch(&mut row.outcome,&p.outcome);apply_patch(&mut row.owner,&p.owner);apply_patch(&mut row.next_action,&p.next_action);apply_patch(&mut row.task_id,&p.task_id);if let Some(lifecycle)=&p.lifecycle{row.lifecycle=lifecycle.clone();}row.revision=revision+1;
            validate_task(c,&p.project_id,row.task_id.as_deref())?;
            c.execute("UPDATE project_branches SET revision=$3,intent=$4,outcome=$5,owner=$6,next_action=$7,task_id=$8,lifecycle=$9,record_json=$10::jsonb,updated_at=$11,updated_by=$12 WHERE project_id=$1 AND branch_id=$2",params![p.project_id,p.branch_id,row.revision,row.intent,row.outcome,row.owner,row.next_action,row.task_id,row.lifecycle,json(&row)?,now,actor])?;
            changed(c,&p.project_id,&actor,&now)?;event(c,&p.project_id,Some(&source),None,Some(&p.branch_id),"branch.updated","Branch intent and lifecycle were updated",&actor,&now)?;Ok(row)
        }).map_err(database_error)?;
        self.publish_change(&p.project_id, "project.updated");
        Ok(result)
    }
    pub fn update_checkout(
        &self,
        p: api::CheckoutUpdate,
        caller: &Caller,
    ) -> Result<api::Checkout, ProtocolError> {
        validate_id(&p.project_id)?;
        validate_id(&p.worktree_id)?;
        validate_metadata(
            &[&p.intent, &p.outcome, &p.owner, &p.next_action, &p.task_id],
            p.lifecycle.as_deref(),
        )?;
        if p.role.as_deref().is_some_and(|r| {
            !matches!(
                r,
                "primary" | "worktree" | "deployment" | "scratch" | "clone"
            )
        }) {
            return Err(invalid("checkout role is invalid"));
        }
        let scope = self.scope(caller)?;
        let now = timestamp();
        let actor = caller.actor();
        let result=self.database.transaction(|c|{
            let target=canonical_checkout_id(c,&p.project_id,&p.worktree_id)?;
            let (revision,physical,protected,raw)=c.query_row("SELECT revision,physical_repository_id,protected_role,record_json::text FROM project_checkouts WHERE project_id=$1 AND worktree_id=$2 FOR UPDATE",params![p.project_id,target],|r|Ok((r.get::<_,u32>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?))).optional()?.ok_or_else(not_found_db)?;
            if !scope.edits(&physical){return Err(denied("operator on this checkout's repository is required").into());}
            if revision!=p.expected_revision{return Err(conflict().into());}
            let mut row:api::Checkout=from_json(&raw)?;
            if row.immutable_generation&&p.lifecycle.is_some(){return Err(invalid("immutable deployment generation lifecycle is observed from its owner").into());}
            if let Some(role)=&p.role{if protected.as_ref().is_some_and(|v|v!=role)||matches!(role.as_str(),"primary"|"deployment"){return Err(invalid("primary and deployment roles are assigned from protected ownership and cannot be changed").into());}row.role=role.clone();}
            apply_patch(&mut row.intent,&p.intent);apply_patch(&mut row.outcome,&p.outcome);apply_patch(&mut row.owner,&p.owner);apply_patch(&mut row.next_action,&p.next_action);apply_patch(&mut row.task_id,&p.task_id);if let Some(lifecycle)=&p.lifecycle{row.lifecycle=lifecycle.clone();}row.revision=revision+1;validate_task(c,&p.project_id,row.task_id.as_deref())?;
            c.execute("UPDATE project_checkouts SET revision=$3,intent=$4,outcome=$5,owner=$6,next_action=$7,task_id=$8,lifecycle=$9,role=$10,record_json=$11::jsonb,updated_at=$12,updated_by=$13 WHERE project_id=$1 AND worktree_id=$2",params![p.project_id,target,row.revision,row.intent,row.outcome,row.owner,row.next_action,row.task_id,row.lifecycle,row.role,json(&row)?,now,actor])?;
            changed(c,&p.project_id,&actor,&now)?;event(c,&p.project_id,Some(&row.repository_id),Some(&physical),Some(&target),"checkout.updated","Checkout intent and lifecycle were updated",&actor,&now)?;redact_checkout(&mut row,&scope,&physical);Ok(row)
        }).map_err(database_error)?;
        self.publish_change(&p.project_id, "project.updated");
        Ok(result)
    }
    pub fn commits(
        &self,
        p: api::BranchReference,
        caller: &Caller,
    ) -> Result<api::CommitPage, ProtocolError> {
        validate_id(&p.project_id)?;
        validate_id(&p.branch_id)?;
        validate_limit(p.limit)?;
        let scope = self.scope(caller)?;
        let page = api::Page {
            project_id: p.project_id.clone(),
            cursor: p.cursor.clone(),
            limit: p.limit,
            query: None,
            lifecycle: None,
        };
        let after = decode_cursor(&page, &format!("commits:{}", p.branch_id))?;
        let position = if after.is_empty() {
            0
        } else {
            after
                .parse::<i64>()
                .map_err(|_| invalid("commit cursor is invalid"))?
        };
        self.database.read_transaction(|c|{
            let visible=scope.visible_json();let (repository_id,head)=c.query_row(&format!("SELECT b.repository_id,{BRANCH_HEAD} FROM project_branches b WHERE b.project_id=$1 AND b.branch_id=$3 AND {BRANCH_VISIBLE}"),params![p.project_id,visible,p.branch_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?))).optional()?.ok_or_else(not_found_db)?;
            let coverage=c.query_row("SELECT end_position,has_more,observed_at FROM project_commit_pages WHERE project_id=$1 AND branch_id=$2 AND after_position<=$3 AND end_position>=$3 AND head=$4 ORDER BY after_position DESC LIMIT 1",params![p.project_id,p.branch_id,position,head],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,bool>(1)?,r.get::<_,String>(2)?))).optional()?;
            let end=coverage.as_ref().map(|v|v.0).unwrap_or(position);
            let rows=c.prepare("SELECT position,record_json::text FROM project_commits WHERE project_id=$1 AND branch_id=$2 AND position>$3 AND position<=$5 ORDER BY position LIMIT $4")?.query_map(params![p.project_id,p.branch_id,position,i64::from(p.limit)+1,end],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<crate::database::Result<Vec<_>>>()?;
            let more=rows.len()>usize::from(p.limit);let mut commits=Vec::new();let mut last=position;
            for (pos,raw) in rows.into_iter().take(usize::from(p.limit)){commits.push(from_json(&raw)?);last=pos;}
            let has_more=more||coverage.as_ref().is_some_and(|v|v.1);
            let refresh_required=coverage.as_ref().is_none_or(|v|position==v.0&&v.1);
            Ok(api::CommitPage{repository_id,branch_id:p.branch_id.clone(),next_cursor:has_more.then(||encode_cursor(&page,&format!("commits:{}",p.branch_id),&last.to_string())),commits,refresh_required,refreshed_at:coverage.map(|v|v.2)})
        }).map_err(database_error)
    }
    pub fn refresh_commits_page(
        &self,
        p: api::CommitRefresh,
        caller: &Caller,
    ) -> Result<api::Job, ProtocolError> {
        validate_id(&p.project_id)?;
        validate_id(&p.branch_id)?;
        validate_id(&p.idempotency_key)?;
        let scope = self.scope(caller)?;
        let page = api::Page {
            project_id: p.project_id.clone(),
            cursor: p.cursor,
            limit: 100,
            query: None,
            lifecycle: None,
        };
        let after = decode_cursor(&page, &format!("commits:{}", p.branch_id))?;
        let offset = if after.is_empty() {
            0
        } else {
            after
                .parse::<u32>()
                .map_err(|_| invalid("commit cursor is invalid"))?
        };
        if offset > 1_000_000 {
            return Err(invalid("commit history position exceeds its fixed bound"));
        }
        let id = new_id("job")?;
        let now = timestamp();
        let job=self.database.transaction(|c|{
            if let Some(job)=load_job_optional(c,&p.project_id,None,Some((&p.idempotency_key,"commits")))?{return Ok(job);}
            let head=c.query_row(&format!("SELECT {BRANCH_HEAD} FROM project_branches b WHERE b.project_id=$1 AND b.branch_id=$3 AND {BRANCH_VISIBLE}"),params![p.project_id,scope.visible_json(),p.branch_id],|r|r.get::<_,Option<String>>(0)).optional()?.flatten().ok_or_else(not_found_db)?;
            let physical=c.prepare("SELECT m.physical_repository_id FROM project_repository_memberships m JOIN project_branches b ON b.project_id=m.project_id AND b.repository_id=m.repository_id WHERE m.project_id=$1 AND b.branch_id=$3 AND ($2::jsonb IS NULL OR m.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))) ORDER BY m.physical_repository_id LIMIT 256")?.query_map(params![p.project_id,scope.visible_json(),p.branch_id],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?;
            ensure_not_relocating(c,&physical)?;
            let pending=c.query_row("SELECT count(*) FROM project_jobs WHERE project_id=$1 AND state IN('queued','running')",[&p.project_id],|r|r.get::<_,u32>(0))?;if pending>=8{return Err(ProtocolError::new(ErrorCode::Busy,"project already has eight pending refreshes").into());}
            let cursor=refresh::CommitRequest{branch:p.branch_id.clone(),head,offset};
            c.execute("INSERT INTO project_jobs(job_id,project_id,kind,idempotency_key,state,authorized_repositories,cursor,queued_at,actor) VALUES($1,$2,'commits',$3,'queued',$4::jsonb,$5,$6,$7)",params![id,p.project_id,p.idempotency_key,json(&physical)?,json(&cursor)?,now,caller.actor()])?;
            load_job_optional(c,&p.project_id,Some(&id),None)?.ok_or_else(not_found_db)
        }).map_err(database_error)?;
        self.wake.notify_one();
        Ok(job)
    }
    pub fn refresh(&self, p: api::Refresh, caller: &Caller) -> Result<api::Job, ProtocolError> {
        validate_id(&p.project_id)?;
        validate_id(&p.idempotency_key)?;
        let scope = self.scope(caller)?;
        let id = new_id("job")?;
        let now = timestamp();
        let actor = caller.actor();
        let job=self.database.transaction(|c|{
            summary(c,&p.project_id,&scope)?;
            if let Some(job)=load_job_optional(c,&p.project_id,None,Some((&p.idempotency_key,"refresh")))?{return Ok(job);}
            // Each observer refreshes only its authorized physical repositories.
            let physical=c.prepare("SELECT physical_repository_id FROM project_repository_memberships WHERE project_id=$1 AND ($2::jsonb IS NULL OR physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))) ORDER BY physical_repository_id LIMIT 257")?.query_map(params![p.project_id,scope.visible_json()],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?;
            if physical.len()>256{return Err(invalid("refresh supports at most 256 physical repositories per project").into());}
            ensure_not_relocating(c,&physical)?;
            let pending=c.query_row("SELECT count(*) FROM project_jobs WHERE project_id=$1 AND state IN('queued','running')",[&p.project_id],|r|r.get::<_,i64>(0))?;
            if pending>=8{return Err(ProtocolError::new(ErrorCode::Busy,"project already has eight pending refreshes").into());}
            c.execute("INSERT INTO project_jobs(job_id,project_id,kind,idempotency_key,state,include_remote,authorized_repositories,queued_at,actor) VALUES($1,$2,'refresh',$3,'queued',$4,$5::jsonb,$6,$7)",params![id,p.project_id,p.idempotency_key,p.include_remote,json(&physical)?,now,actor])?;
            load_job_optional(c,&p.project_id,Some(&id),None)?.ok_or_else(not_found_db)
        }).map_err(database_error)?;
        self.wake.notify_one();
        Ok(job)
    }
    pub fn job(&self, p: api::JobReference, caller: &Caller) -> Result<api::Job, ProtocolError> {
        validate_id(&p.project_id)?;
        validate_id(&p.job_id)?;
        let scope = self.scope(caller)?;
        self.database.read_transaction(|c|{summary(c,&p.project_id,&scope)?;let permitted=c.query_row("SELECT EXISTS(SELECT 1 FROM project_jobs WHERE project_id=$1 AND job_id=$2 AND ($3::jsonb IS NULL OR authorized_repositories<@$3::jsonb))",params![p.project_id,p.job_id,scope.visible_json()],|r|r.get::<_,bool>(0))?;if !permitted{return Err(not_found_db());}load_job_optional(c,&p.project_id,Some(&p.job_id),None)?.ok_or_else(not_found_db)}).map_err(database_error)
    }
}

fn summary(c: &Connection, id: &str, scope: &Scope) -> Result<api::ProjectSummary, DatabaseError> {
    let visible = scope.visible_json();
    let identity=c.query_row("SELECT p.name,p.slug,p.revision FROM projects p WHERE p.project_id=$1 AND ($2::jsonb IS NULL OR EXISTS(SELECT 1 FROM project_repository_memberships m WHERE m.project_id=p.project_id AND m.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))))",params![id,visible],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u32>(2)?))).optional()?.ok_or_else(not_found_db)?;
    let repository_count = c.query_row(
        &format!(
            "SELECT count(*) FROM project_sources s WHERE s.project_id=$1 AND {SOURCE_VISIBLE}"
        ),
        params![id, visible],
        |r| r.get(0),
    )?;
    let checkout_count=c.query_row("SELECT count(*) FROM project_checkouts WHERE project_id=$1 AND ($2::jsonb IS NULL OR physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb)))",params![id,visible],|r|r.get(0))?;
    let branch_count = c.query_row(
        &format!(
            "SELECT count(*) FROM project_branches b WHERE b.project_id=$1 AND {BRANCH_VISIBLE}"
        ),
        params![id, visible],
        |r| r.get(0),
    )?;
    let storage = storage_summary(c, id, scope)?;
    Ok(api::ProjectSummary {
        project_id: id.into(),
        name: identity.0,
        slug: identity.1,
        revision: identity.2,
        repository_count,
        checkout_count,
        branch_count,
        local_bytes: storage.allocated_bytes,
        unmeasured_count: storage.unmeasured_count,
    })
}
fn load_sources(
    c: &Connection,
    id: &str,
    scope: &Scope,
) -> Result<Vec<api::Repository>, DatabaseError> {
    let rows=c.prepare(&format!("SELECT s.repository_id,s.name,s.github_url,s.default_branch,s.primary_checkout_id,s.refresh_state,s.last_success,s.error FROM project_sources s WHERE s.project_id=$1 AND {SOURCE_VISIBLE} ORDER BY s.repository_id LIMIT 257"))?.query_map(params![id,scope.visible_json()],|r|Ok(api::Repository{repository_id:r.get(0)?,name:r.get(1)?,github_url:r.get(2)?,default_branch:r.get(3)?,primary_checkout_id:r.get(4)?,refresh_state:r.get(5)?,last_success:r.get(6)?,github_observation:None,error:r.get(7)?,permissions:api::Permissions{edit:false,cleanup:false,admin:scope.admin},details_omitted:false}))?.collect::<crate::database::Result<Vec<_>>>()?;
    let mut result = Vec::new();
    for mut row in rows {
        row.github_observation =
            refresh::github_observation(c, id, &row.repository_id, row.github_url.as_deref())?;
        let editable = source_editable(c, id, &row.repository_id, scope)?;
        row.permissions.edit = editable;
        row.permissions.cleanup = editable;
        if !scope.admin {
            row.primary_checkout_id=row.primary_checkout_id.filter(|wid|c.query_row("SELECT physical_repository_id FROM project_checkouts WHERE project_id=$1 AND worktree_id=$2",params![id,wid],|r|r.get::<_,String>(0)).optional().ok().flatten().is_some_and(|phys|scope.sees(&phys)));
        }
        result.push(row);
    }
    Ok(result)
}
fn load_roots(c: &Connection, id: &str, scope: &Scope) -> Result<Vec<api::Root>, DatabaseError> {
    c.prepare("SELECT r.kind,r.label,r.absolute_path FROM project_roots r WHERE r.project_id=$1 AND ($2::jsonb IS NULL OR EXISTS(SELECT 1 FROM project_checkouts c WHERE c.project_id=r.project_id AND c.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb)) AND (c.absolute_path=r.absolute_path OR left(c.absolute_path,length(r.absolute_path)+1)=r.absolute_path||'/'))) ORDER BY r.kind,r.label,r.root_id LIMIT 64")?.query_map(params![id,scope.visible_json()],|r|{let path:String=r.get(2)?;Ok(api::Root{kind:r.get(0)?,label:r.get(1)?,relative_path:".".into(),absolute_path:scope.admin.then_some(path)})})?.collect::<crate::database::Result<_>>().map_err(DatabaseError::from)
}
fn source_editable(
    c: &Connection,
    id: &str,
    source: &str,
    scope: &Scope,
) -> Result<bool, DatabaseError> {
    if scope.admin {
        return Ok(true);
    }
    let physical=c.prepare("SELECT physical_repository_id FROM project_repository_memberships WHERE project_id=$1 AND repository_id=$2 LIMIT 257")?.query_map(params![id,source],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?;
    Ok(!physical.is_empty() && physical.len() <= 256 && physical.iter().all(|id| scope.edits(id)))
}
fn require_source_edit(
    c: &Connection,
    id: &str,
    source: &str,
    scope: &Scope,
) -> Result<(), DatabaseError> {
    if source_editable(c, id, source, scope)? {
        Ok(())
    } else {
        Err(denied("operator on each physical repository in this source is required").into())
    }
}
fn ensure_not_relocating(c: &Connection, physical: &[String]) -> Result<(), DatabaseError> {
    let pending=c.query_row("SELECT EXISTS(SELECT 1 FROM repository_relocations WHERE repository_id IN(SELECT jsonb_array_elements_text($1::jsonb)) AND state IN('prepared','verified'))",[json(&physical)?],|r|r.get::<_,bool>(0))?;
    if pending {
        Err(ProtocolError::new(
            ErrorCode::ConfigurationConflict,
            "a source has a prepared relocation; verify and commit or cancel it before refreshing",
        )
        .into())
    } else {
        Ok(())
    }
}
fn storage_summary(c: &Connection, id: &str, scope: &Scope) -> Result<api::Storage, DatabaseError> {
    let visible = scope.visible_json();
    let api::Storage{allocated_bytes:total,source_bytes:source,build_bytes:build,shared_bytes:shared,unmeasured_count:unmeasured,measured_at:measured}=c.query_row("SELECT sum(allocated_bytes)::bigint,sum(allocated_bytes) FILTER(WHERE kind='source')::bigint,sum(allocated_bytes) FILTER(WHERE kind='build')::bigint,sum(allocated_bytes) FILTER(WHERE kind='shared')::bigint,count(*) FILTER(WHERE allocated_bytes IS NULL),min(measured_at) FROM project_storage WHERE project_id=$1 AND ($2::jsonb IS NULL OR physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb)))",params![id,visible],|r|Ok(api::Storage{allocated_bytes:r.get(0)?,source_bytes:r.get(1)?,build_bytes:r.get(2)?,shared_bytes:r.get(3)?,unmeasured_count:r.get(4)?,measured_at:r.get(5)?}))?;
    let missing:u32=c.query_row("SELECT count(*) FROM project_checkouts c WHERE c.project_id=$1 AND c.exists_on_disk IS DISTINCT FROM false AND NOT coalesce((c.record_json->>'immutable_generation')::boolean,false) AND ($2::jsonb IS NULL OR c.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))) AND NOT EXISTS(SELECT 1 FROM project_storage s WHERE s.project_id=c.project_id AND s.worktree_id=c.worktree_id AND s.kind='source')",params![id,visible],|r|r.get(0))?;
    let (unknown_source,unknown_build,unknown_shared):(bool,bool,bool)=c.query_row("SELECT coalesce(bool_or(allocated_bytes IS NULL) FILTER(WHERE kind='source'),false),coalesce(bool_or(allocated_bytes IS NULL) FILTER(WHERE kind='build'),false),coalesce(bool_or(allocated_bytes IS NULL) FILTER(WHERE kind='shared'),false) FROM project_storage WHERE project_id=$1 AND ($2::jsonb IS NULL OR physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb)))",params![id,visible],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let missing_build:u32=c.query_row(&format!("SELECT count(*) FROM project_sources s WHERE s.project_id=$1 AND {SOURCE_VISIBLE} AND EXISTS(SELECT 1 FROM project_roots r WHERE r.project_id=s.project_id AND r.kind='build') AND NOT EXISTS(SELECT 1 FROM project_storage a WHERE a.project_id=s.project_id AND a.repository_id=s.repository_id AND a.kind='build' AND ($2::jsonb IS NULL OR a.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))))"),params![id,visible],|r|r.get(0))?;
    let unknown = unmeasured
        .saturating_add(missing)
        .saturating_add(missing_build);
    Ok(api::Storage {
        allocated_bytes: if unknown == 0 { total } else { None },
        source_bytes: if unknown_source || missing > 0 {
            None
        } else {
            source
        },
        build_bytes: if unknown_build || missing_build > 0 {
            None
        } else {
            build
        },
        shared_bytes: if unknown_shared { None } else { shared },
        unmeasured_count: unknown,
        measured_at: measured,
    })
}
fn branch_page(
    c: &Connection,
    p: &api::Page,
    scope: &Scope,
    target: Option<&str>,
) -> Result<api::BranchPage, DatabaseError> {
    let after = decode_cursor(p, "branch").map_err(DatabaseError::Domain)?;
    let rows=c.prepare(&format!("SELECT b.branch_id,b.record_json::text FROM project_branches b WHERE b.project_id=$1 AND {BRANCH_VISIBLE} AND b.branch_id>$3 AND ($4::text IS NULL OR strpos(lower(b.branch_name),lower($4))>0 OR strpos(lower(coalesce(b.intent,'')),lower($4))>0) AND ($5::text IS NULL OR b.lifecycle=$5) AND ($7::text IS NULL OR b.branch_id=$7) ORDER BY b.branch_id LIMIT $6"))?.query_map(params![p.project_id,scope.visible_json(),after,p.query,p.lifecycle,i64::from(p.limit)+1,target],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<crate::database::Result<Vec<_>>>()?;
    let more = rows.len() > usize::from(p.limit);
    let mut branches = Vec::new();
    for (_, raw) in rows.into_iter().take(usize::from(p.limit)) {
        let mut b: api::Branch = from_json(&raw)?;
        // Local facts are recomputed only from visible persisted observations.
        let heads=c.prepare("SELECT head,upstream,local_present FROM project_branch_observations WHERE project_id=$1 AND branch_id=$2 AND ($3::jsonb IS NULL OR physical_repository_id IN(SELECT jsonb_array_elements_text($3::jsonb))) ORDER BY local_present DESC,observed_at DESC,physical_repository_id LIMIT 257")?.query_map(params![p.project_id,b.branch_id,scope.visible_json()],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,bool>(2)?)))?.collect::<crate::database::Result<Vec<_>>>()?;
        b.local_present = heads.iter().any(|r| r.2);
        if let Some((head, upstream, _)) = heads.first() {
            b.head = head.clone();
            b.upstream = upstream.clone();
        } else {
            b.head = c.query_row(
                "SELECT remote_head FROM project_branches WHERE project_id=$1 AND branch_id=$2",
                params![p.project_id, b.branch_id],
                |r| r.get(0),
            )?;
            b.upstream = None;
        }
        let checkouts=c.prepare("SELECT record_json::text FROM project_checkouts WHERE project_id=$1 AND branch_id=$2 AND ($3::jsonb IS NULL OR physical_repository_id IN(SELECT jsonb_array_elements_text($3::jsonb))) ORDER BY worktree_id LIMIT 4097")?.query_map(params![p.project_id,b.branch_id,scope.visible_json()],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?;
        b.worktree_ids.clear();
        let (mut source, mut build, mut local) = (Some(0u64), Some(0u64), Some(0u64));
        if checkouts.is_empty() {
            source = None;
            build = None;
            local = None;
        }
        for raw in checkouts {
            let row: api::Checkout = from_json(&raw)?;
            b.worktree_ids.push(row.worktree_id);
            add_optional(&mut source, row.source_bytes);
            add_optional(&mut build, row.build_bytes);
            add_optional(&mut local, row.local_bytes);
        }
        b.source_bytes = source;
        b.build_bytes = build;
        b.local_bytes = local;
        b.cleanup_receipt=c.query_row("SELECT e.actor,c.worktree_id FROM project_events e JOIN project_checkouts c ON c.project_id=e.project_id AND c.worktree_id=e.target_id WHERE e.project_id=$1 AND c.repository_id=$2 AND c.branch_name=$3 AND c.head=$4 AND c.exists_on_disk IS FALSE AND e.kind='checkout_removed' AND ($5::jsonb IS NULL OR c.physical_repository_id IN(SELECT jsonb_array_elements_text($5::jsonb))) ORDER BY e.event_id DESC LIMIT 1",params![p.project_id,b.repository_id,b.branch_name,b.head,scope.visible_json()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?.and_then(|(actor,worktree_id)|actor.strip_prefix("storage:").map(|job|api::CleanupReceiptCandidate{job_id:job.into(),worktree_id}));
        branches.push(b);
    }
    if target.is_none() {
        for branch in &mut branches {
            compact_branch(branch);
        }
    }
    let mut truncated = more;
    while encoded_len(&branches) > PAGE_BYTES && branches.len() > 1 {
        branches.pop();
        truncated = true;
    }
    let next_cursor =
        truncated.then(|| encode_cursor(p, "branch", &branches.last().unwrap().branch_id));
    Ok(api::BranchPage {
        branches,
        next_cursor,
    })
}
fn checkout_page(
    c: &Connection,
    p: &api::Page,
    scope: &Scope,
    target: Option<&str>,
) -> Result<api::CheckoutPage, DatabaseError> {
    let after = decode_cursor(p, "checkout").map_err(DatabaseError::Domain)?;
    let rows=c.prepare("SELECT record_json::text,physical_repository_id FROM project_checkouts WHERE project_id=$1 AND ($2::jsonb IS NULL OR physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))) AND worktree_id>$3 AND ($4::text IS NULL OR strpos(lower(coalesce(branch_name,'')),lower($4))>0 OR strpos(lower(coalesce(intent,'')),lower($4))>0 OR strpos(lower(record_json->>'relative_path'),lower($4))>0) AND ($5::text IS NULL OR lifecycle=$5) AND ($7::text IS NULL OR worktree_id=$7) ORDER BY worktree_id LIMIT $6")?.query_map(params![p.project_id,scope.visible_json(),after,p.query,p.lifecycle,i64::from(p.limit)+1,target],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<crate::database::Result<Vec<_>>>()?;
    let more = rows.len() > usize::from(p.limit);
    let mut checkouts = Vec::new();
    for (raw, physical) in rows.into_iter().take(usize::from(p.limit)) {
        let mut row = from_json(&raw)?;
        redact_checkout(&mut row, scope, &physical);
        checkouts.push(row);
    }
    if target.is_none() {
        for checkout in &mut checkouts {
            compact_checkout(checkout);
        }
    }
    let mut truncated = more;
    while encoded_len(&checkouts) > PAGE_BYTES && checkouts.len() > 1 {
        checkouts.pop();
        truncated = true;
    }
    let next_cursor =
        truncated.then(|| encode_cursor(p, "checkout", &checkouts.last().unwrap().worktree_id));
    Ok(api::CheckoutPage {
        checkouts,
        next_cursor,
    })
}
fn redact_checkout(row: &mut api::Checkout, scope: &Scope, physical: &str) {
    row.permissions = api::Permissions {
        edit: scope.edits(physical),
        cleanup: scope.edits(physical),
        admin: scope.admin,
    };
    row.can_remove &= row.permissions.cleanup;
    if !scope.admin {
        row.absolute_path = None;
    }
    if !scope.admin && !row.protection_reasons.is_empty() {
        row.can_remove = false;
    }
}
fn history_page(
    c: &Connection,
    p: &api::Page,
    scope: &Scope,
) -> Result<api::History, DatabaseError> {
    let after = decode_cursor(p, "history").map_err(DatabaseError::Domain)?;
    let after = if after.is_empty() {
        i64::MAX
    } else {
        after
            .parse::<i64>()
            .map_err(|_| DatabaseError::Domain(invalid("history cursor is invalid")))?
    };
    let rows=c.prepare("SELECT e.event_id,e.repository_id,e.target_id,e.kind,e.summary,e.at FROM project_events e WHERE e.project_id=$1 AND e.event_id<$3 AND ($2::jsonb IS NULL OR (e.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))) OR (e.physical_repository_id IS NULL AND (e.repository_id IS NULL OR EXISTS(SELECT 1 FROM project_repository_memberships m WHERE m.project_id=e.project_id AND m.repository_id=e.repository_id AND m.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb)))))) ORDER BY e.event_id DESC LIMIT $4")?.query_map(params![p.project_id,scope.visible_json(),after,i64::from(p.limit)+1],|r|Ok(api::Event{event_id:r.get::<_,i64>(0)?.to_string(),repository_id:r.get(1)?,target_id:r.get(2)?,kind:r.get(3)?,summary:r.get(4)?,at:r.get(5)?}))?.collect::<crate::database::Result<Vec<_>>>()?;
    let more = rows.len() > usize::from(p.limit);
    let mut events = rows
        .into_iter()
        .take(usize::from(p.limit))
        .collect::<Vec<_>>();
    let mut truncated = more;
    while encoded_len(&events) > PAGE_BYTES && events.len() > 1 {
        events.pop();
        truncated = true;
    }
    let next_cursor =
        truncated.then(|| encode_cursor(p, "history", &events.last().unwrap().event_id));
    Ok(api::History {
        events,
        next_cursor,
    })
}
fn load_job_optional(
    c: &Connection,
    project: &str,
    id: Option<&str>,
    key: Option<(&str, &str)>,
) -> Result<Option<api::Job>, DatabaseError> {
    let row=c.query_row("SELECT job_id,project_id,kind,state,queued_at,started_at,finished_at,error,revision,result_json::text FROM project_jobs WHERE project_id=$1 AND ($2::text IS NULL OR job_id=$2) AND ($3::text IS NULL OR (idempotency_key=$3 AND kind=$4)) ORDER BY queued_at DESC LIMIT 1",params![project,id,key.map(|k|k.0),key.map(|k|k.1)],|r|Ok((api::Job{job_id:r.get(0)?,project_id:r.get(1)?,kind:r.get(2)?,state:r.get(3)?,queued_at:r.get(4)?,started_at:r.get(5)?,finished_at:r.get(6)?,error:r.get(7)?,revision:r.get(8)?,retirement:None},r.get::<_,Option<String>>(9)?))).optional()?;
    row.map(|(mut job, result)| {
        job.retirement = result.as_deref().map(from_json).transpose()?;
        Ok(job)
    })
    .transpose()
}
fn empty_checkout(
    id: &str,
    repository: &str,
    path: &str,
    relative_path: String,
    root_kind: String,
    role: &str,
) -> api::Checkout {
    api::Checkout {
        worktree_id: id.into(),
        repository_id: repository.into(),
        relative_path,
        root_kind,
        absolute_path: Some(path.into()),
        role: role.into(),
        branch_id: None,
        branch_name: None,
        head: None,
        dirty: None,
        exists: None,
        lifecycle: "active".into(),
        owner: None,
        intent: None,
        outcome: None,
        next_action: None,
        task_id: None,
        revision: 1,
        source_bytes: None,
        build_bytes: None,
        local_bytes: None,
        artifact_ids: Vec::new(),
        protection_reasons: if role == "primary" {
            vec!["primary checkout".into()]
        } else {
            vec!["checkout has not been inspected".into()]
        },
        can_remove: false,
        locations: Vec::new(),
        permissions: api::Permissions::default(),
        registration_state: "unregistered".into(),
        immutable_generation: false,
        deployment_references: Vec::new(),
        details_omitted: false,
    }
}
fn empty_branch(id: &str, source: &str, name: &str) -> api::Branch {
    api::Branch {
        branch_id: id.into(),
        repository_id: source.into(),
        branch_name: name.into(),
        head: None,
        upstream: None,
        remote_present: None,
        local_present: false,
        lifecycle: "unreviewed".into(),
        intent: None,
        outcome: None,
        owner: None,
        next_action: None,
        task_id: None,
        revision: 1,
        worktree_ids: Vec::new(),
        local_bytes: None,
        source_bytes: None,
        build_bytes: None,
        pr: None,
        merge_proof: None,
        cleanup_receipt: None,
        details_omitted: false,
    }
}
fn changed(c: &Connection, id: &str, actor: &str, now: &str) -> Result<(), DatabaseError> {
    c.execute(
        "UPDATE projects SET revision=revision+1,updated_at=$2,updated_by=$3 WHERE project_id=$1",
        params![id, now, actor],
    )?;
    Ok(())
}
/// Refreshes replace cached facts without editing the project's metadata.
/// Keep activity timestamps while preserving the revision held by editors and
/// removal reviews; their target-specific native checks still run separately.
fn observed(c: &Connection, id: &str, actor: &str, now: &str) -> Result<(), DatabaseError> {
    c.execute(
        "UPDATE projects SET updated_at=$2,updated_by=$3 WHERE project_id=$1",
        params![id, now, actor],
    )?;
    Ok(())
}
// Event ownership and actor fields mirror the append-only database projection.
#[allow(clippy::too_many_arguments)]
fn event(
    c: &Connection,
    project: &str,
    repository: Option<&str>,
    physical: Option<&str>,
    target: Option<&str>,
    kind: &str,
    summary: &str,
    actor: &str,
    now: &str,
) -> Result<(), DatabaseError> {
    c.execute("INSERT INTO project_events(project_id,repository_id,physical_repository_id,target_id,kind,summary,actor,at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",params![project,repository,physical,target,kind,summary,actor,now])?;
    Ok(())
}
fn display_path(roots: &[api::RootInput], path: &str) -> (String, String) {
    let path = Path::new(path);
    let root = roots
        .iter()
        .filter(|r| path.starts_with(&r.path))
        .max_by_key(|r| r.path.len());
    if let Some(root) = root {
        let relative = path.strip_prefix(&root.path).unwrap_or(path);
        (
            if relative.as_os_str().is_empty() {
                ".".into()
            } else {
                relative.to_string_lossy().into()
            },
            root.kind.clone(),
        )
    } else {
        (
            path.file_name()
                .map(|v| v.to_string_lossy().into())
                .unwrap_or_else(|| "checkout".into()),
            "source".into(),
        )
    }
}
fn add_optional(total: &mut Option<u64>, value: Option<u64>) {
    *total = total.and_then(|left| value.and_then(|right| left.checked_add(right)));
}
fn apply_patch<T: Clone>(target: &mut Option<T>, patch: &Option<Option<T>>) {
    if let Some(value) = patch {
        *target = value.clone();
    }
}
fn validate_task(c: &Connection, project: &str, task: Option<&str>) -> Result<(), DatabaseError> {
    if let Some(task) = task {
        let found=c.query_row("SELECT EXISTS(SELECT 1 FROM tasks t JOIN project_repository_memberships m ON m.physical_repository_id=t.repository_id WHERE m.project_id=$1 AND t.task_id=$2)",params![project,task],|r|r.get::<_,bool>(0))?;
        if !found {
            return Err(
                invalid("task must belong to a repository explicitly in this project").into(),
            );
        }
    }
    Ok(())
}
fn validate_metadata(
    fields: &[&Option<Option<String>>],
    lifecycle: Option<&str>,
) -> Result<(), ProtocolError> {
    for field in fields {
        if let Some(Some(value)) = field
            && (value.trim().is_empty() || value.len() > 2000 || value.contains('\0'))
        {
            return Err(invalid("metadata text must contain 1–2000 bytes"));
        }
    }
    if lifecycle.is_some_and(|l| {
        !matches!(
            l,
            "planned" | "active" | "paused" | "blocked" | "ready" | "abandoned"
        )
    }) {
        return Err(invalid(
            "merged and retired lifecycle values require observed proof; choose a declared lifecycle",
        ));
    }
    Ok(())
}
fn validate_configuration(p: &api::Configure) -> Result<(), ProtocolError> {
    validate_text(&p.name, 200)?;
    if p.slug.is_empty()
        || p.slug.len() > 100
        || !p
            .slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(invalid(
            "project slug must contain lowercase letters, digits, and hyphens",
        ));
    }
    validate_optional_id(p.project_id.as_deref())?;
    if p.roots.len() > 64 || p.repositories.is_empty() || p.repositories.len() > 128 {
        return Err(invalid("configure 1–128 repositories and at most 64 roots"));
    }
    for root in &p.roots {
        validate_text(&root.label, 200)?;
        if !matches!(root.kind.as_str(), "source" | "build")
            || !Path::new(&root.path).is_absolute()
            || root.path.len() > 4096
            || root.path.contains('\0')
            || Path::new(&root.path)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(invalid(
                "root must have a source/build kind and an absolute normalized path",
            ));
        }
    }
    for source in &p.repositories {
        validate_id(&source.repository_id)?;
        validate_text(&source.name, 200)?;
        if source.physical_repository_ids.is_empty()
            || source.physical_repository_ids.len() > 256
            || !source
                .physical_repository_ids
                .contains(&source.repository_id)
        {
            return Err(invalid(
                "physical members must include their existing canonical source identity",
            ));
        }
        for id in &source.physical_repository_ids {
            validate_id(id)?;
        }
        if let Some(url) = &source.github_url {
            github::GitHubRepository::from_url(url)
                .map_err(|_| invalid("source URL must identify an exact GitHub repository"))?;
        }
        if let Some(branch) = &source.default_branch {
            validate_text(branch, 512)?;
        }
    }
    Ok(())
}
fn validate_text(text: &str, max: usize) -> Result<(), ProtocolError> {
    if text.trim().is_empty() || text.len() > max || text.contains('\0') {
        Err(invalid(
            "text has an invalid length or contains a null byte",
        ))
    } else {
        Ok(())
    }
}
fn validate_id(id: &str) -> Result<(), ProtocolError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        Err(invalid(
            "identity must contain 1–128 letters, digits, or -_.:",
        ))
    } else {
        Ok(())
    }
}
fn validate_optional_id(id: Option<&str>) -> Result<(), ProtocolError> {
    id.map(validate_id).transpose().map(|_| ())
}
fn validate_query(query: Option<&str>) -> Result<(), ProtocolError> {
    if query.is_some_and(|q| q.len() > 200 || q.contains('\0')) {
        Err(invalid("query is limited to 200 bytes"))
    } else {
        Ok(())
    }
}
fn validate_limit(limit: u16) -> Result<(), ProtocolError> {
    if !(1..=100).contains(&limit) {
        Err(invalid("page limit must be between 1 and 100"))
    } else {
        Ok(())
    }
}
fn validate_page(p: &api::Page) -> Result<(), ProtocolError> {
    validate_id(&p.project_id)?;
    validate_limit(p.limit)?;
    validate_query(p.query.as_deref())?;
    if let Some(value) = &p.lifecycle {
        validate_text(value, 20)?;
    }
    Ok(())
}
fn decode_cursor(p: &api::Page, kind: &str) -> Result<String, ProtocolError> {
    let Some(encoded) = &p.cursor else {
        return Ok(String::new());
    };
    if encoded.len() > 2048 {
        return Err(invalid("page cursor is too long"));
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| invalid("page cursor is invalid"))?;
    let cursor: Cursor =
        serde_json::from_slice(&decoded).map_err(|_| invalid("page cursor is invalid"))?;
    if cursor.project != p.project_id
        || cursor.kind != kind
        || cursor.query != p.query
        || cursor.lifecycle != p.lifecycle
        || cursor.after.len() > 512
    {
        return Err(invalid(
            "page cursor belongs to a different project, collection, or filter",
        ));
    }
    Ok(cursor.after)
}
fn encode_cursor(p: &api::Page, kind: &str, after: &str) -> String {
    URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&Cursor {
            project: p.project_id.clone(),
            kind: kind.into(),
            after: after.into(),
            query: p.query.clone(),
            lifecycle: p.lifecycle.clone(),
        })
        .expect("typed cursor"),
    )
}
fn new_id(prefix: &str) -> Result<String, ProtocolError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        ProtocolError::new(
            ErrorCode::InternalError,
            "secure identity generation is unavailable",
        )
    })?;
    Ok(format!(
        "{prefix}-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
fn timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}
fn json<T: Serialize>(value: &T) -> Result<String, DatabaseError> {
    serde_json::to_string(value).map_err(|_| {
        DatabaseError::Domain(ProtocolError::new(
            ErrorCode::InternalError,
            "project record could not be encoded",
        ))
    })
}
fn from_json<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, DatabaseError> {
    serde_json::from_str(raw).map_err(|_| {
        DatabaseError::Domain(ProtocolError::new(
            ErrorCode::InternalError,
            "persisted project record is invalid",
        ))
    })
}
fn invalid(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::ParamsInvalid, message)
}
fn denied(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::PermissionDenied, message)
}
fn conflict() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::ConfigurationConflict,
        "the record changed; refresh before editing",
    )
}
fn not_found_db() -> DatabaseError {
    ProtocolError::new(
        ErrorCode::RepositoryNotFound,
        "project or project item is not visible or registered",
    )
    .into()
}
fn database_error(error: DatabaseError) -> ProtocolError {
    match error {
        DatabaseError::Domain(error) => error,
        _ => ProtocolError::new(
            ErrorCode::InternalError,
            "project authority database operation failed",
        ),
    }
}

// Leave room for the protocol envelope below its 256 KiB transport ceiling.
const PAGE_BYTES: usize = 180 * 1024;
const STATUS_BYTES: usize = 190 * 1024;
const INDEX_ROW_BYTES: usize = 24 * 1024;
fn encoded_len(value: &impl Serialize) -> usize {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .unwrap_or(usize::MAX)
}
fn preview(value: &mut String, max: usize) -> bool {
    if value.len() <= max {
        return false;
    }
    let mut end = max.saturating_sub(3);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str("...");
    true
}
fn preview_metadata(fields: [&mut Option<String>; 5]) -> bool {
    let mut omitted = false;
    for text in fields.into_iter().flatten() {
        omitted |= preview(text, 512);
    }
    omitted
}
fn compact_branch(row: &mut api::Branch) {
    row.details_omitted |= preview_metadata([
        &mut row.intent,
        &mut row.outcome,
        &mut row.owner,
        &mut row.next_action,
        &mut row.task_id,
    ]);
    if row.worktree_ids.len() > 64 {
        row.worktree_ids.truncate(64);
        row.details_omitted = true;
    }
    while encoded_len(row) > INDEX_ROW_BYTES && !row.worktree_ids.is_empty() {
        row.worktree_ids.pop();
        row.details_omitted = true;
    }
}
fn compact_checkout(row: &mut api::Checkout) {
    row.details_omitted |= preview_metadata([
        &mut row.intent,
        &mut row.outcome,
        &mut row.owner,
        &mut row.next_action,
        &mut row.task_id,
    ]);
    row.details_omitted |= preview(&mut row.relative_path, 512);
    if row.artifact_ids.len() > 64 {
        row.artifact_ids.truncate(64);
        row.details_omitted = true;
    }
    if row.locations.len() > 16 {
        row.locations.truncate(16);
        row.details_omitted = true;
    }
    for location in &mut row.locations {
        row.details_omitted |= preview(&mut location.relative_path, 512);
    }
    while encoded_len(row) > INDEX_ROW_BYTES
        && (!row.locations.is_empty() || !row.artifact_ids.is_empty())
    {
        if !row.locations.is_empty() {
            row.locations.pop();
        } else {
            row.artifact_ids.pop();
        }
        row.details_omitted = true;
    }
}
fn bound_status(status: &mut api::Status) {
    if encoded_len(status) > STATUS_BYTES {
        for root in &mut status.roots {
            root.absolute_path = None;
        }
        for row in &mut status.checkouts {
            if row.absolute_path.take().is_some() {
                row.details_omitted = true;
            }
        }
    }
    let page = api::Page {
        project_id: status.project.project_id.clone(),
        cursor: None,
        limit: 50,
        query: None,
        lifecycle: None,
    };
    while encoded_len(status) > STATUS_BYTES
        && (status.branches.len() > 1 || status.checkouts.len() > 1)
    {
        if status.branches.len() > 1
            && (status.checkouts.len() <= 1
                || encoded_len(&status.branches) >= encoded_len(&status.checkouts))
        {
            status.branches.pop();
            status.next_branch_cursor = status
                .branches
                .last()
                .map(|row| encode_cursor(&page, "branch", &row.branch_id));
        } else {
            status.checkouts.pop();
            status.next_checkout_cursor = status
                .checkouts
                .last()
                .map(|row| encode_cursor(&page, "checkout", &row.worktree_id));
        }
    }
    if encoded_len(status) > STATUS_BYTES {
        for root in &mut status.roots {
            preview(&mut root.label, 32);
            root.absolute_path = None;
        }
        for source in &mut status.repositories {
            preview(&mut source.name, 32);
            source.github_url = None;
            source.default_branch = None;
            source.error = None;
            source.details_omitted = true;
        }
    }
}

pub(crate) fn canonical_checkout_id(
    c: &Connection,
    project: &str,
    id: &str,
) -> Result<String, DatabaseError> {
    c.query_row("SELECT worktree_id FROM project_checkouts WHERE project_id=$1 AND worktree_id=$2 UNION ALL SELECT worktree_id FROM project_checkout_aliases WHERE project_id=$1 AND alias_worktree_id=$2 LIMIT 1",params![project,id],|r|r.get(0)).optional()?.ok_or_else(not_found_db)
}
fn rekey_project_checkout(
    c: &Connection,
    project: &str,
    old: &str,
    new: &str,
    now: &str,
    actor: &str,
) -> Result<(), DatabaseError> {
    if old == new {
        return Ok(());
    }
    let (path,physical)=c.query_row("SELECT absolute_path,physical_repository_id FROM project_checkouts WHERE project_id=$1 AND worktree_id=$2 FOR UPDATE",params![project,old],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?;
    let valid=c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE worktree_id=$1 AND repository_id=$2 AND worktree_path=$3) AND NOT EXISTS(SELECT 1 FROM project_checkouts WHERE project_id=$4 AND worktree_id=$1)",params![new,physical,path,project],|r|r.get::<_,bool>(0))?;
    if !valid {
        return Err(conflict().into());
    }
    c.execute("UPDATE project_checkouts SET worktree_id=$3,registration_state='registered',revision=revision+1,record_json=record_json||jsonb_build_object('worktree_id',$3::text,'registration_state','registered','revision',revision+1),updated_at=$4,updated_by=$5 WHERE project_id=$1 AND worktree_id=$2",params![project,old,new,now,actor])?;
    c.execute("INSERT INTO project_checkout_aliases(project_id,alias_worktree_id,worktree_id,created_at) VALUES($1,$2,$3,$4) ON CONFLICT(project_id,alias_worktree_id) DO UPDATE SET worktree_id=excluded.worktree_id",params![project,old,new,now])?;
    c.execute("UPDATE project_sources SET primary_checkout_id=$3 WHERE project_id=$1 AND primary_checkout_id=$2",params![project,old,new])?;
    c.execute(
        "UPDATE project_storage SET worktree_id=$3 WHERE project_id=$1 AND worktree_id=$2",
        params![project, old, new],
    )?;
    event(
        c,
        project,
        None,
        Some(&physical),
        Some(new),
        "checkout.identity_adopted",
        "Checkout identity was linked to the execution registry; historical links remain valid",
        actor,
        now,
    )
}
