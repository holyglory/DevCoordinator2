//! Project checkout removal is a narrowly authorized use of the storage
//! cleanup engine. It shares jobs, locks, mutation intents and item receipts.
use super::model::Context;
use super::*;
use devcoordinator2_api::project::cleanup as checkout;
use serde::{Deserialize, Serialize};

pub(crate) const CHECKOUT_EFFECT: &str = "Remove the selected local checkout directories and their Git worktree registrations; preserve tasks, history, deployment data and remote branches.";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckoutBinding {
    pub project_id: String,
    pub repository_id: String,
    pub physical_repository_id: String,
    pub worktree_id: String,
    pub revision: u64,
    pub path: std::path::PathBuf,
    pub canonical_path: std::path::PathBuf,
    pub primary_path: std::path::PathBuf,
    pub protected_role: Option<String>,
    pub lifecycle: String,
    pub branch_name: Option<String>,
    pub task_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntegrationProof {
    pub branch_name: String,
    pub original_head: String,
    pub merge_commit: String,
    pub integration_branch: String,
    pub integration_head: String,
    pub remote_identity: String,
    pub verified_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoundCheckout {
    pub binding: CheckoutBinding,
    pub artifact_id: String,
    pub device: u64,
    pub inode: u64,
    pub independent: bool,
    pub observed: checkout::CheckoutEffect,
    pub git_fingerprint: String,
    pub integration: Option<IntegrationProof>,
    pub expires_at_ms: u64,
    #[serde(default)]
    pub data_loss_approved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CheckoutPlanData {
    pub public: checkout::RemovalPlan,
    pub targets: Vec<BoundCheckout>,
}

/// Durable review input. The request connection only enqueues this bounded
/// identity set; the worker performs the native Git/filesystem observation and
/// writes the reviewed plan before approval can bind to it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CheckoutReviewRequest {
    pub plan: checkout::Plan,
    pub bindings: Vec<CheckoutBinding>,
    pub paths: BTreeMap<String, (String, bool)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredApproval {
    receipt: checkout::ApprovalReceipt,
    idempotency_key: String,
    request_sha256: String,
    actor: String,
    consent_decision_id: Option<String>,
}

/// Called only inside the registry relocation commit transaction after the
/// operator's same-inode move has been verified. No files, immutable plans,
/// historical item receipts or retained evidence bytes are changed here.
pub(crate) fn relocate_source_paths(
    connection: &crate::database::Connection,
    old: &std::path::Path,
    new: &std::path::Path,
    relocation_id: &str,
    now_ms: u64,
) -> Result<(), DatabaseError> {
    let old_text = old
        .to_str()
        .ok_or_else(|| DatabaseError::Domain(invalid("non_utf8_relocation_path")))?;
    let mut query=connection.prepare("SELECT record_json FROM storage_artifacts a WHERE removed_at_ms IS NULL AND EXISTS(SELECT 1 FROM (SELECT value FROM (VALUES(a.record_json#>>'{locator,path}'),(a.record_json#>>'{locator,root}'),(a.record_json#>>'{locator,git_root}'),(a.record_json#>>'{locator,worktree}'),(a.record_json#>>'{locator,source}'),(a.record_json#>>'{locator,target}')) p(value) UNION ALL SELECT value FROM jsonb_array_elements_text(a.record_json->'private_aliases')) paths WHERE value=$1 OR strpos(value,$1||'/')=1 OR strpos($1,value||'/')=1) ORDER BY artifact_id FOR UPDATE")?;
    let values = query
        .query_map([old_text], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let sequence = next_sequence_in(connection)?;
    for value in values {
        let mut record: Record = parse(&value).map_err(DatabaseError::Domain)?;
        let expected = record.artifact.revision;
        locks::ensure_tree_editable(
            connection,
            &record.resource_key,
            &record.ancestor_keys,
            None,
        )?;
        let ancestors = json(&record.ancestor_keys).map_err(DatabaseError::Domain)?;
        let leased=connection.query_row("WITH related AS (SELECT artifact_id FROM storage_artifacts WHERE removed_at_ms IS NULL AND (artifact_id=$1 OR projection_resource_key=$2 OR projection_resource_key IN(SELECT jsonb_array_elements_text($3::jsonb)) OR projection_ancestors @> jsonb_build_array($2))) SELECT EXISTS(SELECT 1 FROM related r JOIN storage_leases l ON l.projection_artifact_ids @> jsonb_build_array(r.artifact_id) WHERE l.projection_expires_at_ms>$4::bigint)",crate::database::params![record.artifact.artifact_id,record.resource_key,ancestors,now_ms as i64],|row|row.get::<_,bool>(0))?;
        let intent=connection.query_row("SELECT EXISTS(SELECT 1 FROM storage_item_intents i JOIN storage_jobs j USING(job_id) WHERE i.artifact_id=$1 AND j.state IN('queued','running','cancelling'))",[&record.artifact.artifact_id],|row|row.get::<_,bool>(0))?;
        if leased || intent {
            return Err(DatabaseError::Domain(blocked(
                "active_storage_use_blocks_relocation",
            )));
        }
        let mut changed = false;
        let remap = |path: &mut std::path::PathBuf| {
            if let Ok(relative) = path.strip_prefix(old) {
                *path = new.join(relative);
                true
            } else {
                false
            }
        };
        match &mut record.locator {
            Locator::Directory {
                path,
                root,
                git_root,
                ..
            } => {
                changed |= remap(path);
                changed |= remap(root);
                if let Some(git) = git_root {
                    changed |= remap(git);
                }
            }
            Locator::Evidence { worktree, .. } => {
                changed |= remap(worktree);
            }
            Locator::Mount { .. } | Locator::Docker { .. } => {
                return Err(DatabaseError::Domain(blocked(
                    "native_consumer_binding_blocks_relocation",
                )));
            }
        }
        for alias in &mut record.private_aliases {
            changed |= remap(alias);
        }
        if !changed {
            continue;
        }
        let mut ancestors = record
            .ancestor_keys
            .iter()
            .filter(|key| !key.starts_with("fs:"))
            .cloned()
            .collect::<BTreeSet<_>>();
        for path in &record.private_aliases {
            for ancestor in path.ancestors() {
                if let Ok((device, inode)) = fs::identity(ancestor) {
                    let key = format!("fs:{device}:{inode}");
                    if key != record.resource_key {
                        ancestors.insert(key);
                    }
                }
            }
        }
        record.ancestor_keys = ancestors.into_iter().collect();
        record.fingerprint = hash(
            json(&(&record.locator, relocation_id))
                .map_err(DatabaseError::Domain)?
                .as_bytes(),
        );
        record.artifact.revision = expected
            .checked_add(1)
            .ok_or_else(|| DatabaseError::Domain(conflict("storage_revision_exhausted")))?;
        record.update_sequence = sequence;
        record.artifact.verified_at_ms = None;
        record.artifact.deletable = false;
        record.artifact.automatic_eligible = false;
        record
            .blockers
            .push("source_relocated_revalidation_required".into());
        record.blockers.sort();
        record.blockers.dedup();
        let encoded = json(&record).map_err(DatabaseError::Domain)?;
        let updated=connection.execute("UPDATE storage_artifacts SET record_json=$1,revision=$2,updated_at_ms=$3 WHERE artifact_id=$4 AND revision=$5",crate::database::params![encoded,record.artifact.revision as i64,now_ms as i64,record.artifact.artifact_id,expected as i64])?;
        if updated != 1 {
            return Err(DatabaseError::Domain(conflict("artifact_changed")));
        }
        let event = json(&serde_json::json!({"relocation_id":relocation_id}))
            .map_err(DatabaseError::Domain)?;
        connection.execute("INSERT INTO storage_changes(artifact_id,kind,actor,created_at_ms,change_json) VALUES($1,'source_paths_relocated',$2,$3,$4)",crate::database::params![record.artifact.artifact_id,format!("relocation:{relocation_id}"),now_ms as i64,event])?;
    }
    Ok(())
}

impl StorageService {
    pub(crate) fn queue_checkout_review(
        &self,
        request: CheckoutReviewRequest,
        caller: &Caller,
    ) -> Result<api::Job, ProtocolError> {
        self.queue(
            "checkout_review",
            &request.plan.idempotency_key,
            &request,
            caller,
            None,
        )
    }

    pub(crate) fn checkout_plan(
        &self,
        request: &checkout::Plan,
        bindings: Vec<CheckoutBinding>,
        paths: BTreeMap<String, (String, bool)>,
        caller: &Caller,
    ) -> Result<checkout::RemovalPlan, ProtocolError> {
        if bindings.is_empty() || bindings.len() > MAX_SELECTION {
            return Err(invalid("selection_requires_1_to_200_checkouts"));
        }
        let context = self.context()?;
        let now = self.now_ms();
        let expires = now.saturating_add(SAFETY_FRESH_MS);
        let mut records = Vec::new();
        let mut targets = Vec::new();
        for binding in bindings {
            let integration = self.checkout_integration(&binding)?;
            let (mut record, mut target) =
                self.backend
                    .observe_checkout(&binding, integration, &context)?;
            target.expires_at_ms = expires;
            let (display_path, absolute) = paths
                .get(&binding.worktree_id)
                .ok_or_else(|| invalid("checkout_display_path_missing"))?;
            target.observed.path = display_path.clone();
            target.observed.path_is_absolute = *absolute;
            // An observation cannot silently release an existing pin, lease,
            // or a lock held by a different cleanup operation.
            let current = self.reconcile_matches(&[record.artifact.artifact_id.clone()])?;
            if let Some(old) = current.get(&record.artifact.artifact_id) {
                if old.artifact.removed_at_ms.is_some() {
                    return Err(conflict("checkout_identity_previously_removed"));
                }
                record.artifact.protected = old.artifact.protected;
                record.artifact.revision = old.artifact.revision.saturating_add(1);
                record.disposal_approved = old.disposal_approved;
                record.disposal_reason = old.disposal_reason.clone();
                if old.artifact.protected {
                    target
                        .observed
                        .protections
                        .push("explicitly_protected".into());
                    target.observed.blockers.push("explicitly_protected".into());
                }
            }
            records.push(record);
            targets.push(target);
        }
        let mut effects = targets
            .iter()
            .map(|t| t.observed.clone())
            .collect::<Vec<_>>();
        effects.sort_by(|a, b| a.worktree_id.cmp(&b.worktree_id));
        let requires_approval = effects
            .iter()
            .any(|target| !target.possible_data_loss.is_empty());
        let blockers = effects
            .iter()
            .flat_map(|target| target.blockers.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let total = effects.iter().try_fold(0u64, |sum, target| {
            target
                .estimated_reclaimable_bytes
                .and_then(|bytes| sum.checked_add(bytes))
        });
        let fingerprint = hash(
            json(&(
                &request.project_id,
                request.expected_project_revision,
                &effects,
                CHECKOUT_EFFECT,
            ))?
            .as_bytes(),
        );
        let public = checkout::RemovalPlan {
            project_id: request.project_id.clone(),
            project_revision: request.expected_project_revision,
            plan_id: new_id("sp")?,
            fingerprint,
            created_at_ms: now,
            expires_at_ms: expires,
            targets: effects,
            blockers: blockers.clone(),
            requires_approval,
            ready: blockers.is_empty() && !requires_approval,
            estimated_reclaimable_bytes: total,
            effect: CHECKOUT_EFFECT.into(),
        };
        if json(&public)?.len() > 190 * 1024 {
            return Err(invalid("checkout_plan_too_large_select_fewer_targets"));
        }
        let plan = StoredPlan {
            public: api::CleanupPlan {
                plan_id: public.plan_id.clone(),
                created_at_ms: now,
                expires_at_ms: expires,
                ready: blockers.is_empty(),
                automatic: false,
                includes_persistent_data: true,
                reclaimable_bytes: total.unwrap_or(0),
                items: records
                    .iter()
                    .zip(&targets)
                    .map(|(record, target)| api::PlanItem {
                        artifact_id: record.artifact.artifact_id.clone(),
                        revision: record.artifact.revision,
                        name: record.artifact.name.clone(),
                        kind: record.artifact.kind,
                        effect: record.artifact.effect,
                        allocated_bytes: record.artifact.allocated_bytes,
                        blockers: target.observed.blockers.clone(),
                    })
                    .collect(),
            },
            records: records.clone(),
            policies: BTreeMap::new(),
            checkout_removal: Some(CheckoutPlanData {
                public: public.clone(),
                targets,
            }),
        };
        let value = json(&plan)?;
        let actor = caller.actor();
        self.database.transaction(move |connection| {
            for mut record in records {
                locks::ensure_tree_editable(connection,&record.resource_key,&record.ancestor_keys,None)?;
                record.update_sequence=next_sequence(connection)?;
                let encoded=json(&record).map_err(DatabaseError::Domain)?;
                connection.execute("INSERT INTO storage_artifacts(artifact_id,repository_id,kind,revision,removed_at_ms,record_json,updated_at_ms) VALUES($1,$2,$3,$4,NULL,$5,$6) ON CONFLICT(artifact_id) DO UPDATE SET record_json=excluded.record_json,revision=excluded.revision,updated_at_ms=excluded.updated_at_ms WHERE storage_artifacts.revision=$7 AND storage_artifacts.removed_at_ms IS NULL",crate::database::params![record.artifact.artifact_id,record.artifact.repository_id,atom(record.artifact.kind),record.artifact.revision as i64,encoded,now as i64,record.artifact.revision.saturating_sub(1) as i64])?;
                let saved:i64=connection.query_row("SELECT revision FROM storage_artifacts WHERE artifact_id=$1",[&record.artifact.artifact_id],|row|row.get(0))?;
                if saved as u64!=record.artifact.revision {
                    return Err(DatabaseError::Domain(conflict("artifact_changed")));
                }
            }
            connection.execute("INSERT INTO storage_plans(plan_id,plan_json,created_at_ms,actor) VALUES($1,$2,$3,$4)",crate::database::params![plan.public.plan_id,value,now as i64,actor])?;
            change(connection,None,"checkout_removal_planned",&actor,now,&json(&plan.checkout_removal.as_ref().map(|data|(&data.public.project_id,&data.public.plan_id,&data.public.fingerprint))).map_err(DatabaseError::Domain)?)?;
            Ok(())
        }).map_err(db_error)?;
        Ok(public)
    }

    pub(crate) fn checkout_approve(
        &self,
        request: checkout::Approve,
        caller: &Caller,
    ) -> Result<checkout::ApprovalReceipt, ProtocolError> {
        text(&request.idempotency_key, 128)?;
        let digest = hash(json(&request)?.as_bytes());
        if let Some(prior) =
            self.checkout_prior_approval(&caller.actor(), &request.idempotency_key)?
        {
            if prior.request_sha256 != digest {
                return Err(conflict("idempotency_key_reused"));
            }
            return Ok(prior.receipt);
        }
        let plan = self.stored_plan(&request.plan_id)?;
        let data =
            self.checkout_validate_request(&plan, &request.project_id, &request.fingerprint)?;
        self.checkout_revalidate(&plan, data, false)?;
        let consent_source = self.checkout_consent(caller, &request, data)?;
        let now = self.now_ms();
        let mut approval = StoredApproval {
            receipt: checkout::ApprovalReceipt {
                receipt_id: String::new(),
                project_id: request.project_id.clone(),
                plan_id: request.plan_id.clone(),
                fingerprint: request.fingerprint.clone(),
                approved_at_ms: now,
                expires_at_ms: data.public.expires_at_ms,
                effect: CHECKOUT_EFFECT.into(),
                consent_decision_id: request.consent_decision_id.clone(),
                user_consent_source: consent_source,
            },
            idempotency_key: request.idempotency_key.clone(),
            request_sha256: digest.clone(),
            actor: caller.actor(),
            consent_decision_id: request.consent_decision_id,
        };
        self.database.transaction(move |connection| {
            connection.query_row("SELECT pg_advisory_xact_lock(hashtextextended($1,0))",[format!("checkout-approval:{}:{}",approval.actor,approval.idempotency_key)],|_|Ok(()))?;
            let prior=connection.query_row("SELECT change_json FROM storage_changes WHERE kind='checkout_loss_approved' AND actor=$1 AND change_json->>'idempotency_key'=$2 ORDER BY sequence LIMIT 1",crate::database::params![approval.actor,approval.idempotency_key],|row|row.get::<_,String>(0)).optional()?;
            if let Some(prior)=prior {
                let prior:StoredApproval=parse(&prior).map_err(DatabaseError::Domain)?;
                if prior.request_sha256!=digest { return Err(DatabaseError::Domain(conflict("idempotency_key_reused"))); }
                return Ok(prior.receipt);
            }
            let sequence:i64=connection.query_row("INSERT INTO storage_changes(artifact_id,kind,actor,created_at_ms,change_json) VALUES(NULL,'checkout_loss_approved',$1,$2,'{}') RETURNING sequence",crate::database::params![approval.actor,now as i64],|row|row.get(0))?;
            approval.receipt.receipt_id=format!("ca{sequence}");
            connection.execute("UPDATE storage_changes SET change_json=$1 WHERE sequence=$2",crate::database::params![json(&approval).map_err(DatabaseError::Domain)?,sequence])?;
            Ok(approval.receipt)
        }).map_err(db_error)
    }

    pub(crate) fn checkout_start(
        &self,
        request: checkout::Start,
        caller: &Caller,
    ) -> Result<api::Job, ProtocolError> {
        let digest = hash(json(&request)?.as_bytes());
        let actor = caller.actor();
        let key = request.idempotency_key.clone();
        let prior=self.database.read(move |connection| {
            Ok(connection.query_row("SELECT job_json,request_sha256 FROM storage_jobs WHERE actor=$1 AND kind='cleanup' AND idempotency_key=$2",crate::database::params![actor,key],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()?)
        }).map_err(db_error)?;
        if let Some((value, old_digest)) = prior {
            if digest != old_digest {
                return Err(conflict("idempotency_key_reused"));
            }
            return parse(&value);
        }
        let plan = self.stored_plan(&request.plan_id)?;
        let data =
            self.checkout_validate_request(&plan, &request.project_id, &request.fingerprint)?;
        let approved = self.checkout_approval_for_start(data, &request, false)?;
        self.checkout_revalidate(&plan, data, approved)?;
        self.queue(
            "cleanup",
            &request.idempotency_key,
            &request,
            caller,
            Some(request.plan_id.clone()),
        )
    }

    pub(crate) fn checkout_job(
        &self,
        project_id: &str,
        job_id: &str,
    ) -> Result<(api::Job, CheckoutPlanData), ProtocolError> {
        let job = self.job(job_id)?;
        let plan = self.stored_plan(
            job.plan_id
                .as_deref()
                .ok_or_else(|| invalid("checkout_plan_missing"))?,
        )?;
        let data = plan
            .checkout_removal
            .ok_or_else(|| invalid("job_is_not_checkout_removal"))?;
        if data.public.project_id != project_id {
            return Err(not_found("checkout_job_not_found"));
        }
        Ok((job, data))
    }

    pub(crate) fn checkout_target_absent(
        &self,
        target: &BoundCheckout,
    ) -> Result<bool, ProtocolError> {
        let record = self.record(&target.artifact_id)?;
        let mut context = self.context()?;
        context
            .checkout_removals
            .insert(target.artifact_id.clone(), target.clone());
        self.backend.absent(&record, &context)
    }

    pub(super) fn checkout_job_context(
        &self,
        plan: &StoredPlan,
        job: &api::Job,
        context: &mut Context,
    ) -> Result<(), ProtocolError> {
        let Some(data) = &plan.checkout_removal else {
            return Ok(());
        };
        let job_id = job.job_id.clone();
        let request = self
            .database
            .read(move |connection| {
                Ok(connection.query_row(
                    "SELECT request_json FROM storage_jobs WHERE job_id=$1",
                    [job_id],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .map_err(db_error)?;
        let request: checkout::Start = parse(&request)?;
        if request.plan_id != plan.public.plan_id
            || request.project_id != data.public.project_id
            || request.fingerprint != data.public.fingerprint
        {
            return Err(conflict("checkout_plan_changed"));
        }
        // Historical authorization may explain an already-started effect
        // after a crash. Native validation still enforces plan expiry before
        // any new file deletion; only exact absent-path Git repair can resume.
        let approved = self.checkout_approval_for_start(data, &request, true)?;
        for target in &data.targets {
            let mut target = target.clone();
            target.data_loss_approved = approved;
            context
                .checkout_removals
                .insert(target.artifact_id.clone(), target);
        }
        Ok(())
    }

    fn checkout_validate_request<'a>(
        &self,
        plan: &'a StoredPlan,
        project_id: &str,
        fingerprint: &str,
    ) -> Result<&'a CheckoutPlanData, ProtocolError> {
        let data = plan
            .checkout_removal
            .as_ref()
            .ok_or_else(|| invalid("plan_is_not_checkout_removal"))?;
        if data.public.project_id != project_id || data.public.fingerprint != fingerprint {
            return Err(conflict("checkout_plan_changed"));
        }
        if self.now_ms() > data.public.expires_at_ms {
            return Err(conflict("plan_expired"));
        }
        if !data.public.blockers.is_empty() {
            return Err(blocked("plan_has_blockers"));
        }
        let project = project_id.to_owned();
        let revision = self
            .database
            .read(move |connection| {
                Ok(connection
                    .query_row(
                        "SELECT revision FROM projects WHERE project_id=$1",
                        [project],
                        |row| row.get::<_, u64>(0),
                    )
                    .optional()?)
            })
            .map_err(db_error)?;
        if revision != Some(data.public.project_revision) {
            return Err(conflict("project_changed"));
        }
        Ok(data)
    }

    fn checkout_revalidate(
        &self,
        plan: &StoredPlan,
        data: &CheckoutPlanData,
        approved: bool,
    ) -> Result<(), ProtocolError> {
        let mut context = self.context()?;
        for target in &data.targets {
            let mut target = target.clone();
            // Reviewing a loss approval may observe risky data, but cannot
            // bypass identity, content or any active/protected-use check.
            target.data_loss_approved = approved || data.public.requires_approval;
            context
                .checkout_removals
                .insert(target.artifact_id.clone(), target);
        }
        for original in &plan.records {
            let current = self.record(&original.artifact.artifact_id)?;
            if original.artifact.revision != current.artifact.revision
                || original.fingerprint != current.fingerprint
                || original.resource_key != current.resource_key
            {
                return Err(conflict("artifact_changed"));
            }
            self.backend.validate(&current, &context, &[])?;
        }
        Ok(())
    }

    fn checkout_approval_for_start(
        &self,
        data: &CheckoutPlanData,
        request: &checkout::Start,
        recovery: bool,
    ) -> Result<bool, ProtocolError> {
        if !data.public.requires_approval {
            return Ok(false);
        }
        let id = request
            .approval_receipt_id
            .as_deref()
            .ok_or_else(|| blocked("exact_data_loss_approval_required"))?;
        let sequence = id
            .strip_prefix("ca")
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| invalid("approval_receipt_invalid"))?;
        let value=self.database.read(move |connection|Ok(connection.query_row("SELECT change_json FROM storage_changes WHERE sequence=$1 AND kind='checkout_loss_approved'",[sequence],|row|row.get::<_,String>(0)).optional()?)).map_err(db_error)?.ok_or_else(||blocked("approval_receipt_not_found"))?;
        let approval: StoredApproval = parse(&value)?;
        if approval.receipt.project_id != request.project_id
            || approval.receipt.plan_id != request.plan_id
            || approval.receipt.fingerprint != request.fingerprint
            || approval.receipt.effect != CHECKOUT_EFFECT
        {
            return Err(conflict("approval_receipt_target_changed"));
        }
        if !recovery && self.now_ms() > approval.receipt.expires_at_ms {
            return Err(conflict("approval_receipt_expired"));
        }
        Ok(true)
    }

    fn checkout_prior_approval(
        &self,
        actor: &str,
        key: &str,
    ) -> Result<Option<StoredApproval>, ProtocolError> {
        let actor = actor.to_owned();
        let key = key.to_owned();
        let value=self.database.read(move |connection|Ok(connection.query_row("SELECT change_json FROM storage_changes WHERE kind='checkout_loss_approved' AND actor=$1 AND change_json->>'idempotency_key'=$2 ORDER BY sequence LIMIT 1",crate::database::params![actor,key],|row|row.get::<_,String>(0)).optional()?)).map_err(db_error)?;
        value.map(|value| parse(&value)).transpose()
    }

    fn checkout_consent(
        &self,
        caller: &Caller,
        request: &checkout::Approve,
        data: &CheckoutPlanData,
    ) -> Result<Option<String>, ProtocolError> {
        // Security assumptions grant authenticated Console administrators the
        // action itself. Local agent identity is never substituted for a human.
        if caller.via_edge && caller.identity.is_some() {
            let identity = caller.identity.clone().unwrap_or_default();
            let administrator = self
                .database
                .read(move |connection| {
                    Ok(connection
                        .query_row(
                            "SELECT administrator FROM users WHERE email=$1",
                            [identity],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()?
                        .is_some_and(|value| value != 0))
                })
                .map_err(db_error)?;
            if administrator {
                return Ok(None);
            }
            return Err(ProtocolError::new(
                devcoordinator2_api::ErrorCode::PermissionDenied,
                "checkout loss approval requires Console administrator",
            ));
        }
        let decision = request
            .consent_decision_id
            .clone()
            .ok_or_else(|| blocked("recorded_human_consent_required"))?;
        // Same-owner agents may record the actual user's exact instruction.
        // The structured citation is trusted provenance, not an assertion that
        // this server independently authenticated a foreign chat message.
        let project = data.public.project_id.clone();
        let plan = request.plan_id.clone();
        let fingerprint = request.fingerprint.clone();
        let note=self.database.read(move |connection| {
            Ok(connection.query_row("SELECT d.technical_note FROM decisions d JOIN project_repository_memberships m ON m.physical_repository_id=d.repository_id WHERE m.project_id=$1 AND d.decision_id=$2 AND d.superseded_by IS NULL",crate::database::params![project,decision],|row|row.get::<_,Option<String>>(0)).optional()?.flatten())
        }).map_err(db_error)?.and_then(|note|serde_json::from_str::<serde_json::Value>(&note).ok()).ok_or_else(||blocked("recorded_human_consent_not_bound_to_plan"))?;
        let source = note
            .get("user_consent_source")
            .and_then(serde_json::Value::as_str)
            .filter(|source| {
                !source.is_empty()
                    && source.len() <= 512
                    && source.contains(':')
                    && !source.chars().any(char::is_whitespace)
            });
        if note.get("kind").and_then(serde_json::Value::as_str)
            != Some("project_checkout_data_loss_consent")
            || note.get("project_id").and_then(serde_json::Value::as_str)
                != Some(data.public.project_id.as_str())
            || note.get("plan_id").and_then(serde_json::Value::as_str) != Some(plan.as_str())
            || note.get("fingerprint").and_then(serde_json::Value::as_str)
                != Some(fingerprint.as_str())
            || note.get("effect").and_then(serde_json::Value::as_str) != Some(CHECKOUT_EFFECT)
            || source.is_none()
        {
            return Err(blocked("recorded_human_consent_not_bound_to_plan"));
        }
        Ok(source.map(str::to_owned))
    }

    fn checkout_integration(
        &self,
        binding: &CheckoutBinding,
    ) -> Result<Option<IntegrationProof>, ProtocolError> {
        let repository = binding.physical_repository_id.clone();
        let branch = binding.branch_name.clone();
        let value=self.database.read(move |connection|Ok(connection.query_row("SELECT change_json->>'proof' FROM storage_changes WHERE kind='checkout_integration_verified' AND change_json->>'repository_id'=$1 AND change_json->>'branch_name'=$2 ORDER BY sequence DESC LIMIT 1",crate::database::params![repository,branch],|row|row.get::<_,String>(0)).optional()?)).map_err(db_error)?;
        value.map(|value| parse(&value)).transpose()
    }
}
