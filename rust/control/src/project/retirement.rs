//! One existing maintenance job owns the exact local/remote retirement sequence.
use super::*;
use crate::maintenance::Budget;
use std::time::Duration;
#[derive(Serialize, Deserialize)]
struct RetirementRequest {
    request: api::BranchRetire,
    uid: u32,
    gid: u32,
    identity: Option<String>,
    via_edge: bool,
    #[serde(default)]
    targets: Vec<(String, String)>,
    #[serde(default)]
    next_local: usize,
    #[serde(default)]
    local_receipts: Vec<String>,
}
impl RetirementRequest {
    fn caller(&self) -> Caller {
        Caller {
            via_edge: self.via_edge,
            pid: 0,
            uid: self.uid,
            gid: self.gid,
            client_kind: if self.identity.is_some() {
                devcoordinator2_api::ClientKind::Edge
            } else {
                devcoordinator2_api::ClientKind::Codex
            },
            model: None,
            effort: None,
            client_session: None,
            work: None,
            identity: self.identity.clone(),
        }
    }
}
impl ProjectService {
    pub fn retire_branch(
        &self,
        p: api::BranchRetire,
        caller: &Caller,
    ) -> Result<api::Job, ProtocolError> {
        for id in [
            &p.project_id,
            &p.branch_id,
            &p.cleanup_job_id,
            &p.worktree_id,
            &p.idempotency_key,
        ] {
            validate_id(id)?;
        }
        if !self.scope(caller)?.admin {
            return Err(denied(
                "remote branch retirement requires a Console administrator",
            ));
        }
        if !matches!(p.expected_head.len(), 40 | 64)
            || !p.expected_head.bytes().all(|b| b.is_ascii_hexdigit())
            || p.pull_request_number == 0
        {
            return Err(invalid(
                "retirement requires an exact branch head and pull request",
            ));
        }
        let cleanup =
            self.verified_checkout_removal(&p.project_id, &p.cleanup_job_id, &p.worktree_id)?;
        if cleanup.head_sha() != p.expected_head {
            return Err(invalid(
                "cleanup receipt belongs to a different branch head",
            ));
        }
        let now = timestamp();
        let id = new_id("job")?;
        let job=self.database.transaction(|c|{
            if let Some(job)=load_job_optional(c,&p.project_id,None,Some((&p.idempotency_key,"retire")))?{return Ok(job);}
            let (revision,source,name,head)=c.query_row("SELECT revision,repository_id,branch_name,head FROM project_branches WHERE project_id=$1 AND branch_id=$2 FOR UPDATE",params![p.project_id,p.branch_id],|r|Ok((r.get::<_,u32>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?))).optional()?.ok_or_else(not_found_db)?;
            if revision!=p.expected_revision||head.as_deref()!=Some(&p.expected_head){return Err(conflict().into());}
            if source!=cleanup.repository_id()||name!=cleanup.branch_name(){return Err(invalid("cleanup receipt belongs to another source or branch").into());}
            let default=c.query_row("SELECT default_branch FROM project_sources WHERE project_id=$1 AND repository_id=$2",params![p.project_id,source],|r|r.get::<_,Option<String>>(0))?;
            if default.as_deref()==Some(&name){return Err(invalid("a source's default branch cannot be retired").into());}
            let physical=c.prepare("SELECT physical_repository_id FROM project_repository_memberships WHERE project_id=$1 AND repository_id=$2 ORDER BY physical_repository_id LIMIT 257")?.query_map(params![p.project_id,source],|r|r.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?;
            if physical.len()>256{return Err(invalid("retirement exceeds its physical-source bound").into());}
            ensure_not_relocating(c,&physical)?;
            let targets=c.prepare("SELECT DISTINCT ON(physical_repository_id) project_id,physical_repository_id FROM project_repository_memberships WHERE repository_id=$1 ORDER BY physical_repository_id,project_id LIMIT 257")?.query_map([&source],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<crate::database::Result<Vec<_>>>()?;
            if targets.len()>256{return Err(invalid("retirement exceeds its physical-source bound").into());}
            let saved=RetirementRequest{request:p.clone(),uid:caller.uid,gid:caller.gid,identity:caller.identity.clone(),via_edge:caller.via_edge,targets,next_local:0,local_receipts:Vec::new()};
            c.execute("INSERT INTO project_jobs(job_id,project_id,kind,idempotency_key,state,authorized_repositories,cursor,queued_at,actor) VALUES($1,$2,'retire',$3,'queued',$4::jsonb,$5,$6,$7)",params![id,p.project_id,p.idempotency_key,json(&physical)?,json(&saved)?,now,caller.actor()])?;
            load_job_optional(c,&p.project_id,Some(&id),None)?.ok_or_else(not_found_db)
        }).map_err(database_error)?;
        self.wake.notify_one();
        Ok(job)
    }
    pub(super) fn run_retirement_job(&self, budget: &mut Budget) -> Result<bool, ProtocolError> {
        let job=self.database.transaction(|c|{let row=c.query_row("SELECT job_id,project_id,cursor FROM project_jobs WHERE kind='retire' AND state IN('queued','running') ORDER BY queued_at,job_id LIMIT 1 FOR UPDATE SKIP LOCKED",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional()?;if let Some((id,_,_))=&row{c.execute("UPDATE project_jobs SET state='running',started_at=coalesce(started_at,$2),revision=revision+1 WHERE job_id=$1",params![id,timestamp()])?;}Ok(row)}).map_err(database_error)?;
        let Some((id, project, raw)) = job else {
            return Ok(false);
        };
        let mut saved: RetirementRequest = from_json(&raw).map_err(database_error)?;
        let caller = saved.caller();
        let p = saved.request.clone();
        let result = (|| -> Result<Option<api::RetirementReceipt>, ProtocolError> {
            if !self.scope(&caller)?.admin {
                return Err(denied(
                    "retirement requester no longer has administrator access",
                ));
            }
            let cleanup =
                self.verified_checkout_removal(&p.project_id, &p.cleanup_job_id, &p.worktree_id)?;
            let (source,name,url,physicals)=self.database.read(|c|{
                let (source,name,url,head,revision)=c.query_row("SELECT b.repository_id,b.branch_name,s.github_url,b.head,b.revision FROM project_branches b JOIN project_sources s USING(project_id,repository_id) WHERE b.project_id=$1 AND b.branch_id=$2",params![p.project_id,p.branch_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,u32>(4)?))).optional()?.ok_or_else(not_found_db)?;
                if revision!=p.expected_revision||head.as_deref()!=Some(&p.expected_head)||cleanup.head_sha()!=p.expected_head||cleanup.repository_id()!=source||cleanup.branch_name()!=name{return Err(conflict().into());}
                let physicals=c.prepare("SELECT DISTINCT ON(physical_repository_id) project_id,physical_repository_id FROM project_repository_memberships WHERE repository_id=$1 ORDER BY physical_repository_id,project_id LIMIT 257")?.query_map([&source],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<crate::database::Result<Vec<_>>>()?;
                if physicals!=saved.targets{return Err(conflict().into());}
                ensure_not_relocating(c,&physicals.iter().map(|(_,id)|id.clone()).collect::<Vec<_>>())?;
                let remaining=c.query_row("SELECT EXISTS(SELECT 1 FROM project_checkouts WHERE repository_id=$1 AND branch_name=$2 AND exists_on_disk IS DISTINCT FROM false)",params![source,name],|r|r.get::<_,bool>(0))?;
                if remaining{return Err(invalid("other checkouts still use the branch; remove or preserve them before remote retirement").into());}
                if physicals.len()>256{return Err(invalid("retirement exceeds its physical-source bound").into());}Ok((source,name,url,physicals))
            }).map_err(database_error)?;
            let remote = github::GitHubRepository::from_url(
                url.as_deref()
                    .ok_or_else(|| invalid("GitHub source is not configured"))?,
            )
            .map_err(|_| invalid("GitHub source is invalid"))?;
            let client = self
                .github
                .as_ref()
                .ok_or_else(|| denied("GitHub credentials are not configured"))?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| invalid("remote I/O runtime is unavailable"))?;
            if let Some((target_project, physical)) = physicals.get(saved.next_local) {
                // Refresh exact integration before one bounded local CAS step.
                runtime
                    .block_on(async {
                        tokio::time::timeout(
                            budget.remaining(),
                            client.verify_integration(
                                &remote,
                                &name,
                                &p.expected_head,
                                p.pull_request_number,
                                p.integration_branch.as_deref(),
                            ),
                        )
                        .await
                    })
                    .map_err(|_| invalid("integration proof timed out"))?
                    .map_err(|_| {
                        invalid("exact merged pull request integration could not be verified")
                    })?;
                if budget.remaining() < Duration::from_millis(100) {
                    return Err(invalid("branch retirement reached its maintenance budget"));
                }
                // This helper performs a fresh real Git checkout-user check and
                // compare-and-swap deletion at the exact reviewed head.
                let receipt = self.retire_local_checkout_branch(
                    target_project,
                    physical,
                    &name,
                    &cleanup,
                    &caller,
                )?;
                saved.local_receipts.push(receipt.receipt_id);
                saved.next_local += 1;
                return Ok(None);
            }
            let deletion = github::GithubDeleteRequest {
                repository_id: source.clone(),
                branch_name: name.clone(),
                expected_head_sha: p.expected_head.clone(),
                pull_request_number: p.pull_request_number,
                integration_branch: p.integration_branch.clone(),
            };
            let remote_receipt = runtime
                .block_on(async {
                    tokio::time::timeout(
                        budget.remaining(),
                        client.delete_branch(&remote, &deletion, &cleanup),
                    )
                    .await
                })
                .map_err(|_| {
                    invalid("remote retirement timed out; verify the branch before retrying")
                })?
                .map_err(|_| invalid("remote branch retirement is not verified"))?;
            let receipt = api::RetirementReceipt {
                branch_id: p.branch_id.clone(),
                repository_id: source,
                branch_name: name,
                head_sha: p.expected_head.clone(),
                local_cleanup_receipt_id: cleanup.receipt_id().into(),
                local_branch_receipt_ids: saved.local_receipts.clone(),
                remote_receipt_id: remote_receipt.mutation_id,
                observed_at: remote_receipt.observed_at,
            };
            self.database.transaction(|c|{let raw=c.query_row("SELECT record_json::text FROM project_branches WHERE project_id=$1 AND branch_id=$2 FOR UPDATE",params![p.project_id,p.branch_id],|r|r.get::<_,String>(0))?;let mut branch:api::Branch=from_json(&raw)?;branch.lifecycle="retired".into();branch.local_present=false;branch.remote_present=Some(false);branch.revision+=1;c.execute("UPDATE project_branches SET lifecycle='retired',local_present=false,remote_present=false,revision=$3,record_json=$4::jsonb,updated_at=$5 WHERE project_id=$1 AND branch_id=$2",params![p.project_id,p.branch_id,branch.revision,json(&branch)?,receipt.observed_at])?;c.execute("UPDATE project_branch_observations SET local_present=false WHERE project_id=$1 AND branch_id=$2",params![p.project_id,p.branch_id])?;changed(c,&p.project_id,&caller.actor(),&receipt.observed_at)?;event(c,&p.project_id,Some(&branch.repository_id),None,Some(&p.branch_id),"branch.retired","The exact integrated branch was removed locally and from GitHub",&caller.actor(),&receipt.observed_at)}).map_err(database_error)?;
            Ok(Some(receipt))
        })();
        let now = timestamp();
        let finished = !matches!(result, Ok(None));
        self.database.transaction(|c|{c.execute("UPDATE project_jobs SET state=$2,error=$3,result_json=$4::jsonb,finished_at=$5,cursor=$6,revision=revision+1 WHERE job_id=$1",params![id,match &result{Ok(Some(_))=>"succeeded",Ok(None)=>"running",Err(_)=>"failed"},result.as_ref().err().map(|_|"branch retirement was not fully verified; inspect current branch and cleanup receipts"),result.as_ref().ok().and_then(Option::as_ref).map(json).transpose()?,finished.then_some(now),json(&saved)?])?;Ok(())}).map_err(database_error)?;
        if !finished {
            self.wake.notify_one();
        }
        self.publish_change(&project, "project.updated");
        budget.consume(1, 0);
        Ok(true)
    }
}
