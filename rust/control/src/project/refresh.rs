//! Resumable refresh units run only on the existing Coordinator maintenance lane.
use super::*;
use crate::maintenance::{Budget, JobResult, JobStatus};
use crate::repository::run_git_arguments;
use std::{path::PathBuf, time::Duration};

const GIT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REFS: usize = 4096;
const MAX_CHECKOUTS: usize = 4096;
const MAX_COMMITS: usize = 200;
#[derive(Default, Serialize, Deserialize)]
struct RefreshCursor {
    physical: usize,
    stage: String,
    after: String,
    remote_source: usize,
    remote_cursor: Option<String>,
    remote_after: String,
    had_error: bool,
    #[serde(default)]
    failed_sources: BTreeSet<String>,
}
struct Pending {
    id: String,
    project: String,
    physical: Vec<String>,
    remote: bool,
    actor: String,
    cursor: RefreshCursor,
}
#[derive(Serialize, Deserialize)]
pub(super) struct CommitRequest {
    pub branch: String,
    pub head: String,
    pub offset: u32,
}
#[derive(Clone)]
struct Physical {
    id: String,
    source: String,
    path: PathBuf,
    uid: u32,
}
#[derive(Debug, Clone, Default)]
struct Worktree {
    path: String,
    head: Option<String>,
    branch: Option<String>,
    locked: bool,
    prunable: bool,
    bare: bool,
}
#[derive(Debug)]
struct LocalBranch {
    name: String,
    head: String,
    upstream: Option<String>,
}
impl ProjectService {
    pub fn refresh_step(&self, budget: &mut Budget) -> JobResult {
        let result = self.refresh_one(budget);
        match result {
            Ok(rows) => JobResult {
                status: JobStatus::Complete,
                rows,
                bytes: budget.bytes(),
            },
            Err(_) => JobResult {
                status: JobStatus::Retry {
                    reason: "project_refresh_unavailable".into(),
                },
                rows: 0,
                bytes: 0,
            },
        }
    }
    fn refresh_one(&self, budget: &mut Budget) -> Result<u64, ProtocolError> {
        if budget.exhausted() {
            return Ok(0);
        }
        if self.run_retirement_job(budget)? {
            return Ok(1);
        }
        if self.run_commit_job(budget)? {
            return Ok(1);
        }
        let pending=self.database.transaction(|c|{
            let row=c.query_row("SELECT job_id,project_id,authorized_repositories::text,include_remote,actor,cursor FROM project_jobs WHERE kind='refresh' AND state IN('queued','running') ORDER BY queued_at,job_id LIMIT 1 FOR UPDATE SKIP LOCKED",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?))).optional()?;
            let Some((id,project,physical,remote,actor,cursor))=row else{return Ok(None);};
            c.execute("UPDATE project_jobs SET state='running',started_at=coalesce(started_at,$2),revision=revision+1 WHERE job_id=$1",params![id,timestamp()])?;
            Ok(Some(Pending{id,project,physical:from_json(&physical)?,remote,actor,cursor:cursor.as_deref().map(from_json).transpose()?.unwrap_or_default()}))
        }).map_err(database_error)?;
        let Some(mut p) = pending else {
            return Ok(0);
        };
        let attempt = (|| -> Result<(), ProtocolError> {
            if p.cursor.physical < p.physical.len() {
                let id = &p.physical[p.cursor.physical];
                let physical = self.physical(&p.project, id)?;
                self.verify_physical(&physical)?;
                match p.cursor.stage.as_str() {
                    "status" => {
                        if let Some(last) =
                            self.refresh_checkout(&p, &physical, &p.cursor.after, budget)?
                        {
                            p.cursor.after = last;
                        } else {
                            p.cursor.physical += 1;
                            p.cursor.stage.clear();
                            p.cursor.after.clear();
                        }
                    }
                    _ => {
                        self.refresh_inventory(&p, &physical, budget)?;
                        p.cursor.stage = "status".into();
                        p.cursor.after.clear();
                    }
                }
            } else if p.remote {
                self.refresh_remote(&mut p, budget)?;
            }
            Ok(())
        })();
        if let Err(error) = attempt {
            p.cursor.had_error = true;
            if p.cursor.physical < p.physical.len() {
                if let Ok(physical) = self.physical(&p.project, &p.physical[p.cursor.physical]) {
                    p.cursor.failed_sources.insert(physical.source);
                }
            } else if let Some((source, _)) = self.remote_sources(&p)?.get(p.cursor.remote_source) {
                p.cursor.failed_sources.insert(source.clone());
            }
            // Typed public failures never include subprocess output, paths, or credentials.
            let code = if error.code == ErrorCode::PermissionDenied {
                "source owner unavailable"
            } else {
                "source refresh unavailable; previous observations retained"
            };
            self.database.transaction(|c|{
                if p.cursor.physical<p.physical.len(){let physical=&p.physical[p.cursor.physical];c.execute("UPDATE project_sources SET refresh_state='failed',error=$3 WHERE project_id=$1 AND repository_id=(SELECT repository_id FROM project_repository_memberships WHERE project_id=$1 AND physical_repository_id=$2)",params![p.project,physical,code])?;}
                c.execute("UPDATE project_jobs SET error=$2 WHERE job_id=$1",params![p.id,code])?;Ok(())
            }).map_err(database_error)?;
            if p.cursor.physical < p.physical.len() {
                p.cursor.physical += 1;
                p.cursor.stage.clear();
                p.cursor.after.clear();
            } else {
                p.cursor.remote_source += 1;
                p.cursor.remote_cursor = None;
                p.cursor.remote_after.clear();
                p.cursor.stage.clear();
            }
        }
        let sources = self.remote_sources(&p)?;
        let finished = p.cursor.physical >= p.physical.len()
            && (!p.remote || p.cursor.remote_source >= sources.len());
        let now = timestamp();
        self.database.transaction(|c|{
            c.execute("UPDATE project_jobs SET cursor=$2,state=$3,finished_at=$4,revision=revision+1 WHERE job_id=$1",params![p.id,json(&p.cursor)?,if finished {if p.cursor.had_error{"partial"}else{"succeeded"}}else{"running"},finished.then_some(now.clone())])?;
            if finished {
                for (source,_) in &sources {
                    if p.cursor.failed_sources.contains(source){c.execute("UPDATE project_sources SET refresh_state='failed',error='One or more source observations failed; previous data retained' WHERE project_id=$1 AND repository_id=$2",params![p.project,source])?;}
                    else {c.execute("UPDATE project_sources SET last_success=$3,error=NULL WHERE project_id=$1 AND repository_id=$2",params![p.project,source,now])?;}
                }
                c.execute("UPDATE projects SET refreshed_at=$2,revision=revision+1,updated_at=$2 WHERE project_id=$1",params![p.project,now])?;
                event(c,&p.project,None,None,None,"project.refreshed",if p.cursor.had_error{"Refresh finished with unavailable source observations"}else{"Project source observations were refreshed"},&p.actor,&now)?;
                c.execute("DELETE FROM project_remote_observations WHERE job_id=$1",[&p.id])?;
            }
            Ok(())
        }).map_err(database_error)?;
        budget.consume(1, 0);
        if !finished {
            self.wake.notify_one();
        } else {
            self.publish_change(
                &p.project,
                if p.cursor.had_error {
                    "project.refresh.failed"
                } else {
                    "project.refreshed"
                },
            );
        }
        Ok(1)
    }
    fn physical(&self, project: &str, id: &str) -> Result<Physical, ProtocolError> {
        self.database.read(|c|{ensure_not_relocating(c,&[id.to_owned()])?;c.query_row("SELECT r.repository_id,m.repository_id,r.root_path,r.registered_by_uid FROM project_repository_memberships m JOIN repositories r ON r.repository_id=m.physical_repository_id WHERE m.project_id=$1 AND m.physical_repository_id=$2",params![project,id],|r|Ok(Physical{id:r.get(0)?,source:r.get(1)?,path:PathBuf::from(r.get::<_,String>(2)?),uid:r.get(3)?})).optional()?.ok_or_else(not_found_db)}).map_err(database_error)
    }
    fn verify_physical(&self, physical: &Physical) -> Result<(), ProtocolError> {
        if physical.uid == 0 {
            return Err(denied("a registered non-root source owner is required"));
        }
        let gid = crate::systemd::primary_gid(physical.uid)
            .map_err(|_| denied("registered source owner is unavailable"))?;
        // Registration verifies/binds the native identity, preserving existing
        // IDs. It also adopts a real primary checkout when only a linked one
        // had previously been registered. A reused path cannot bypass it.
        let current =
            Registry::new(self.database.clone()).register(&physical.path, physical.uid, gid)?;
        if current.repository_id != physical.id {
            return Err(ProtocolError::new(
                ErrorCode::ConfigurationConflict,
                "the registered physical source identity changed",
            ));
        }
        Ok(())
    }
    fn refresh_inventory(
        &self,
        p: &Pending,
        physical: &Physical,
        budget: &mut Budget,
    ) -> Result<(), ProtocolError> {
        let worktrees = parse_worktrees(&git(
            physical,
            &["worktree", "list", "--porcelain", "-z"],
            budget,
        )?)?;
        let refs = parse_refs(&git(
            physical,
            &[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(upstream:short)%00",
                "refs/heads/",
            ],
            budget,
        )?)?;
        if worktrees.len() > MAX_CHECKOUTS || refs.len() > MAX_REFS {
            return Err(invalid("source inventory exceeds its fixed refresh bound"));
        }
        let roots = self.root_inputs(&p.project)?;
        let now = timestamp();
        self.database.transaction(|c|{
            let mut branch_ids=BTreeMap::new();
            // A complete successful snapshot marks absent refs only for this physical clone.
            c.execute("UPDATE project_branch_observations SET local_present=false,observed_at=$3 WHERE project_id=$1 AND physical_repository_id=$2",params![p.project,physical.id,now])?;
            for branch in &refs {
                let mut row=load_or_create_branch(c,&p.project,&physical.source,&branch.name,&now,&p.actor)?;
                row.head=Some(branch.head.clone());row.upstream=branch.upstream.clone();row.local_present=true;row.revision+=1;
                persist_branch(c,&p.project,&row,&now,&p.actor)?;
                c.execute("INSERT INTO project_branch_observations(project_id,branch_id,physical_repository_id,head,upstream,local_present,observed_at) VALUES($1,$2,$3,$4,$5,true,$6) ON CONFLICT(project_id,branch_id,physical_repository_id) DO UPDATE SET head=excluded.head,upstream=excluded.upstream,local_present=true,observed_at=excluded.observed_at",params![p.project,row.branch_id,physical.id,branch.head,branch.upstream,now])?;
                branch_ids.insert(branch.name.clone(),row.branch_id);
            }
            let mut seen=BTreeSet::new();
            for worktree in &worktrees {
                if worktree.bare{continue;}
                let existing=c.query_row("SELECT worktree_id FROM project_checkouts WHERE project_id=$1 AND absolute_path=$2",params![p.project,worktree.path],|r|r.get::<_,String>(0)).optional()?;
                let registered=c.query_row("SELECT worktree_id FROM worktrees WHERE worktree_path=$1",[&worktree.path],|r|r.get::<_,String>(0)).optional()?;
                let wid=existing.or(registered).unwrap_or(new_id("checkout").map_err(DatabaseError::Domain)?);
                seen.insert(wid.clone());
                let (relative,kind)=display_path(&roots,&worktree.path);
                let mut row=load_checkout(c,&p.project,&wid)?.unwrap_or_else(||empty_checkout(&wid,&physical.source,&worktree.path,relative.clone(),kind.clone(),if Path::new(&worktree.path)==physical.path {if physical.id==physical.source{"primary"}else{"clone"}}else{"worktree"}));
                row.relative_path=relative;row.root_kind=kind;row.branch_name=worktree.branch.clone();row.branch_id=worktree.branch.as_ref().and_then(|name|branch_ids.get(name).cloned());row.head=worktree.head.clone();row.revision+=1;
                row.protection_reasons.clear();if worktree.locked{row.protection_reasons.push("Git worktree is locked".into());}if worktree.prunable{row.protection_reasons.push("Git reports a missing or prunable checkout".into());}
                let deployment=c.query_row("SELECT EXISTS(SELECT 1 FROM deployments WHERE worktree_id=$1 AND state NOT IN('removed','retired'))",[&wid],|r|r.get::<_,bool>(0))?;
                if deployment {if row.role!="primary" {row.role="deployment".into();}row.protection_reasons.push("checkout is a deployment source".into());}
                if row.role=="primary"{row.protection_reasons.push("primary checkout".into());}
                row.can_remove=false;
                persist_checkout(c,&p.project,&physical.id,&row,&now,&p.actor)?;
                if row.role=="primary"{c.execute("UPDATE project_sources SET primary_checkout_id=$3 WHERE project_id=$1 AND repository_id=$2",params![p.project,physical.source,wid])?;}
            }
            // Preserve registered and metadata-only checkouts missing from Git's list.
            let existing=c.prepare("SELECT worktree_id,record_json::text FROM project_checkouts WHERE project_id=$1 AND physical_repository_id=$2 LIMIT 4097")?.query_map(params![p.project,physical.id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<crate::database::Result<Vec<_>>>()?;
            for (wid,raw) in existing {if !seen.contains(&wid){let mut row:api::Checkout=from_json(&raw)?;row.protection_reasons.retain(|r|r!="checkout is absent from Git inventory");row.protection_reasons.push("checkout is absent from Git inventory".into());row.can_remove=false;row.revision+=1;persist_checkout(c,&p.project,&physical.id,&row,&now,&p.actor)?;}}
            c.execute("UPDATE project_sources SET refresh_state='local' WHERE project_id=$1 AND repository_id=$2",params![p.project,physical.source])?;
            changed(c,&p.project,&p.actor,&now)?;Ok(())
        }).map_err(database_error)
    }
    fn root_inputs(&self, id: &str) -> Result<Vec<api::RootInput>, ProtocolError> {
        self.database.read(|c|{c.prepare("SELECT kind,label,absolute_path FROM project_roots WHERE project_id=$1 ORDER BY root_id LIMIT 64")?.query_map([id],|r|Ok(api::RootInput{kind:r.get(0)?,label:r.get(1)?,path:r.get(2)?}))?.collect::<crate::database::Result<Vec<_>>>().map_err(DatabaseError::from)}).map_err(database_error)
    }
    fn refresh_checkout(
        &self,
        p: &Pending,
        physical: &Physical,
        after: &str,
        budget: &mut Budget,
    ) -> Result<Option<String>, ProtocolError> {
        let row=self.database.read(|c|{c.query_row("SELECT record_json::text FROM project_checkouts WHERE project_id=$1 AND physical_repository_id=$2 AND worktree_id>$3 ORDER BY worktree_id LIMIT 1",params![p.project,physical.id,after],|r|r.get::<_,String>(0)).optional().map_err(DatabaseError::from)}).map_err(database_error)?;
        let Some(raw) = row else {
            return Ok(None);
        };
        let mut row: api::Checkout = from_json(&raw).map_err(database_error)?;
        let path = PathBuf::from(
            row.absolute_path
                .as_deref()
                .ok_or_else(|| invalid("checkout path is unavailable"))?,
        );
        row.exists = Some(path.is_dir());
        if row.exists == Some(true) {
            let current = Physical {
                path: path.clone(),
                ..physical.clone()
            };
            let common = git(
                &current,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                budget,
            )?;
            let common = std::str::from_utf8(&common)
                .map_err(|_| invalid("checkout Git identity is invalid"))?;
            let common = Path::new(common.strip_suffix('\n').unwrap_or(common));
            let observed_identity = crate::storage::fs::identity(common)
                .map_err(|_| invalid("checkout Git identity is unavailable"))?;
            let expected_identity = crate::storage::fs::identity(&physical.path.join(".git"))
                .map_err(|_| invalid("registered Git identity is unavailable"))?;
            if observed_identity != expected_identity {
                return Err(ProtocolError::new(
                    ErrorCode::ConfigurationConflict,
                    "checkout belongs to a different physical Git source",
                ));
            }
            row.dirty = Some(
                !git(
                    &current,
                    &[
                        "status",
                        "--porcelain=v1",
                        "-z",
                        "--untracked-files=all",
                        "--ignore-submodules=none",
                    ],
                    budget,
                )?
                .is_empty(),
            );
        } else {
            row.dirty = None;
        }
        row.protection_reasons.retain(|r| {
            r != "checkout has not been inspected"
                && r != "checkout contains local changes"
                && r != "checkout is missing"
        });
        if row.dirty != Some(false) {
            row.protection_reasons.push(
                if row.exists == Some(false) {
                    "checkout is missing"
                } else {
                    "checkout contains local changes"
                }
                .into(),
            );
        }
        if row.head.is_none() || row.branch_name.is_none() {
            row.protection_reasons
                .push("detached or unresolved checkout".into());
        }
        row.can_remove = row.protection_reasons.is_empty()
            && row.dirty == Some(false)
            && row.exists == Some(true)
            && !matches!(row.role.as_str(), "primary" | "deployment");
        row.revision += 1;
        let now = timestamp();
        let roots = self.root_inputs(&p.project)?;
        self.attach_storage(&p.project, &p.id, physical, &mut row, &roots, &now, budget)?;
        let wid = row.worktree_id.clone();
        self.database
            .transaction(|c| {
                persist_checkout(c, &p.project, &physical.id, &row, &now, &p.actor)?;
                changed(c, &p.project, &p.actor, &now)
            })
            .map_err(database_error)?;
        Ok(Some(wid))
    }
    fn run_commit_job(&self, budget: &mut Budget) -> Result<bool, ProtocolError> {
        let job=self.database.transaction(|c|{
            let row=c.query_row("SELECT job_id,project_id,authorized_repositories::text,cursor FROM project_jobs WHERE kind='commits' AND state IN('queued','running') ORDER BY queued_at,job_id LIMIT 1 FOR UPDATE SKIP LOCKED",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()?;
            if let Some((id,_,_,_))=&row{c.execute("UPDATE project_jobs SET state='running',started_at=coalesce(started_at,$2),revision=revision+1 WHERE job_id=$1",params![id,timestamp()])?;}Ok(row)
        }).map_err(database_error)?;
        let Some((id, project, physical, request)) = job else {
            return Ok(false);
        };
        let request: CommitRequest = from_json(&request).map_err(database_error)?;
        let physical_ids: Vec<String> = from_json(&physical).map_err(database_error)?;
        let result = (|| -> Result<(), ProtocolError> {
            if !valid_sha(&request.head) {
                return Err(invalid("commit request head is invalid"));
            }
            let authorized = json(&physical_ids).map_err(database_error)?;
            let (source,url,current_head)=self.database.read(|c|{ensure_not_relocating(c,&physical_ids)?;c.query_row("SELECT b.repository_id,s.github_url,coalesce((SELECT o.head FROM project_branch_observations o WHERE o.project_id=b.project_id AND o.branch_id=b.branch_id AND o.physical_repository_id IN(SELECT jsonb_array_elements_text($3::jsonb)) ORDER BY o.local_present DESC,o.observed_at DESC,o.physical_repository_id LIMIT 1),b.remote_head) FROM project_branches b JOIN project_sources s USING(project_id,repository_id) WHERE b.project_id=$1 AND b.branch_id=$2",params![project,request.branch,authorized],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))).optional()?.ok_or_else(not_found_db)}).map_err(database_error)?;
            if current_head.as_deref() != Some(&request.head) {
                return Err(conflict());
            }
            let mut local = None;
            for pid in &physical_ids {
                let observed=self.database.read(|c|c.query_row("SELECT EXISTS(SELECT 1 FROM project_branch_observations WHERE project_id=$1 AND branch_id=$2 AND physical_repository_id=$3 AND head=$4 AND local_present)",params![project,request.branch,pid,request.head],|r|r.get::<_,bool>(0)).map_err(DatabaseError::from)).map_err(database_error)?;
                if observed {
                    local = Some(self.physical(&project, pid)?);
                    break;
                }
            }
            let (mut commits, more, next_remote) = if let Some(physical) = local {
                self.verify_physical(&physical)?;
                let output = git(
                    &physical,
                    &[
                        "log",
                        "-z",
                        "--format=tformat:%H%x00%P%x00%s%x00%an%x00%aI",
                        &format!("--max-count={}", MAX_COMMITS + 1),
                        &format!("--skip={}", request.offset),
                        "--end-of-options",
                        &request.head,
                        "--",
                    ],
                    budget,
                )?;
                let rows = parse_commits(&output)?;
                let more = rows.len() > MAX_COMMITS;
                (rows, more, None)
            } else {
                let remote = github::GitHubRepository::from_url(
                    url.as_deref()
                        .ok_or_else(|| invalid("no local or GitHub commit source is available"))?,
                )
                .map_err(|_| invalid("commit source is invalid"))?;
                let cursor = if request.offset == 0 {
                    None
                } else {
                    self.database.read(|c|c.query_row("SELECT remote_cursor FROM project_commit_pages WHERE project_id=$1 AND branch_id=$2 AND head=$3 AND end_position=$4 AND has_more ORDER BY after_position DESC LIMIT 1",params![project,request.branch,request.head,request.offset],|r|r.get::<_,Option<String>>(0)).optional().map_err(DatabaseError::from)).map_err(database_error)?.flatten()
                };
                if request.offset > 0 && cursor.is_none() {
                    return Err(invalid(
                        "remote commit continuation is not cached; refresh the preceding page",
                    ));
                }
                let client = self
                    .github
                    .as_ref()
                    .ok_or_else(|| denied("GitHub access is not configured"))?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| invalid("remote I/O runtime is unavailable"))?;
                let page = remote_call(
                    &runtime,
                    budget,
                    client.commits(&remote, &request.head, cursor.as_deref(), 100),
                )?;
                (
                    page.items
                        .into_iter()
                        .map(|c| api::Commit {
                            sha: c.sha,
                            parents: c.parent_shas,
                            subject: c.subject,
                            author: c.author,
                            authored_at: c.authored_at,
                        })
                        .collect(),
                    !page.complete,
                    page.next_cursor,
                )
            };
            commits.truncate(MAX_COMMITS);
            let now = timestamp();
            self.database.transaction(|c|{
                let current=c.query_row("SELECT coalesce((SELECT o.head FROM project_branch_observations o WHERE o.project_id=b.project_id AND o.branch_id=b.branch_id AND o.physical_repository_id IN(SELECT jsonb_array_elements_text($3::jsonb)) ORDER BY o.local_present DESC,o.observed_at DESC,o.physical_repository_id LIMIT 1),b.remote_head) FROM project_branches b WHERE b.project_id=$1 AND b.branch_id=$2 FOR UPDATE",params![project,request.branch,authorized],|r|r.get::<_,Option<String>>(0))?;if current.as_deref()!=Some(&request.head){return Err(conflict().into());}
                if request.offset==0{c.execute("DELETE FROM project_commits WHERE project_id=$1 AND branch_id=$2",params![project,request.branch])?;c.execute("DELETE FROM project_commit_pages WHERE project_id=$1 AND branch_id=$2",params![project,request.branch])?;}
                for (position,commit) in commits.iter().enumerate(){c.execute("INSERT INTO project_commits(project_id,branch_id,sha,position,record_json,observed_at) VALUES($1,$2,$3,$4,$5::jsonb,$6) ON CONFLICT(project_id,branch_id,sha) DO UPDATE SET position=excluded.position,record_json=excluded.record_json,observed_at=excluded.observed_at",params![project,request.branch,commit.sha,i64::from(request.offset)+position as i64+1,json(commit)?,now])?;}
                c.execute("INSERT INTO project_commit_pages(project_id,branch_id,after_position,head,end_position,has_more,remote_cursor,observed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(project_id,branch_id,after_position) DO UPDATE SET head=excluded.head,end_position=excluded.end_position,has_more=excluded.has_more,remote_cursor=excluded.remote_cursor,observed_at=excluded.observed_at",params![project,request.branch,request.offset,request.head,i64::from(request.offset)+commits.len() as i64,more,next_remote,now])?;
                changed(c,&project,"project-refresh",&now)?;event(c,&project,Some(&source),None,Some(&request.branch),"commits.refreshed","A bounded commit history page was refreshed","project-refresh",&now)
            }).map_err(database_error)
        })();
        self.database.transaction(|c|{c.execute("UPDATE project_jobs SET state=$2,error=$3,finished_at=$4,revision=revision+1 WHERE job_id=$1",params![id,if result.is_ok(){"succeeded"}else{"failed"},result.as_ref().err().map(|_|"commit history refresh unavailable; cached page retained"),timestamp()])?;Ok(())}).map_err(database_error)?;
        budget.consume(1, 0);
        self.publish_change(
            &project,
            if result.is_ok() {
                "project.refreshed"
            } else {
                "project.refresh.failed"
            },
        );
        Ok(true)
    }
    fn remote_sources(&self, p: &Pending) -> Result<Vec<(String, Option<String>)>, ProtocolError> {
        let physical =
            serde_json::to_string(&p.physical).map_err(|_| invalid("refresh scope is invalid"))?;
        self.database.read(|c|{c.prepare("SELECT s.repository_id,s.github_url FROM project_sources s WHERE s.project_id=$1 AND EXISTS(SELECT 1 FROM project_repository_memberships m WHERE m.project_id=s.project_id AND m.repository_id=s.repository_id AND m.physical_repository_id IN(SELECT jsonb_array_elements_text($2::jsonb))) ORDER BY s.repository_id LIMIT 129")?.query_map(params![p.project,physical],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<crate::database::Result<_>>().map_err(DatabaseError::from)}).map_err(database_error)
    }
    fn refresh_remote(&self, p: &mut Pending, budget: &mut Budget) -> Result<(), ProtocolError> {
        let sources = self.remote_sources(p)?;
        let Some((source, url)) = sources.get(p.cursor.remote_source) else {
            return Ok(());
        };
        let Some(url) = url else {
            p.cursor.remote_source += 1;
            p.cursor.stage.clear();
            return Ok(());
        };
        let remote = github::GitHubRepository::from_url(url)
            .map_err(|_| invalid("source remote is invalid"))?;
        let github = self
            .github
            .as_ref()
            .ok_or_else(|| denied("GitHub access is not configured"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| invalid("remote I/O runtime is unavailable"))?;
        if p.cursor.stage == "pull_requests" {
            let branch=self.database.read(|c|{c.query_row("SELECT record_json::text FROM project_branches WHERE project_id=$1 AND repository_id=$2 AND branch_id>$3 ORDER BY branch_id LIMIT 1",params![p.project,source,p.cursor.remote_after],|r|r.get::<_,String>(0)).optional().map_err(DatabaseError::from)}).map_err(database_error)?;
            let Some(raw) = branch else {
                p.cursor.remote_source += 1;
                p.cursor.remote_cursor = None;
                p.cursor.remote_after.clear();
                p.cursor.stage.clear();
                return Ok(());
            };
            let mut branch: api::Branch = from_json(&raw).map_err(database_error)?;
            let page = remote_call(
                &runtime,
                budget,
                github.pull_requests(&remote, &branch.branch_name, None, 100),
            )?;
            if !page.complete {
                return Err(ProtocolError::new(
                    ErrorCode::Busy,
                    "pull request history exceeds this refresh step; previous proof retained",
                ));
            }
            if let Some(pr) = page
                .items
                .into_iter()
                .find(|pr| Some(&pr.head_sha) == branch.head.as_ref())
            {
                branch.pr = Some(api::PullRequest {
                    number: pr.number,
                    url: pr.html_url,
                    state: pr.state,
                    merged: pr.merged,
                    head_sha: pr.head_sha.clone(),
                    merge_commit_sha: pr.merge_commit_sha.clone(),
                });
                if pr.merged && budget.remaining() > Duration::from_millis(500) {
                    if let Ok(proof) = remote_call(
                        &runtime,
                        budget,
                        github.verify_integration(
                            &remote,
                            &branch.branch_name,
                            &pr.head_sha,
                            pr.number,
                            None,
                        ),
                    ) {
                        for physical in &p.physical {
                            if self
                                .physical(&p.project, physical)
                                .is_ok_and(|member| member.source == *source)
                            {
                                let _ = self.record_checkout_integration_from_refresh(
                                    &p.project, physical, &proof, &p.actor,
                                );
                            }
                        }
                        branch.merge_proof = Some(api::MergeProof {
                            kind: "github_pull_request".into(),
                            head_sha: proof.head_sha().into(),
                            merged_into: proof.integration_branch().into(),
                            observed_at: proof.observed_at().into(),
                        });
                        branch.lifecycle = "merged".into();
                    }
                }
            }
            branch.revision += 1;
            let now = timestamp();
            self.database
                .transaction(|c| {
                    persist_branch(c, &p.project, &branch, &now, &p.actor)?;
                    changed(c, &p.project, &p.actor, &now)
                })
                .map_err(database_error)?;
            p.cursor.remote_after = branch.branch_id;
        } else {
            if p.cursor.remote_cursor.is_none() {
                let info = remote_call(&runtime, budget, github.repository(&remote))?;
                self.database.transaction(|c|{c.execute("UPDATE project_sources SET default_branch=$3,refresh_state='remote' WHERE project_id=$1 AND repository_id=$2",params![p.project,source,info.default_branch])?;Ok(())}).map_err(database_error)?;
            }
            let page = remote_call(
                &runtime,
                budget,
                github.list_branches(&remote, p.cursor.remote_cursor.as_deref(), 100),
            )?;
            let now = timestamp();
            self.database.transaction(|c|{
                let previous:u32=c.query_row("SELECT count(*) FROM project_remote_observations WHERE job_id=$1 AND repository_id=$2",params![p.id,source],|r|r.get(0))?;
                if previous as usize+page.items.len()>MAX_REFS{return Err(invalid("remote branch inventory exceeds its fixed refresh bound").into());}
                for item in &page.items {
                    let mut row=load_or_create_branch(c,&p.project,source,&item.name,&now,&p.actor)?;row.remote_present=Some(true);if !row.local_present{row.head=Some(item.head_sha.clone());}row.revision+=1;persist_branch(c,&p.project,&row,&now,&p.actor)?;
                    c.execute("UPDATE project_branches SET remote_head=$3 WHERE project_id=$1 AND branch_id=$2",params![p.project,row.branch_id,item.head_sha])?;
                    c.execute("INSERT INTO project_remote_observations(job_id,repository_id,branch_name,head,record_json) VALUES($1,$2,$3,$4,$5::jsonb) ON CONFLICT(job_id,repository_id,branch_name) DO UPDATE SET head=excluded.head,record_json=excluded.record_json",params![p.id,source,item.name,item.head_sha,json(&row)?])?;
                }
                if page.complete {
                    let absent=c.prepare("SELECT record_json::text FROM project_branches b WHERE project_id=$1 AND repository_id=$2 AND NOT EXISTS(SELECT 1 FROM project_remote_observations o WHERE o.job_id=$3 AND o.repository_id=b.repository_id AND o.branch_name=b.branch_name) LIMIT 4097")?.query_map(params![p.project,source,p.id],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?;
                    for raw in absent{let mut row:api::Branch=from_json(&raw)?;row.remote_present=Some(false);row.revision+=1;persist_branch(c,&p.project,&row,&now,&p.actor)?;}
                    c.execute("UPDATE project_sources SET refresh_state='fresh' WHERE project_id=$1 AND repository_id=$2",params![p.project,source])?;
                }
                changed(c,&p.project,&p.actor,&now)
            }).map_err(database_error)?;
            p.cursor.remote_cursor = page.next_cursor;
            if page.complete {
                p.cursor.stage = "pull_requests".into();
                p.cursor.remote_after.clear();
            }
        }
        Ok(())
    }
    fn attach_storage(
        &self,
        project: &str,
        job_id: &str,
        physical: &Physical,
        row: &mut api::Checkout,
        roots: &[api::RootInput],
        now: &str,
        budget: &Budget,
    ) -> Result<(), ProtocolError> {
        use crate::storage::model::{Locator, Record};
        use devcoordinator2_api::storage::Kind;
        let records=self.database.read(|c|{c.prepare("SELECT record_json::text FROM storage_artifacts WHERE repository_id=$1 AND removed_at_ms IS NULL ORDER BY artifact_id LIMIT 4097")?.query_map([&physical.id],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>().map_err(DatabaseError::from)}).map_err(database_error)?;
        if records.len() > 4096 {
            return Err(invalid(
                "storage membership exceeds its fixed refresh bound",
            ));
        }
        let checkout = Path::new(
            row.absolute_path
                .as_deref()
                .ok_or_else(|| invalid("checkout path is unavailable"))?,
        );
        let mut candidates = Vec::new();
        let mut root_measurement = None;
        for raw in records {
            let record: Record = from_json(&raw).map_err(database_error)?;
            if let Locator::Directory { path, .. } = &record.locator {
                if path == checkout && record.artifact.kind == Kind::Worktree {
                    root_measurement = Some(record.clone());
                }
                if path.starts_with(checkout)
                    && path != checkout
                    && matches!(
                        record.artifact.kind,
                        Kind::BuildOutput | Kind::DependencyCache
                    )
                {
                    candidates.push((path.clone(), record));
                }
            }
        }
        // Retain one shallow non-overlapping directory per allocation identity.
        candidates.sort_by_key(|(path, _)| path.components().count());
        let mut selected: Vec<(PathBuf, Record)> = Vec::new();
        let mut accounting = BTreeSet::new();
        for (path, record) in candidates {
            let key = if record.artifact.accounting_id.is_empty() {
                record.resource_key.clone()
            } else {
                record.artifact.accounting_id.clone()
            };
            if !accounting.insert(key)
                || selected.iter().any(|(parent, _)| path.starts_with(parent))
            {
                continue;
            }
            selected.push((path, record));
        }
        row.artifact_ids.clear();
        row.locations.clear();
        let mut build = if selected.is_empty() {
            None
        } else {
            Some(0u64)
        };
        for (path, record) in &selected {
            let (relative, kind) = display_path(roots, &path.to_string_lossy());
            row.locations.push(api::Location {
                kind: "build".into(),
                relative_path: relative,
                root_kind: kind,
                allocated_bytes: record.artifact.allocated_bytes,
                artifact_id: Some(record.artifact.artifact_id.clone()),
            });
            row.artifact_ids.push(record.artifact.artifact_id.clone());
            add_optional(&mut build, record.artifact.allocated_bytes);
        }
        let observed = if row.exists == Some(true) {
            bounded_measure(checkout, budget)
        } else {
            None
        };
        let total = match observed.as_ref() {
            Some(m) if !m.multiply_linked => Some(m.bytes),
            Some(_) => None,
            None => root_measurement
                .as_ref()
                .and_then(|r| r.artifact.allocated_bytes),
        };
        let source_measured_at = if observed.is_some() {
            Some(now.to_owned())
        } else {
            root_measurement
                .as_ref()
                .and_then(|r| measurement_time(r.artifact.verified_at_ms))
        };
        let common = physical.path.join(".git");
        let common_identity = crate::storage::fs::identity(&common).ok();
        let shared_key = common_identity
            .map(|(dev, ino)| format!("shared-git-{dev:x}-{ino:x}"))
            .unwrap_or_else(|| format!("shared-git-{}", physical.id));
        let cached_shared=self.database.read(|c|c.query_row("SELECT s.allocated_bytes,s.measured_at FROM project_storage s JOIN project_jobs j ON j.job_id=$4 WHERE s.project_id=$1 AND s.physical_repository_id=$2 AND s.kind='shared' AND s.accounting_id=$3 AND s.measured_at>=j.started_at ORDER BY s.measured_at DESC LIMIT 1",params![project,physical.id,shared_key,job_id],|r|Ok((r.get::<_,Option<u64>>(0)?,r.get::<_,Option<String>>(1)?))).optional().map_err(DatabaseError::from)).map_err(database_error)?;
        let (shared_bytes, shared_measured_at) = if let Some(cached) = cached_shared {
            cached
        } else {
            let measured = bounded_measure(&common, budget);
            (
                measured
                    .as_ref()
                    .and_then(|m| (!m.multiply_linked).then_some(m.bytes)),
                measured.map(|_| now.to_owned()),
            )
        };
        row.build_bytes = build;
        // The common Git allocation is a repository bucket, never a branch
        // allocation. Existing build records are subtracted exactly once.
        let includes_git = checkout == physical.path;
        let local = if includes_git {
            total.and_then(|t| shared_bytes.and_then(|g| t.checked_sub(g)))
        } else {
            total
        };
        row.source_bytes = local.and_then(|t| {
            if selected.is_empty() {
                Some(t)
            } else {
                build.and_then(|b| t.checked_sub(b))
            }
        });
        row.local_bytes = local;
        if let Some(root) = &root_measurement {
            row.artifact_ids.push(root.artifact.artifact_id.clone());
        }
        let source_accounting = observed
            .as_ref()
            .map(|m| format!("source-fs-{:x}-{:x}", m.device, m.inode))
            .or_else(|| {
                root_measurement.as_ref().map(|r| {
                    if r.artifact.accounting_id.is_empty() {
                        format!("source-{}", r.resource_key)
                    } else {
                        format!("source-{}", r.artifact.accounting_id)
                    }
                })
            })
            .unwrap_or_else(|| format!("unmeasured-{}", row.worktree_id));
        self.database.transaction(|c|{
            c.execute("DELETE FROM project_storage WHERE project_id=$1 AND worktree_id=$2",params![project,row.worktree_id])?;
            c.execute("INSERT INTO project_storage(project_id,repository_id,physical_repository_id,accounting_id,artifact_id,worktree_id,kind,allocated_bytes,measured_at) VALUES($1,$2,$3,$4,$5,$6,'source',$7,$8) ON CONFLICT(project_id,accounting_id) DO NOTHING",params![project,physical.source,physical.id,source_accounting,root_measurement.as_ref().map(|r|r.artifact.artifact_id.as_str()).unwrap_or(""),row.worktree_id,row.source_bytes,source_measured_at])?;
            for (_,record) in &selected {let key=if record.artifact.accounting_id.is_empty(){record.resource_key.clone()}else{record.artifact.accounting_id.clone()};c.execute("INSERT INTO project_storage(project_id,repository_id,physical_repository_id,accounting_id,artifact_id,worktree_id,kind,allocated_bytes,measured_at) VALUES($1,$2,$3,$4,$5,$6,'build',$7,$8) ON CONFLICT(project_id,accounting_id) DO NOTHING",params![project,physical.source,physical.id,key,record.artifact.artifact_id,row.worktree_id,record.artifact.allocated_bytes,measurement_time(record.artifact.verified_at_ms)])?;}
            c.execute("DELETE FROM project_storage WHERE project_id=$1 AND physical_repository_id=$2 AND kind='shared' AND accounting_id<>$3",params![project,physical.id,shared_key])?;
            c.execute("INSERT INTO project_storage(project_id,repository_id,physical_repository_id,accounting_id,artifact_id,kind,allocated_bytes,measured_at) VALUES($1,$2,$3,$4,'','shared',$5,$6) ON CONFLICT(project_id,accounting_id) DO UPDATE SET allocated_bytes=excluded.allocated_bytes,measured_at=excluded.measured_at",params![project,physical.source,physical.id,shared_key,shared_bytes,shared_measured_at])?;
            Ok(())
        }).map_err(database_error)?;
        Ok(())
    }
}
fn bounded_measure(path: &Path, budget: &Budget) -> Option<crate::storage::fs::Measurement> {
    let remaining = budget.remaining().min(Duration::from_millis(250));
    if remaining < Duration::from_millis(5) {
        return None;
    }
    crate::storage::fs::MeasurementCursor::new(path)
        .ok()?
        .step(100_000, remaining)
        .ok()
        .flatten()
}
fn measurement_time(ms: Option<u64>) -> Option<String> {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms?) * 1_000_000)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn git(physical: &Physical, args: &[&str], budget: &Budget) -> Result<Vec<u8>, ProtocolError> {
    if physical.uid == 0 {
        return Err(denied("a registered non-root source owner is required"));
    }
    let gid = crate::systemd::primary_gid(physical.uid)
        .map_err(|_| denied("registered source owner is unavailable"))?;
    if budget.remaining() < Duration::from_millis(50) {
        return Err(ProtocolError::new(
            ErrorCode::Busy,
            "refresh step reached its time budget",
        ));
    }
    let output = run_git_arguments(
        &physical.path,
        Some((physical.uid, gid)),
        args,
        GIT_TIMEOUT.min(budget.remaining()),
    )
    .map_err(|_| ProtocolError::new(ErrorCode::InternalError, "Git observation unavailable"))?;
    if !output.status.success() {
        return Err(ProtocolError::new(
            ErrorCode::InternalError,
            "Git observation failed; previous snapshot retained",
        ));
    }
    Ok(output.stdout)
}
fn parse_worktrees(bytes: &[u8]) -> Result<Vec<Worktree>, ProtocolError> {
    let mut rows = Vec::new();
    let mut current = Worktree::default();
    for field in bytes.split(|b| *b == 0) {
        if field.is_empty() {
            if !current.path.is_empty() {
                rows.push(std::mem::take(&mut current));
            }
            continue;
        }
        let field = std::str::from_utf8(field)
            .map_err(|_| invalid("Git checkout paths must be valid UTF-8"))?;
        if let Some(path) = field.strip_prefix("worktree ") {
            if !current.path.is_empty() {
                rows.push(std::mem::take(&mut current));
            }
            if !Path::new(path).is_absolute() || path.len() > 4096 {
                return Err(invalid("Git checkout path is invalid"));
            }
            current.path = path.into();
        } else if let Some(head) = field.strip_prefix("HEAD ") {
            if valid_sha(head) {
                current.head = Some(head.into());
            }
        } else if let Some(branch) = field.strip_prefix("branch refs/heads/") {
            current.branch = Some(branch.into());
        } else if field == "locked" || field.starts_with("locked ") {
            current.locked = true;
        } else if field == "prunable" || field.starts_with("prunable ") {
            current.prunable = true;
        } else if field == "bare" {
            current.bare = true;
        }
    }
    if !current.path.is_empty() {
        rows.push(current);
    }
    if rows.is_empty() {
        return Err(invalid("Git returned no complete checkout inventory"));
    }
    Ok(rows)
}
fn parse_refs(bytes: &[u8]) -> Result<Vec<LocalBranch>, ProtocolError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("Git refs are not valid UTF-8"))?;
    let mut result = Vec::new();
    for line in text.split('\n').filter(|l| !l.is_empty()) {
        let fields = line.split('\0').collect::<Vec<_>>();
        if fields.len() != 4 || !fields[3].is_empty() {
            return Err(invalid("Git branch observation is malformed"));
        }
        let Some(name) = fields[0].strip_prefix("refs/heads/") else {
            continue;
        };
        if name.is_empty() || name.len() > 512 || !valid_sha(fields[1]) {
            return Err(invalid("Git branch observation is invalid"));
        }
        result.push(LocalBranch {
            name: name.into(),
            head: fields[1].into(),
            upstream: (!fields[2].is_empty()).then(|| fields[2].into()),
        });
    }
    Ok(result)
}
fn parse_commits(bytes: &[u8]) -> Result<Vec<api::Commit>, ProtocolError> {
    let mut fields = bytes.split(|b| *b == 0).collect::<Vec<_>>();
    if fields.last() == Some(&&b""[..]) {
        fields.pop();
    }
    if fields.len() % 5 != 0 {
        return Err(invalid("Git commit observation is malformed"));
    }
    let mut commits = Vec::new();
    for chunk in fields.chunks_exact(5) {
        let fields = chunk
            .iter()
            .map(|v| {
                std::str::from_utf8(v)
                    .map(str::to_owned)
                    .map_err(|_| invalid("Git commit metadata is not UTF-8"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let parents = fields[1]
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !valid_sha(&fields[0]) || parents.iter().any(|p| !valid_sha(p)) {
            return Err(invalid("Git commit identity is malformed"));
        }
        commits.push(api::Commit {
            sha: fields[0].clone(),
            parents,
            subject: fields[2].clone(),
            author: fields[3].clone(),
            authored_at: fields[4].clone(),
        });
    }
    Ok(commits)
}
fn valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}
fn load_or_create_branch(
    c: &Connection,
    project: &str,
    source: &str,
    name: &str,
    now: &str,
    actor: &str,
) -> Result<api::Branch, DatabaseError> {
    if let Some(raw)=c.query_row("SELECT record_json::text FROM project_branches WHERE project_id=$1 AND repository_id=$2 AND branch_name=$3",params![project,source,name],|r|r.get::<_,String>(0)).optional()?{return from_json(&raw);}
    let row = empty_branch(
        &new_id("branch").map_err(DatabaseError::Domain)?,
        source,
        name,
    );
    c.execute("INSERT INTO project_branches(project_id,branch_id,repository_id,branch_name,record_json,updated_at,updated_by) VALUES($1,$2,$3,$4,$5::jsonb,$6,$7)",params![project,row.branch_id,source,name,json(&row)?,now,actor])?;
    Ok(row)
}
pub(super) fn persist_branch(
    c: &Connection,
    project: &str,
    row: &api::Branch,
    now: &str,
    actor: &str,
) -> Result<(), DatabaseError> {
    let mut row = row.clone();
    let mut previous = None;
    if let Some(raw)=c.query_row("SELECT record_json::text FROM project_branches WHERE project_id=$1 AND branch_id=$2 FOR UPDATE",params![project,row.branch_id],|r|r.get::<_,String>(0)).optional()? {
        let current:api::Branch=from_json(&raw)?;
        row.intent=current.intent;row.outcome=current.outcome;row.owner=current.owner;row.next_action=current.next_action;row.task_id=current.task_id;
        if row.merge_proof.as_ref().is_none_or(|proof|Some(&proof.head_sha)!=row.head.as_ref()){row.lifecycle=current.lifecycle.clone();}
        previous=Some((current.revision,current.lifecycle));
    }
    if row
        .merge_proof
        .as_ref()
        .is_some_and(|proof| Some(&proof.head_sha) != row.head.as_ref())
    {
        row.merge_proof = None;
        if matches!(row.lifecycle.as_str(), "merged" | "retired") {
            row.lifecycle = "unreviewed".into();
        }
    }
    if row
        .pr
        .as_ref()
        .is_some_and(|pr| Some(&pr.head_sha) != row.head.as_ref())
    {
        row.pr = None;
    }
    if let Some((revision, lifecycle)) = previous {
        row.revision = revision.saturating_add(u32::from(row.lifecycle != lifecycle));
    }
    c.execute("UPDATE project_branches SET head=$3,local_present=$4,remote_present=$5,lifecycle=$6,revision=$7,record_json=$8::jsonb,observed_at=$9,updated_at=$9,updated_by=$10 WHERE project_id=$1 AND branch_id=$2",params![project,row.branch_id,row.head,row.local_present,row.remote_present,row.lifecycle,row.revision,json(&row)?,now,actor])?;
    Ok(())
}
fn load_checkout(
    c: &Connection,
    project: &str,
    id: &str,
) -> Result<Option<api::Checkout>, DatabaseError> {
    let raw=c.query_row("SELECT record_json::text FROM project_checkouts WHERE project_id=$1 AND worktree_id=$2 FOR UPDATE",params![project,id],|r|r.get::<_,String>(0)).optional()?;
    raw.as_deref().map(from_json).transpose()
}
pub(super) fn persist_checkout(
    c: &Connection,
    project: &str,
    physical: &str,
    row: &api::Checkout,
    now: &str,
    actor: &str,
) -> Result<(), DatabaseError> {
    let mut row = row.clone();
    if let Some(current) = load_checkout(c, project, &row.worktree_id)? {
        row.intent = current.intent;
        row.outcome = current.outcome;
        row.owner = current.owner;
        row.next_action = current.next_action;
        row.task_id = current.task_id;
        row.lifecycle = current.lifecycle.clone();
        if row.role != "deployment" || current.role == "primary" {
            row.role = current.role.clone();
        }
        row.revision = current.revision.saturating_add(u32::from(
            row.role != current.role || row.lifecycle != current.lifecycle,
        ));
    }
    c.execute("INSERT INTO project_checkouts(project_id,worktree_id,repository_id,physical_repository_id,absolute_path,role,protected_role,branch_id,branch_name,head,dirty,exists_on_disk,lifecycle,revision,record_json,observed_at,updated_at,updated_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15::jsonb,$16,$16,$17) ON CONFLICT(project_id,worktree_id) DO UPDATE SET role=CASE WHEN project_checkouts.protected_role IS NOT NULL THEN project_checkouts.role ELSE excluded.role END,protected_role=coalesce(project_checkouts.protected_role,excluded.protected_role),branch_id=excluded.branch_id,branch_name=excluded.branch_name,head=excluded.head,dirty=excluded.dirty,exists_on_disk=excluded.exists_on_disk,lifecycle=excluded.lifecycle,revision=excluded.revision,record_json=excluded.record_json,observed_at=excluded.observed_at,updated_at=excluded.updated_at,updated_by=excluded.updated_by",params![project,row.worktree_id,row.repository_id,physical,row.absolute_path,row.role,matches!(row.role.as_str(),"primary"|"deployment").then_some(row.role.as_str()),row.branch_id,row.branch_name,row.head,row.dirty,row.exists,row.lifecycle,row.revision,json(&row)?,now,actor])?;
    Ok(())
}
fn remote_call<T>(
    runtime: &tokio::runtime::Runtime,
    budget: &Budget,
    future: impl std::future::Future<Output = Result<T, github::GithubError>>,
) -> Result<T, ProtocolError> {
    runtime
        .block_on(async { tokio::time::timeout(budget.remaining(), future).await })
        .map_err(|_| {
            ProtocolError::new(ErrorCode::Busy, "remote refresh step reached its deadline")
        })?
        .map_err(|_| {
            ProtocolError::new(
                ErrorCode::InternalError,
                "GitHub refresh unavailable; cached facts retained",
            )
        })
}
