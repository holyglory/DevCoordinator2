//! The Project Explorer's exact checkout lifecycle bridge. Storage owns every
//! cleanup job and irreversible effect; projects retain intent and history.
use super::*;
use crate::repository;
use crate::storage::lifecycle::{BoundCheckout, CheckoutBinding, IntegrationProof};
use devcoordinator2_api::project::cleanup as removal;
use devcoordinator2_api::storage as storage_api;
use std::path::PathBuf;

const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Proof issued only after an exact storage item receipt, path absence and Git
/// metadata absence agree. Remote branch retirement cannot accept a request
/// flag or a deserialized assertion in its place.
#[derive(Clone, Debug)]
pub(crate) struct VerifiedCheckoutRemoval {
    repository_id: String,
    worktree_id: String,
    branch_name: String,
    head_sha: String,
    receipt_id: String,
    approved_unpreserved_git_history: Vec<String>,
}

impl VerifiedCheckoutRemoval {
    pub(crate) fn repository_id(&self) -> &str {
        &self.repository_id
    }
    pub(crate) fn worktree_id(&self) -> &str {
        &self.worktree_id
    }
    pub(crate) fn branch_name(&self) -> &str {
        &self.branch_name
    }
    pub(crate) fn head_sha(&self) -> &str {
        &self.head_sha
    }
    pub(crate) fn receipt_id(&self) -> &str {
        &self.receipt_id
    }
    pub(crate) fn approved_unpreserved_git_history(&self) -> &[String] {
        &self.approved_unpreserved_git_history
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct LocalBranchRetirement {
    pub repository_id: String,
    pub branch_name: String,
    pub removed_head_sha: String,
    pub cleanup_receipt_id: String,
    pub receipt_id: String,
}

impl ProjectService {
    pub fn checkout_remove_plan(
        &self,
        request: removal::Plan,
        caller: &Caller,
    ) -> Result<removal::ReviewJob, ProtocolError> {
        validate_id(&request.project_id)?;
        if request.targets.is_empty() || request.targets.len() > 200 {
            return Err(invalid("select between 1 and 200 exact checkouts"));
        }
        let scope = self.scope(caller)?;
        if request.include_absolute_paths && !scope.admin {
            return Err(denied(
                "absolute checkout paths require administrator access",
            ));
        }
        let mut seen = BTreeSet::new();
        for target in &request.targets {
            validate_id(&target.worktree_id)?;
            if !seen.insert(target.worktree_id.clone()) {
                return Err(invalid("checkout selection contains duplicates"));
            }
        }
        let (bindings,paths)=self.database.read_transaction(|connection| {
            let revision:u64=connection.query_row("SELECT revision FROM projects WHERE project_id=$1",[&request.project_id],|row|row.get(0)).optional()?.ok_or_else(||DatabaseError::Domain(missing("project is not registered")))?;
            if revision!=request.expected_project_revision {return Err(DatabaseError::Domain(conflict("project changed; refresh before removing checkouts")));}
            let roots=connection.prepare("SELECT absolute_path FROM project_roots WHERE project_id=$1 ORDER BY length(absolute_path) DESC,root_id")?.query_map([&request.project_id],|row|row.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?.into_iter().map(PathBuf::from).collect::<Vec<_>>();
            let mut bindings=Vec::new();let mut paths=BTreeMap::new();
            for target in &request.targets {
                let binding=connection.query_row("SELECT c.repository_id,c.physical_repository_id,c.absolute_path,c.protected_role,c.lifecycle,c.branch_name,c.task_id,c.revision,r.root_path,p.absolute_path FROM project_checkouts c JOIN repositories r ON r.repository_id=c.physical_repository_id JOIN project_sources s ON s.project_id=c.project_id AND s.repository_id=c.repository_id LEFT JOIN project_checkouts p ON p.project_id=s.project_id AND p.worktree_id=s.primary_checkout_id WHERE c.project_id=$1 AND c.worktree_id=$2",params![request.project_id,target.worktree_id],|row|Ok(CheckoutBinding {
                    project_id:request.project_id.clone(),repository_id:row.get(0)?,physical_repository_id:row.get(1)?,path:PathBuf::from(row.get::<_,String>(2)?),protected_role:row.get(3)?,lifecycle:row.get(4)?,branch_name:row.get(5)?,task_id:row.get(6)?,revision:row.get(7)?,canonical_path:PathBuf::from(row.get::<_,String>(8)?),primary_path:PathBuf::from(row.get::<_,Option<String>>(9)?.unwrap_or(row.get::<_,String>(8)?)),worktree_id:target.worktree_id.clone(),
                })).optional()?.ok_or_else(||DatabaseError::Domain(missing("selected checkout is not registered in this project")))?;
                if !scope.edits(&binding.physical_repository_id) {return Err(DatabaseError::Domain(denied("checkout removal requires operator access to every selected source")));}
                if binding.revision!=target.expected_revision {return Err(DatabaseError::Domain(conflict("selected checkout changed; refresh before removing it")));}
                let display=if request.include_absolute_paths {(binding.path.to_string_lossy().into_owned(),true)} else {
                    let relative=roots.iter().find_map(|root|binding.path.strip_prefix(root).ok().filter(|relative|!relative.as_os_str().is_empty()));
                    let path=relative.map(|relative|relative.to_string_lossy().into_owned()).unwrap_or_else(||binding.path.file_name().unwrap_or_default().to_string_lossy().into_owned());
                    (path,false)
                };
                paths.insert(binding.worktree_id.clone(),display);
                bindings.push(binding);
            }
            Ok((bindings,paths))
        }).map_err(database_error)?;
        let target_count = bindings.len() as u32;
        let review = crate::storage::lifecycle::CheckoutReviewRequest {
            plan: request.clone(),
            bindings,
            paths,
            progress: removal::ReviewProgress {
                targets_total: target_count,
                ..Default::default()
            },
        };
        let job = self
            .cleanup_storage()?
            .queue_checkout_review(review, caller)?;
        self.checkout_remove_plan_status(
            removal::JobReference {
                project_id: request.project_id,
                job_id: job.job_id,
            },
            caller,
        )
    }

    /// Both pending and complete reviews retain the exact project/source
    /// authorization. A job ID never grants access through a different project.
    pub fn checkout_remove_plan_status(
        &self,
        request: removal::JobReference,
        caller: &Caller,
    ) -> Result<removal::ReviewJob, ProtocolError> {
        let (job, review) = self.checkout_authorize_review(&request, caller)?;
        let plan = if matches!(job.state, storage_api::JobState::Completed) {
            let stored = self.cleanup_storage()?.stored_plan(
                job.plan_id
                    .as_deref()
                    .ok_or_else(|| invalid("checkout_review_plan_missing"))?,
            )?;
            let data = stored
                .checkout_removal
                .ok_or_else(|| invalid("checkout_review_plan_missing"))?;
            self.checkout_authorize_plan(&request.project_id, &data.public.plan_id, caller, false)?;
            Some(data.public)
        } else {
            None
        };
        Ok(removal::ReviewJob {
            project_id: request.project_id,
            job,
            progress: review.progress,
            plan,
        })
    }

    pub fn checkout_remove_plan_cancel(
        &self,
        request: removal::JobReference,
        caller: &Caller,
    ) -> Result<removal::ReviewJob, ProtocolError> {
        // Authorize the original request before any cancellation write.
        let (job, _) = self.checkout_authorize_review(&request, caller)?;
        if matches!(
            job.state,
            storage_api::JobState::Queued
                | storage_api::JobState::Running
                | storage_api::JobState::Cancelling
        ) {
            self.cleanup_storage()?.cancel(
                storage_api::JobReference {
                    job_id: request.job_id.clone(),
                },
                caller,
            )?;
        }
        self.checkout_remove_plan_status(request, caller)
    }

    fn checkout_authorize_review(
        &self,
        request: &removal::JobReference,
        caller: &Caller,
    ) -> Result<
        (
            storage_api::Job,
            crate::storage::lifecycle::CheckoutReviewRequest,
        ),
        ProtocolError,
    > {
        validate_id(&request.project_id)?;
        validate_id(&request.job_id)?;
        let (job, review) = self.cleanup_storage()?.checkout_review(&request.job_id)?;
        if review.plan.project_id != request.project_id
            || review
                .bindings
                .iter()
                .any(|binding| binding.project_id != request.project_id)
        {
            return Err(missing("checkout review is not registered in this project"));
        }
        let scope = self.scope(caller)?;
        if review.plan.include_absolute_paths && !scope.admin {
            return Err(denied(
                "absolute checkout paths require administrator access",
            ));
        }
        if review.bindings.is_empty()
            || review
                .bindings
                .iter()
                .any(|binding| !scope.edits(&binding.physical_repository_id))
        {
            return Err(denied(
                "checkout review requires operator access to every selected source",
            ));
        }
        self.database.read(|connection| {
            for binding in &review.bindings {
                let member=connection.query_row("SELECT EXISTS(SELECT 1 FROM project_repository_memberships WHERE project_id=$1 AND repository_id=$2 AND physical_repository_id=$3)",params![request.project_id,binding.repository_id,binding.physical_repository_id],|row|row.get::<_,bool>(0))?;
                if !member {return Err(DatabaseError::Domain(missing("checkout review source is no longer registered in this project")));}
            }
            Ok(())
        }).map_err(database_error)?;
        Ok((job, review))
    }

    pub fn checkout_remove_approve(
        &self,
        request: removal::Approve,
        caller: &Caller,
    ) -> Result<removal::ApprovalReceipt, ProtocolError> {
        self.checkout_authorize_plan(&request.project_id, &request.plan_id, caller, true)?;
        self.cleanup_storage()?.checkout_approve(request, caller)
    }

    pub fn checkout_remove_start(
        &self,
        request: removal::Start,
        caller: &Caller,
    ) -> Result<removal::RemovalJob, ProtocolError> {
        self.checkout_authorize_plan(&request.project_id, &request.plan_id, caller, false)?;
        let job = self
            .cleanup_storage()?
            .checkout_start(request.clone(), caller)?;
        self.checkout_remove_job(
            removal::JobReference {
                project_id: request.project_id,
                job_id: job.job_id,
            },
            caller,
        )
    }

    pub fn checkout_remove_job(
        &self,
        request: removal::JobReference,
        caller: &Caller,
    ) -> Result<removal::RemovalJob, ProtocolError> {
        let storage = self.cleanup_storage()?;
        let (job, data) = storage.checkout_job(&request.project_id, &request.job_id)?;
        self.checkout_authorize_plan(&request.project_id, &data.public.plan_id, caller, false)?;
        let mut targets = Vec::new();
        for target in &data.targets {
            let removed = job.receipts.iter().any(|receipt| {
                receipt.artifact_id == target.artifact_id && receipt.status == "removed"
            }) && storage.checkout_target_absent(target)?;
            if removed {
                self.checkout_record_removed(target, &job.job_id)?;
            }
            targets.push(removal::CheckoutResult {
                worktree_id: target.binding.worktree_id.clone(),
                artifact_id: target.artifact_id.clone(),
                fully_removed: removed,
                task_stays_open: target.observed.task_stays_open,
                receipt_id: removed.then(|| format!("{}:{}", job.job_id, target.artifact_id)),
            });
        }
        Ok(removal::RemovalJob {
            project_id: request.project_id,
            plan_id: data.public.plan_id,
            job,
            targets,
        })
    }

    pub fn checkout_remove_cancel(
        &self,
        request: removal::JobReference,
        caller: &Caller,
    ) -> Result<removal::RemovalJob, ProtocolError> {
        let storage = self.cleanup_storage()?;
        let (_, data) = storage.checkout_job(&request.project_id, &request.job_id)?;
        self.checkout_authorize_plan(&request.project_id, &data.public.plan_id, caller, false)?;
        storage.cancel(
            devcoordinator2_api::storage::JobReference {
                job_id: request.job_id.clone(),
            },
            caller,
        )?;
        self.checkout_remove_job(request, caller)
    }

    pub(crate) fn retire_local_checkout_branch(
        &self,
        project_id: &str,
        physical_repository_id: &str,
        branch_name: &str,
        cleanup: &VerifiedCheckoutRemoval,
        caller: &Caller,
    ) -> Result<LocalBranchRetirement, ProtocolError> {
        let scope = self.scope(caller)?;
        if !scope.edits(physical_repository_id) {
            return Err(denied("local branch retirement requires operator access"));
        }
        if cleanup.branch_name() != branch_name {
            return Err(conflict("local cleanup receipt belongs to another branch"));
        }
        let (root,uid,source,remaining)=self.database.read(|connection| {
            let member=connection.query_row("SELECT r.root_path,r.registered_by_uid,m.repository_id FROM project_repository_memberships m JOIN repositories r ON r.repository_id=m.physical_repository_id WHERE m.project_id=$1 AND m.physical_repository_id=$2",params![project_id,physical_repository_id],|row|Ok((PathBuf::from(row.get::<_,String>(0)?),row.get::<_,u32>(1)?,row.get::<_,String>(2)?))).optional()?.ok_or_else(||DatabaseError::Domain(missing("project source is not registered")))?;
            let remaining=connection.query_row("SELECT EXISTS(SELECT 1 FROM project_checkouts WHERE project_id=$1 AND physical_repository_id=$2 AND branch_name=$3 AND exists_on_disk IS DISTINCT FROM false)",params![project_id,physical_repository_id,branch_name],|row|row.get::<_,bool>(0))?;
            Ok((member.0,member.1,member.2,remaining))
        }).map_err(database_error)?;
        if source != cleanup.repository_id() {
            return Err(conflict("local cleanup receipt belongs to another source"));
        }
        if remaining {
            return Err(conflict("other local checkouts still use this branch"));
        }
        if branch_name.is_empty() || branch_name.len() > 512 || branch_name.starts_with('-') {
            return Err(invalid("branch name is invalid"));
        }
        if uid == 0 {
            return Err(denied(
                "Git execution requires the registered nonroot owner",
            ));
        }
        let gid = crate::systemd::primary_gid(uid)
            .map_err(|_| denied("registered Git execution owner is unavailable"))?;
        let owner = Some((uid, gid));
        let reference = format!("refs/heads/{branch_name}");
        if root.exists() {
            let valid = repository::run_git_arguments(
                &root,
                owner,
                &["check-ref-format", &reference],
                GIT_TIMEOUT,
            )
            .map_err(|_| invalid("branch reference cannot be verified"))?;
            if !valid.status.success() {
                return Err(invalid("branch reference is invalid"));
            }
            let worktrees = repository::run_git_arguments(
                &root,
                owner,
                &["worktree", "list", "--porcelain", "-z"],
                GIT_TIMEOUT,
            )
            .map_err(|_| invalid("Git worktree users cannot be verified"))?;
            if !worktrees.status.success() {
                return Err(invalid("Git worktree users cannot be verified"));
            }
            let expected = format!("branch {reference}");
            if worktrees
                .stdout
                .split(|byte| *byte == 0)
                .any(|field| field == expected.as_bytes())
            {
                return Err(conflict("a Git checkout still uses this branch"));
            }
            let head = repository::run_git_arguments(
                &root,
                owner,
                &["rev-parse", "--verify", "--quiet", &reference],
                GIT_TIMEOUT,
            )
            .map_err(|_| invalid("local branch identity cannot be verified"))?;
            if head.status.success() {
                if std::str::from_utf8(&head.stdout).ok().map(str::trim) != Some(cleanup.head_sha())
                {
                    return Err(conflict(
                        "local branch changed after the checkout was removed",
                    ));
                }
                verify_branch_reflog_preserved(
                    &root,
                    owner,
                    &reference,
                    cleanup.head_sha(),
                    cleanup.approved_unpreserved_git_history(),
                )?;
                let result = repository::run_git_arguments(
                    &root,
                    owner,
                    &[
                        "update-ref",
                        "--no-deref",
                        "-d",
                        &reference,
                        cleanup.head_sha(),
                    ],
                    GIT_TIMEOUT,
                )
                .map_err(|_| conflict("local branch retirement is unverified"))?;
                if !result.status.success() {
                    return Err(conflict("local branch changed during retirement"));
                }
            } else if head.status.code() != Some(1) {
                return Err(invalid("local branch identity cannot be verified"));
            }
            let absent = repository::run_git_arguments(
                &root,
                owner,
                &["rev-parse", "--verify", "--quiet", &reference],
                GIT_TIMEOUT,
            )
            .map_err(|_| conflict("local branch retirement is unverified"))?;
            if absent.status.success() || absent.status.code() != Some(1) {
                return Err(conflict("local branch retirement is unverified"));
            }
        } else if cleanup.worktree_id().is_empty() {
            return Err(conflict("clone removal is unverified"));
        }
        let receipt = LocalBranchRetirement {
            repository_id: physical_repository_id.into(),
            branch_name: branch_name.into(),
            removed_head_sha: cleanup.head_sha().into(),
            cleanup_receipt_id: cleanup.receipt_id().into(),
            receipt_id: format!(
                "local-branch-{}",
                crate::storage::hash(
                    format!(
                        "{project_id}:{physical_repository_id}:{reference}:{}:{}",
                        cleanup.head_sha(),
                        cleanup.receipt_id()
                    )
                    .as_bytes()
                )
            ),
        };
        let value = serde_json::to_string(&receipt)
            .map_err(|_| invalid("local branch receipt cannot be stored"))?;
        let actor = caller.actor();
        let now = self.cleanup_storage()?.now_ms();
        self.database.transaction(|connection| {
            connection.execute("INSERT INTO storage_changes(artifact_id,kind,actor,created_at_ms,change_json) SELECT NULL,'checkout_local_branch_retired',$1,$2,$3 WHERE NOT EXISTS(SELECT 1 FROM storage_changes WHERE kind='checkout_local_branch_retired' AND change_json->>'receipt_id'=$4)",params![actor,now as i64,value,receipt.receipt_id])?;
            Ok(())
        }).map_err(database_error)?;
        Ok(receipt)
    }

    pub(crate) fn verified_checkout_removal(
        &self,
        project_id: &str,
        job_id: &str,
        worktree_id: &str,
    ) -> Result<VerifiedCheckoutRemoval, ProtocolError> {
        let storage = self.cleanup_storage()?;
        let (job, data) = storage.checkout_job(project_id, job_id)?;
        let target = data
            .targets
            .iter()
            .find(|target| target.binding.worktree_id == worktree_id)
            .ok_or_else(|| missing("checkout removal receipt is not for this checkout"))?;
        if !job
            .receipts
            .iter()
            .any(|receipt| receipt.artifact_id == target.artifact_id && receipt.status == "removed")
            || !storage.checkout_target_absent(target)?
        {
            return Err(conflict("local checkout removal is not verified"));
        }
        Ok(VerifiedCheckoutRemoval {
            repository_id: target.binding.repository_id.clone(),
            worktree_id: worktree_id.into(),
            branch_name: target.observed.branch_name.clone().ok_or_else(|| {
                invalid("detached checkouts do not authorize remote branch retirement")
            })?,
            head_sha: target
                .observed
                .head
                .clone()
                .ok_or_else(|| invalid("checkout head is unavailable"))?,
            receipt_id: format!("{}:{}", job.job_id, target.artifact_id),
            // A cleanup receipt can exist only after checkout_start accepted
            // the exact approval bound to this plan and fingerprint. Carry
            // the reviewed history identifiers into branch retirement so the
            // later ref deletion can accept only this approved loss.
            approved_unpreserved_git_history: target.observed.unpreserved_git_history.clone(),
        })
    }

    /// Only an unforgeable proof returned by the GitHub adapter can establish
    /// squash equivalence; plain branch metadata can never grant it.
    pub(crate) fn record_checkout_integration_from_refresh(
        &self,
        project_id: &str,
        repository_id: &str,
        proof: &github::GithubIntegrationProof,
        actor: &str,
    ) -> Result<(), ProtocolError> {
        if !proof.is_fresh() {
            return Err(conflict("integration evidence expired; refresh it"));
        }
        let (root,uid,url)=self.database.read(|connection|Ok(connection.query_row("SELECT r.root_path,r.registered_by_uid,s.github_url FROM repositories r JOIN project_repository_memberships m ON m.physical_repository_id=r.repository_id JOIN project_sources s ON s.project_id=m.project_id AND s.repository_id=m.repository_id WHERE m.project_id=$1 AND r.repository_id=$2",params![project_id,repository_id],|row|Ok((PathBuf::from(row.get::<_,String>(0)?),row.get::<_,u32>(1)?,row.get::<_,Option<String>>(2)?))).optional()?)).map_err(database_error)?.ok_or_else(||missing("project source is not registered"))?;
        if !url
            .as_deref()
            .and_then(|url| github::GitHubRepository::from_url(url).ok())
            .is_some_and(|source| same_github_source(&source, proof.repository()))
        {
            return Err(conflict(
                "integration evidence belongs to another GitHub source",
            ));
        }
        if uid == 0 {
            return Err(denied(
                "Git execution requires the registered nonroot owner",
            ));
        }
        let gid = crate::systemd::primary_gid(uid)
            .map_err(|_| denied("registered Git execution owner is unavailable"))?;
        let owner = Some((uid, gid));
        let remote = repository::run_git_arguments(
            &root,
            owner,
            &["remote", "get-url", "--", "origin"],
            GIT_TIMEOUT,
        )
        .map_err(|_| invalid("source remote identity is unavailable"))?;
        if !remote.status.success() {
            return Err(invalid("source remote identity is unavailable"));
        }
        let remote_text = std::str::from_utf8(&remote.stdout)
            .map_err(|_| invalid("source remote identity is unavailable"))?
            .trim();
        let remote_source = if let Some(path) = remote_text.strip_prefix("git@github.com:") {
            github::GitHubRepository::from_url(&format!("https://github.com/{path}"))
        } else if let Some(path) = remote_text.strip_prefix("ssh://git@github.com/") {
            github::GitHubRepository::from_url(&format!("https://github.com/{path}"))
        } else {
            github::GitHubRepository::from_url(remote_text)
        }
        .map_err(|_| invalid("source remote does not match the verified GitHub repository"))?;
        if !same_github_source(&remote_source, proof.repository()) {
            return Err(conflict("source remote changed since GitHub verification"));
        }
        let observed = time::OffsetDateTime::parse(
            proof.observed_at(),
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| invalid("integration observation time is invalid"))?;
        let verified_at_ms = u64::try_from(observed.unix_timestamp_nanos() / 1_000_000)
            .map_err(|_| invalid("integration observation time is invalid"))?;
        let now = self.cleanup_storage()?.now_ms();
        if verified_at_ms > now.saturating_add(1_000)
            || now.saturating_sub(verified_at_ms) > crate::storage::SAFETY_FRESH_MS
        {
            return Err(conflict("integration evidence expired; refresh it"));
        }
        let private = IntegrationProof {
            branch_name: proof.branch_name().into(),
            original_head: proof.head_sha().into(),
            merge_commit: proof.merge_commit_sha().into(),
            integration_branch: proof.integration_branch().into(),
            integration_head: proof.integration_head_sha().into(),
            remote_identity: crate::storage::hash(&remote.stdout),
            verified_at_ms,
        };
        let actor = actor.to_owned();
        let repository = repository_id.to_owned();
        let value=serde_json::to_string(&serde_json::json!({"project_id":project_id,"repository_id":repository_id,"branch_name":proof.branch_name(),"proof":private})).map_err(|_|invalid("integration proof cannot be stored"))?;
        self.database.transaction(move |connection| {
            connection.execute("INSERT INTO storage_changes(artifact_id,kind,actor,created_at_ms,change_json) VALUES(NULL,'checkout_integration_verified',$1,$2,$3)",params![actor,now as i64,value])?;
            connection.execute("INSERT INTO project_events(project_id,repository_id,physical_repository_id,target_id,kind,summary,actor,at) VALUES($1,$2,$3,$4,'integration_verified','The exact feature head is preserved by its verified merge.',$5,$6)",params![project_id,repository,repository_id,proof.branch_name(),actor,timestamp()])?;
            Ok(())
        }).map_err(database_error)
    }

    fn cleanup_storage(&self) -> Result<&StorageService, ProtocolError> {
        self.storage
            .as_ref()
            .ok_or_else(|| invalid("checkout cleanup is unavailable"))
    }

    fn checkout_authorize_plan(
        &self,
        project_id: &str,
        plan_id: &str,
        caller: &Caller,
        admin: bool,
    ) -> Result<(), ProtocolError> {
        validate_id(project_id)?;
        validate_id(plan_id)?;
        let scope = self.scope(caller)?;
        if admin && !scope.admin {
            return Err(denied(
                "checkout loss approval requires administrator access",
            ));
        }
        let plan = self.cleanup_storage()?.stored_plan(plan_id)?;
        let data = plan
            .checkout_removal
            .ok_or_else(|| invalid("plan is not a checkout removal"))?;
        if data.public.project_id != project_id {
            return Err(missing("checkout plan is not registered in this project"));
        }
        if data
            .targets
            .iter()
            .any(|target| !scope.edits(&target.binding.physical_repository_id))
        {
            return Err(denied(
                "checkout removal requires operator access to every selected source",
            ));
        }
        Ok(())
    }

    fn checkout_record_removed(
        &self,
        target: &BoundCheckout,
        job_id: &str,
    ) -> Result<(), ProtocolError> {
        let project_id = target.binding.project_id.clone();
        let worktree_id = target.binding.worktree_id.clone();
        let expected_path = target.binding.path.to_string_lossy().into_owned();
        let at = timestamp();
        let actor = format!("storage:{job_id}");
        let repository = target.binding.repository_id.clone();
        let physical = target.binding.physical_repository_id.clone();
        let now_ms = self.cleanup_storage()?.now_ms();
        self.database.transaction(move |connection| {
            let changed=connection.execute("UPDATE project_checkouts SET exists_on_disk=false,lifecycle='retired',revision=revision+1,record_json=record_json || jsonb_build_object('exists',false,'lifecycle','retired','can_remove',false,'protection_reasons',jsonb_build_array('already_removed'),'revision',revision+1),updated_at=$1,updated_by=$2 WHERE project_id=$3 AND worktree_id=$4 AND absolute_path=$5 AND exists_on_disk IS DISTINCT FROM false",params![at,actor,project_id,worktree_id,expected_path])?;
            if changed>0 {
                let nested=connection.prepare("SELECT s.record_json FROM storage_artifacts s WHERE s.removed_at_ms IS NULL AND EXISTS(SELECT 1 FROM (VALUES(s.record_json#>>'{locator,path}'),(s.record_json#>>'{locator,target}')) location(path) WHERE path=$1 OR strpos(path,$1||'/')=1) AND (EXISTS(SELECT 1 FROM project_storage p WHERE p.project_id=$2 AND p.worktree_id=$3 AND p.artifact_id=s.artifact_id) OR EXISTS(SELECT 1 FROM project_checkout_artifacts a WHERE a.project_id=$2 AND a.worktree_id=$3 AND a.artifact_id=s.artifact_id AND a.active)) ORDER BY s.artifact_id FOR UPDATE")?.query_map(params![expected_path,project_id,worktree_id],|row|row.get::<_,String>(0))?.collect::<crate::database::Result<Vec<_>>>()?;
                for raw in nested {
                    let mut record:crate::storage::model::Record=from_json(&raw)?;
                    record.artifact.removed_at_ms=Some(now_ms);
                    record.artifact.deletable=false;
                    record.artifact.automatic_eligible=false;
                    record.artifact.revision=record.artifact.revision.saturating_add(1);
                    record.update_sequence=crate::storage::next_sequence_in(connection)?;
                    connection.execute("UPDATE storage_artifacts SET removed_at_ms=$1,revision=$2,record_json=$3,updated_at_ms=$1 WHERE artifact_id=$4",params![now_ms,record.artifact.revision,json(&record)?,record.artifact.artifact_id])?;
                    connection.execute("UPDATE project_checkout_artifacts SET active=false,revision=revision+1,updated_at=$1,updated_by=$2 WHERE project_id=$3 AND worktree_id=$4 AND artifact_id=$5 AND active",params![at,actor,project_id,worktree_id,record.artifact.artifact_id])?;
                }
                connection.execute("DELETE FROM project_storage p WHERE p.project_id=$1 AND p.worktree_id=$2 AND (p.kind='source' OR EXISTS(SELECT 1 FROM storage_artifacts s WHERE s.artifact_id=p.artifact_id AND EXISTS(SELECT 1 FROM (VALUES(s.record_json#>>'{locator,path}'),(s.record_json#>>'{locator,target}')) location(path) WHERE path=$3 OR strpos(path,$3||'/')=1)))",params![project_id,worktree_id,expected_path])?;
                connection.execute("UPDATE project_checkouts c SET record_json=c.record_json || jsonb_build_object('source_bytes',0,'build_bytes',(SELECT CASE WHEN count(*)=count(allocated_bytes) THEN COALESCE(sum(allocated_bytes),0) ELSE NULL END FROM project_storage WHERE project_id=$1 AND worktree_id=$2 AND kind='build'),'local_bytes',(SELECT CASE WHEN count(*)=count(allocated_bytes) THEN COALESCE(sum(allocated_bytes),0) ELSE NULL END FROM project_storage WHERE project_id=$1 AND worktree_id=$2 AND kind='build'),'locations',COALESCE((SELECT jsonb_agg(location) FROM jsonb_array_elements(COALESCE(c.record_json->'locations','[]'::jsonb)) location WHERE EXISTS(SELECT 1 FROM project_storage p WHERE p.project_id=$1 AND p.worktree_id=$2 AND p.kind='build' AND p.artifact_id=location->>'artifact_id')),'[]'::jsonb),'artifact_ids',COALESCE((SELECT jsonb_agg(artifact_id) FROM project_storage WHERE project_id=$1 AND worktree_id=$2 AND kind='build'),'[]'::jsonb)) WHERE c.project_id=$1 AND c.worktree_id=$2",params![project_id,worktree_id])?;
                connection.execute("UPDATE projects SET revision=revision+1,updated_at=$1,updated_by=$2 WHERE project_id=$3",params![at,actor,project_id])?;
                connection.execute("INSERT INTO project_events(project_id,repository_id,physical_repository_id,target_id,kind,summary,actor,at) VALUES($1,$2,$3,$4,'checkout_removed','The local checkout and its Git registration were removed; tasks and history were retained.',$5,$6)",params![project_id,repository,physical,worktree_id,actor,at])?;
            }
            Ok(())
        }).map_err(database_error)
    }
}

/// Deleting a ref also deletes its own reflog. Prove those commits are still
/// reachable through retained refs before the existing compare-and-swap delete.
fn verify_branch_reflog_preserved(
    root: &Path,
    owner: Option<(u32, u32)>,
    reference: &str,
    current_head: &str,
    approved_unpreserved_git_history: &[String],
) -> Result<(), ProtocolError> {
    let observe = |arguments: &[&str]| {
        let result = repository::run_git_arguments_complete(
            root,
            owner,
            arguments,
            GIT_TIMEOUT,
            1024 * 1024,
        )
        .map_err(|_| conflict("local branch history cannot be completely inspected"))?;
        if !result.status.success() || !result.stderr.is_empty() {
            return Err(conflict(
                "local branch history cannot be completely inspected",
            ));
        }
        Ok(result.stdout)
    };
    let reflog_arguments = [
        "--no-optional-locks",
        "-c",
        "core.hooksPath=/dev/null",
        "reflog",
        "show",
        "--format=%H%x00%gD%x00%gs",
        reference,
    ];
    let before = observe(&reflog_arguments)?;
    let log_name = format!("logs/{reference}");
    let location = observe(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        &log_name,
    ])?;
    let location = std::str::from_utf8(&location)
        .map_err(|_| conflict("local branch history cannot be completely inspected"))?
        .trim();
    let log_path = PathBuf::from(location);
    if !log_path.is_absolute() {
        return Err(conflict(
            "local branch history cannot be completely inspected",
        ));
    }
    let read_raw = || -> Result<(Option<String>, Vec<String>), ProtocolError> {
        match std::fs::symlink_metadata(&log_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((None, Vec::new()));
            }
            Err(_) => {
                return Err(conflict(
                    "local branch history cannot be completely inspected",
                ));
            }
            Ok(_) => {}
        }
        let mut cursor = crate::storage::fs::CheckoutIndexCursor::new_reflog(&log_path)
            .map_err(|_| conflict("local branch history cannot be completely inspected"))?;
        let started = std::time::Instant::now();
        loop {
            if cursor.bytes_hashed() > 1024 * 1024 || started.elapsed() > GIT_TIMEOUT {
                return Err(conflict(
                    "local branch history cannot be completely inspected",
                ));
            }
            if let Some(digest) = cursor
                .step(256 * 1024, std::time::Duration::from_millis(25))
                .map_err(|_| conflict("local branch history cannot be completely inspected"))?
            {
                return Ok((digest, cursor.reflog_roots()));
            }
        }
    };
    let (raw_fingerprint, roots) = read_raw()?;
    if raw_fingerprint.is_none() && !before.is_empty() {
        return Err(conflict(
            "local branch history cannot be completely inspected",
        ));
    }
    let mut tips = roots.into_iter().collect::<BTreeSet<_>>();
    tips.insert(current_head.to_owned());
    let exclusion = format!("--exclude={reference}");
    let mut arguments = vec![
        "--no-optional-locks",
        "-c",
        "core.hooksPath=/dev/null",
        "rev-list",
        "--max-count=4097",
    ];
    arguments.extend(tips.iter().map(String::as_str));
    arguments.extend(["--not", &exclusion, "--all"]);
    let unpreserved = observe(&arguments)?;
    if !unpreserved.is_empty() {
        let approved = std::str::from_utf8(&unpreserved)
            .map_err(|_| conflict("local branch history cannot be completely inspected"))?
            .lines()
            .filter(|oid| !oid.is_empty())
            .map(|oid| format!("commit:{oid}"))
            .collect::<Vec<_>>();
        if approved.iter().any(|oid| {
            !approved_unpreserved_git_history
                .iter()
                .any(|allowed| allowed == oid)
        }) {
            return Err(conflict(
                "local branch reflog contains history not preserved by retained refs",
            ));
        }
    }
    if observe(&reflog_arguments)? != before || read_raw()?.0 != raw_fingerprint {
        return Err(conflict("local branch history changed during retirement"));
    }
    Ok(())
}

fn missing(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::RepositoryNotFound, message)
}
fn conflict(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::ConfigurationConflict, message)
}

fn same_github_source(left: &github::GitHubRepository, right: &github::GitHubRepository) -> bool {
    left.owner().eq_ignore_ascii_case(right.owner())
        && left.name().eq_ignore_ascii_case(right.name())
}

#[cfg(test)]
mod tests;
