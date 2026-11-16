use super::{
    StorageService, blocked, conflict, db_error, hash, json, locks,
    model::{Context, Locator, Record},
    native, parse, text,
};
use crate::{
    access::Caller,
    database::{DatabaseError, OptionalExtension},
};
use devcoordinator2_api::{ErrorCode, ProtocolError, storage as api};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(super) struct Request {
    pub input: api::Revalidate,
    pub bound: Record,
}

pub(super) struct RevalidationCursor {
    job_id: String,
    bound: Record,
    context: Context,
    native: native::ReobserveCursor,
    _resources: locks::ResourceGuard,
}

fn same_bound(current: &Record, bound: &Record) -> bool {
    current.artifact.artifact_id == bound.artifact.artifact_id
        && current.artifact.removed_at_ms.is_none()
        && current.artifact.revision == bound.artifact.revision
        && current.update_sequence == bound.update_sequence
        && current.locator == bound.locator
        && current.fingerprint == bound.fingerprint
        && current.resource_key == bound.resource_key
}

impl StorageService {
    pub fn revalidate(
        &self,
        input: api::Revalidate,
        caller: &Caller,
    ) -> Result<api::Job, ProtocolError> {
        text(&input.idempotency_key, 200)?;
        let digest = hash(json(&input)?.as_bytes());
        let actor = caller.actor();
        let now = self.now_ms();
        let result = self.database.transaction(|connection| {
            connection.query_row(
                "SELECT 1 FROM pg_advisory_xact_lock(hashtextextended($1,0))",
                [format!("devcoordinator2.storage-revalidate:{}:{}", actor, input.idempotency_key)],
                |row| row.get::<_, i32>(0),
            )?;
            // Recover a lost successful reply before checking a revision that
            // this same successful job necessarily changed.
            if let Some((value, prior)) = connection.query_row(
                "SELECT job_json,request_sha256 FROM storage_jobs WHERE actor=$1 AND kind='revalidate' AND idempotency_key=$2",
                crate::database::params![actor, input.idempotency_key],
                |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?)),
            ).optional()? {
                if prior != digest { return Err(DatabaseError::Domain(conflict("idempotency_key_reused"))); }
                return parse(&value).map_err(DatabaseError::Domain);
            }
            let value = connection.query_row(
                "SELECT record_json FROM storage_artifacts WHERE artifact_id=$1",
                [&input.artifact_id], |row| row.get::<_, String>(0),
            ).optional()?.ok_or_else(|| DatabaseError::Domain(super::not_found("artifact_not_found")))?;
            let bound: Record = parse(&value).map_err(DatabaseError::Domain)?;
            if bound.artifact.revision != input.expected_revision || bound.artifact.removed_at_ms.is_some() {
                return Err(DatabaseError::Domain(conflict("artifact_changed")));
            }
            if bound.artifact.kind != api::Kind::Worktree
                || bound.artifact.effect != api::Effect::SourceWorktree
                || bound.discovered_effect != api::Effect::SourceWorktree
                || !matches!(bound.locator, Locator::Directory { git_root: Some(_), .. })
            {
                return Err(DatabaseError::Domain(blocked("revalidation_target_unsupported")));
            }
            let job = api::Job {
                job_id: super::new_id("sj").map_err(DatabaseError::Domain)?, kind: "revalidate".into(),
                state: api::JobState::Queued, created_at_ms: now, started_at_ms: None, completed_at_ms: None,
                actor: actor.clone(), plan_id: None, receipts: vec![], reclaimed_bytes: 0,
                unmeasured_items: 0, error_code: None,
            };
            let key = input.idempotency_key.clone();
            let request = Request { input, bound };
            connection.execute(
                "INSERT INTO storage_jobs VALUES($1,'revalidate','queued',$2,$3,$4,$5,$6,$7,$8)",
                crate::database::params![job.job_id, key, digest, actor, caller.uid,
                    json(&job).map_err(DatabaseError::Domain)?, json(&request).map_err(DatabaseError::Domain)?, now],
            )?;
            Ok(job)
        }).map_err(db_error)?;
        self.wake.notify_one();
        Ok(result)
    }

    pub(super) fn execute_revalidate_step(
        &self,
        id: &str,
        cursor: &mut Option<RevalidationCursor>,
        budget: native::DiscoveryBudget,
    ) -> Result<bool, ProtocolError> {
        if cursor.as_ref().is_some_and(|state| state.job_id != id) {
            return Err(blocked("storage_revalidation_cursor_mismatch"));
        }
        let job = self.job(id)?;
        if job.kind != "revalidate" {
            return Err(blocked("storage_job_is_not_revalidation"));
        }
        if !matches!(
            job.state,
            api::JobState::Queued | api::JobState::Running | api::JobState::Cancelling
        ) {
            *cursor = None;
            return Ok(true);
        }
        let raw = self
            .database
            .read(|connection| {
                Ok(connection.query_row(
                    "SELECT request_json FROM storage_jobs WHERE job_id=$1",
                    [id],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .map_err(db_error)?;
        let request: Request = parse(&raw)?;
        if matches!(job.state, api::JobState::Cancelling) {
            let committed = self.commit_revalidation(
                id,
                &request.bound,
                Err(blocked("revalidation_cancelled")),
            );
            *cursor = None;
            committed?;
            return Ok(true);
        }
        let result = (|| {
            if cursor.is_none() {
                let resources = self.lock_resources(id, std::slice::from_ref(&request.bound))?;
                if !same_bound(&self.record(&request.input.artifact_id)?, &request.bound) {
                    return Err(conflict("artifact_changed"));
                }
                *cursor = Some(RevalidationCursor {
                    job_id: id.into(),
                    bound: request.bound.clone(),
                    context: self.context()?,
                    native: native::ReobserveCursor::default(),
                    _resources: resources,
                });
            }
            let state = cursor.as_mut().expect("target cursor initialized");
            if !same_bound(&self.record(&request.input.artifact_id)?, &state.bound) {
                return Err(conflict("artifact_changed"));
            }
            #[cfg(feature = "root-acceptance")]
            if state.native.awaiting_final_context()
                && self
                    .config
                    .unit_prefix
                    .starts_with("devcoordinator2-rustint-")
            {
                let barrier = self.config.state_dir.join("storage-pause-revalidation");
                if std::fs::read_to_string(&barrier).ok().as_deref()
                    == Some(request.input.idempotency_key.as_str())
                {
                    std::fs::write(barrier.with_extension("ready"), id)
                        .map_err(|_| blocked("fixture_revalidation_barrier_unavailable"))?;
                    return Ok(None);
                }
            }
            if state.native.needs_context() {
                state.context = self.context()?;
            }
            self.backend
                .reobserve_step(&state.bound, &state.context, &mut state.native, budget)
        })();
        match result {
            Ok(None) => Ok(false),
            Err(error)
                if error.code == ErrorCode::Busy
                    || (error.code == ErrorCode::StorageUnavailable
                        && error.message == "storage_database_unavailable") =>
            {
                Err(error)
            }
            result => {
                let committed = self.commit_revalidation(
                    id,
                    &request.bound,
                    result.map(|record| record.expect("completed target")),
                );
                // A lost/failed commit response is recovered from the durable
                // job on retry. Drop every in-memory probe now: a retry that
                // still needs observations must start with fresh inputs.
                *cursor = None;
                committed?;
                Ok(true)
            }
        }
    }

    /// Record only this target and its receipt in one transaction. A revision
    /// alone is insufficient: unchanged scans also advance update_sequence.
    pub(super) fn commit_revalidation(
        &self,
        id: &str,
        bound: &Record,
        result: Result<Record, ProtocolError>,
    ) -> Result<(), ProtocolError> {
        let now = self.now_ms();
        let job = self.database.transaction(|connection| {
            let raw = connection.query_row("SELECT job_json FROM storage_jobs WHERE job_id=$1 FOR UPDATE", [id], |row| row.get::<_, String>(0))?;
            let mut job: api::Job = parse(&raw).map_err(DatabaseError::Domain)?;
            if !matches!(job.state, api::JobState::Queued | api::JobState::Running | api::JobState::Cancelling) {
                return Ok(job);
            }
            let cancelled = matches!(job.state, api::JobState::Cancelling);
            let mut code = result.as_ref().err().map(|error| error.message.clone());
            let mut refreshed = result.is_ok();
            if !cancelled {
                locks::ensure_tree_editable(connection, &bound.resource_key, &bound.ancestor_keys, Some(id))?;
                let value = connection.query_row("SELECT record_json FROM storage_artifacts WHERE artifact_id=$1 FOR UPDATE", [&bound.artifact.artifact_id], |row| row.get::<_, String>(0)).optional()?;
                let current = value.map(|value| parse::<Record>(&value).map_err(DatabaseError::Domain)).transpose()?;
                if current.as_ref().is_some_and(|current| same_bound(current, bound)) {
                    let mut record = match result {
                        Ok(record) => record,
                        Err(_) => {
                            let mut record = bound.clone();
                            record.artifact.verified_at_ms = None;
                            for reason in ["observation_unavailable", code.as_deref().unwrap_or("revalidation_failed")] {
                                if !record.blockers.iter().any(|b| b == reason) { record.blockers.push(reason.into()); }
                            }
                            record
                        }
                    };
                    if refreshed && record.last_activity_signature != bound.last_activity_signature {
                        record.artifact.last_used_at_ms = Some(now);
                    }
                    record.artifact.revision = bound.artifact.revision + 1;
                    // Allocate at commit, so a scan that started earlier cannot
                    // overwrite this observation even when its revision matched.
                    record.update_sequence = super::next_sequence(connection)?;
                    let changed = connection.execute(
                        "UPDATE storage_artifacts SET revision=$1,record_json=$2,updated_at_ms=$3 WHERE artifact_id=$4 AND revision=$5 AND COALESCE((record_json->>'update_sequence')::bigint,0)=$6",
                        crate::database::params![record.artifact.revision, json(&record).map_err(DatabaseError::Domain)?, now,
                            record.artifact.artifact_id, bound.artifact.revision, bound.update_sequence],
                    )?;
                    if changed != 1 { return Err(DatabaseError::Domain(conflict("artifact_changed"))); }
                    super::change(connection, Some(&bound.artifact.artifact_id), if refreshed { "revalidated" } else { "revalidation_failed" }, &job.actor, now, "{}")?;
                } else {
                    refreshed = false;
                    code = Some("artifact_changed".into());
                }
            }
            let status = if cancelled { "cancelled" } else if refreshed { "revalidated" } else { "blocked" };
            job.state = if cancelled { api::JobState::Cancelled } else if refreshed { api::JobState::Completed } else { api::JobState::Failed };
            job.error_code = if cancelled { None } else { code.clone() };
            job.completed_at_ms = Some(now);
            let receipt = api::ItemReceipt {
                artifact_id: bound.artifact.artifact_id.clone(), status: status.into(),
                code: if cancelled { Some("revalidation_cancelled".into()) } else { code },
                measured_bytes_before: None, removed_at_ms: None, space_change: None,
            };
            connection.execute("INSERT INTO storage_item_receipts VALUES($1,$2,0,$3)", crate::database::params![id, receipt.artifact_id, json(&receipt).map_err(DatabaseError::Domain)?])?;
            job.receipts.push(receipt);
            connection.execute("UPDATE storage_jobs SET state=$1,job_json=$2 WHERE job_id=$3", crate::database::params![super::atom(&job.state), json(&job).map_err(DatabaseError::Domain)?, id])?;
            connection.execute("DELETE FROM storage_resource_locks WHERE job_id=$1", [id])?;
            Ok(job)
        }).map_err(db_error)?;
        self.publish(
            &job,
            if matches!(job.state, api::JobState::Failed) {
                "failed"
            } else {
                "finished"
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{automation_test_support::Fixture, storage::read_tests};

    fn caller() -> Caller {
        Caller {
            via_edge: false,
            pid: std::process::id(),
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
            client_kind: devcoordinator2_api::ClientKind::Other,
            model: None,
            effort: None,
            client_session: None,
            work: None,
            identity: None,
        }
    }

    fn target(service: &StorageService) -> Record {
        let mut record = read_tests::record("target", service.now_ms());
        record.update_sequence = service
            .database
            .transaction(super::super::next_sequence)
            .unwrap();
        record.artifact.kind = api::Kind::Worktree;
        record.artifact.effect = api::Effect::SourceWorktree;
        record.discovered_effect = api::Effect::SourceWorktree;
        record.locator = Locator::Directory {
            path: "/fixture/target".into(),
            device: 1,
            inode: 2,
            root: "/fixture/target".into(),
            git_root: Some("/fixture/main".into()),
        };
        record.disposal_approved = true;
        record.artifact.ownership = "cleanup_owned".into();
        read_tests::save(service, &record);
        record
    }

    fn request(record: &Record, key: &str) -> api::Revalidate {
        api::Revalidate {
            artifact_id: record.artifact.artifact_id.clone(),
            expected_revision: record.artifact.revision,
            idempotency_key: key.into(),
        }
    }

    // The existing root worktree journey exercises actual native probes and
    // cleanup. These focused database integration tests isolate interleavings
    // that are difficult to force through the operating-system journey.
    #[test]
    fn target_receipt_is_atomic_and_retry_precedes_revision_validation() {
        let fixture = Fixture::new();
        let service = read_tests::service(&fixture);
        let bound = target(&service);
        let sibling = read_tests::record("sibling", service.now_ms());
        read_tests::save(&service, &sibling);
        let input = request(&bound, "refresh");
        let job = service.revalidate(input.clone(), &caller()).unwrap();
        let _guard = service
            .lock_resources(&job.job_id, std::slice::from_ref(&bound))
            .unwrap();
        service
            .commit_revalidation(&job.job_id, &bound, Ok(bound.clone()))
            .unwrap();
        let after = service.record("target").unwrap();
        let locks: i64 = service
            .database
            .read(|db| {
                Ok(db.query_row(
                    "SELECT COUNT(*) FROM storage_resource_locks WHERE job_id=$1",
                    [&job.job_id],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            locks, 0,
            "terminal receipt must release ownership atomically"
        );
        assert_eq!(after.artifact.revision, bound.artifact.revision + 1);
        assert!(after.update_sequence > bound.update_sequence);
        assert_eq!(
            json(&service.record("sibling").unwrap()).unwrap(),
            json(&sibling).unwrap()
        );
        let retried = service.revalidate(input, &caller()).unwrap();
        assert_eq!(retried.job_id, job.job_id);
        assert!(matches!(retried.state, api::JobState::Completed));
        assert_eq!(retried.receipts.len(), 1);
        assert_eq!(retried.receipts[0].status, "revalidated");
        assert_eq!(retried.reclaimed_bytes, 0);
        assert!(retried.plan_id.is_none());
        assert_eq!(
            service
                .revalidate(request(&bound, "stale"), &caller())
                .unwrap_err()
                .message,
            "artifact_changed"
        );
        assert_eq!(
            service
                .revalidate(request(&after, "refresh"), &caller())
                .unwrap_err()
                .message,
            "idempotency_key_reused"
        );
        assert!(
            !service
                .discovery_requested
                .load(std::sync::atomic::Ordering::SeqCst)
        );
    }

    #[test]
    fn newer_unchanged_scan_and_newer_failure_survive_a_stale_target_result() {
        for newer_failure in [false, true] {
            for stale_success in [false, true] {
                let fixture = Fixture::new();
                let service = read_tests::service(&fixture);
                let bound = target(&service);
                let job = service
                    .revalidate(request(&bound, "stale-refresh"), &caller())
                    .unwrap();
                let mut newer = bound.clone();
                newer.update_sequence += 10;
                if newer_failure {
                    newer.artifact.revision += 1;
                    newer.artifact.verified_at_ms = None;
                    newer.blockers.push("current_deployment".into());
                }
                read_tests::save(&service, &newer);
                let result = if stale_success {
                    Ok(bound.clone())
                } else {
                    Err(blocked("dirty_worktree"))
                };
                service
                    .commit_revalidation(&job.job_id, &bound, result)
                    .unwrap();
                assert_eq!(
                    json(&service.record("target").unwrap()).unwrap(),
                    json(&newer).unwrap()
                );
                let job = service.job(&job.job_id).unwrap();
                assert!(matches!(job.state, api::JobState::Failed));
                assert_eq!(job.receipts[0].code.as_deref(), Some("artifact_changed"));
            }
        }
    }

    #[test]
    fn older_scan_cannot_overwrite_refreshed_target_or_failure() {
        for failure in [false, true] {
            let fixture = Fixture::new();
            let service = read_tests::service(&fixture);
            let mut bound = target(&service);
            let scan_sequence = service
                .database
                .transaction(super::super::next_sequence)
                .unwrap();
            bound.update_sequence = scan_sequence - 1;
            read_tests::save(&service, &bound);
            let job = service
                .revalidate(request(&bound, "refresh-before-old-scan"), &caller())
                .unwrap();
            let result = if failure {
                Err(blocked("dirty_worktree"))
            } else {
                Ok(bound.clone())
            };
            service
                .commit_revalidation(&job.job_id, &bound, result)
                .unwrap();
            let committed = json(&service.record("target").unwrap()).unwrap();
            let mut scan = super::super::reconcile::ReconcileCursor::new(
                &service,
                super::super::model::Discovery {
                    repository_id: Some("project-alpha".into()),
                    records: vec![bound],
                    complete_kinds: vec![api::Kind::Worktree],
                    ..Default::default()
                },
                scan_sequence,
            )
            .unwrap();
            while !scan
                .step(&service, std::time::Duration::from_secs(1))
                .unwrap()
                .0
            {}
            assert_eq!(json(&service.record("target").unwrap()).unwrap(), committed);
        }
    }

    #[test]
    fn failed_and_cancelled_revalidations_keep_exact_target_receipts() {
        let fixture = Fixture::new();
        let service = read_tests::service(&fixture);
        let bound = target(&service);
        let job = service
            .revalidate(request(&bound, "unsafe-refresh"), &caller())
            .unwrap();
        service
            .commit_revalidation(
                &job.job_id,
                &bound,
                Err(blocked("docker_observation_incomplete")),
            )
            .unwrap();
        let failed = service.record("target").unwrap();
        assert!(failed.artifact.verified_at_ms.is_none());
        assert!(
            failed
                .blockers
                .contains(&"docker_observation_incomplete".into())
        );
        assert!(failed.disposal_approved);
        let receipt = service.job(&job.job_id).unwrap().receipts.remove(0);
        assert_eq!(receipt.status, "blocked");
        assert_eq!(receipt.artifact_id, "target");
        let queued = service
            .revalidate(request(&failed, "queued-cancel"), &caller())
            .unwrap();
        let cancelled = service
            .cancel(
                api::JobReference {
                    job_id: queued.job_id.clone(),
                },
                &caller(),
            )
            .unwrap();
        assert!(matches!(cancelled.state, api::JobState::Cancelled));
        assert_eq!(cancelled.receipts.len(), 1);
        service
            .commit_revalidation(&queued.job_id, &failed, Ok(failed.clone()))
            .unwrap();
        assert_eq!(
            json(&service.record("target").unwrap()).unwrap(),
            json(&failed).unwrap()
        );

        // Force a commit-admission conflict after the native phase. The
        // existing database fixture can express this interleaving without a
        // new provider or weakening production coordination.
        let retry = service
            .revalidate(request(&failed, "retry-after-commit-conflict"), &caller())
            .unwrap();
        let guard = service
            .lock_resources(&retry.job_id, std::slice::from_ref(&failed))
            .unwrap();
        let foreign = service
            .revalidate(request(&failed, "foreign-commit-lock"), &caller())
            .unwrap();
        service
            .database
            .transaction(|db| {
                db.execute(
                    "INSERT INTO storage_resource_locks VALUES($1,$2,1)",
                    crate::database::params![failed.resource_key, foreign.job_id],
                )?;
                Ok(())
            })
            .unwrap();
        let mut cursor = Some(RevalidationCursor {
            job_id: retry.job_id.clone(),
            bound: failed.clone(),
            context: Context::default(),
            native: native::ReobserveCursor::default(),
            _resources: guard,
        });
        assert_eq!(
            service
                .execute_revalidate_step(
                    &retry.job_id,
                    &mut cursor,
                    native::DiscoveryBudget::default()
                )
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        assert!(
            cursor.is_none(),
            "commit retry retained stale native probes"
        );
        let own_locks: i64 = service
            .database
            .read(|db| {
                Ok(db.query_row(
                    "SELECT COUNT(*) FROM storage_resource_locks WHERE job_id=$1",
                    [&retry.job_id],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(own_locks, 0, "failed commit retained target locks");
        service
            .database
            .transaction(|db| {
                db.execute(
                    "DELETE FROM storage_resource_locks WHERE job_id=$1",
                    [&foreign.job_id],
                )?;
                Ok(())
            })
            .unwrap();
        service.execute_job(&retry.job_id).unwrap();
        assert!(matches!(
            service.job(&retry.job_id).unwrap().state,
            api::JobState::Failed
        ));
    }
}
