use base64::Engine;
use devcoordinator2_api::delivery::*;
use devcoordinator2_api::params::{ArtifactCatalog as CatalogParams, ArtifactFile};
use devcoordinator2_api::{ErrorCode, ProtocolError};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

use crate::access::Caller;
use crate::database::Database;
use crate::review::timestamp;
use crate::review_validation::{database_error, invalid, page, text};
use crate::test_artifacts::TestArtifactService;

#[derive(Clone)]
pub(crate) struct DeliveryService {
    database: Database,
    artifacts: TestArtifactService,
}

impl DeliveryService {
    pub(crate) fn new(database: Database, artifacts: TestArtifactService) -> Self {
        Self {
            database,
            artifacts,
        }
    }

    pub(crate) fn deliver(
        &self,
        params: Deliver,
        caller: &Caller,
        actor: &str,
        now_ms: u64,
    ) -> Result<Receipt, ProtocolError> {
        text(&params.target, 1, 100)?;
        let release_id = params.release_id.clone();
        let (repository_id, status) = self.database.call(move |connection| {
            Ok(connection.query_row("SELECT releases.repository_id,status FROM releases JOIN repositories USING(repository_id) WHERE release_id=?1 AND archived_at IS NULL", [&release_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).optional()?)
        }).map_err(database_error)?.ok_or_else(|| ProtocolError::new(ErrorCode::ReleaseNotFound, "No active release"))?;
        let catalog = self.artifacts.catalog(
            CatalogParams {
                path: params.path.clone(),
                run_id: params.run_id.clone(),
                check: params.check.clone(),
                artifact: Some(params.artifact.clone()),
                manifest_sha256: Some(params.manifest_sha256.clone()),
                offset: 0,
                limit: 100,
            },
            caller,
        )?;
        if catalog.repository_id != repository_id || catalog.source_sha256 != params.source_sha256 {
            return Err(invalid(
                "Release repository or expected source does not match retained evidence",
            ));
        }
        if !catalog.run_complete
            || catalog.run_status != "passed"
            || catalog
                .run_finished_at_epoch_ms
                .is_none_or(|finished| finished > now_ms)
        {
            return Err(invalid(
                "Delivery requires a completed passing run with a real finished timestamp",
            ));
        }
        let artifact = catalog
            .artifact
            .as_ref()
            .ok_or_else(|| invalid("Retained artifact was not selected"))?;
        let mut receipt = Receipt {
            receipt_id: String::new(), release_id: params.release_id.clone(), repository_id,
            worktree_id: catalog.worktree_id.clone(), kind: params.kind.clone(), target: params.target.clone(),
            source_sha256: catalog.source_sha256.clone(), config_sha256: catalog.config_sha256.clone(), run_id: params.run_id.clone(),
            check: params.check.clone(), artifact: params.artifact.clone(), artifact_sha256: artifact.sha256.clone(),
            manifest_sha256: catalog.manifest_sha256.clone(), run_metadata_sha256: catalog.run_metadata_sha256.clone(),
            verification_sha256: None, qualification: Qualification::PendingExternalEvidence, checked_at_ms: now_ms,
            qualified: false, verified_at_ms: None,
            delivered_at_ms: None, access: None,
            reason: Some("Artifact integrity is verified; actual delivery/access or executable validation evidence is still missing.".into()),
        };
        if let Some(verification_file) = &params.verification_file {
            let chunk = self.artifacts.file(
                ArtifactFile {
                    path: params.path.clone(),
                    run_id: params.run_id.clone(),
                    check: params.check.clone(),
                    artifact: params.artifact.clone(),
                    file: verification_file.clone(),
                    manifest_sha256: params.manifest_sha256.clone(),
                    offset: 0,
                    max_bytes: 8192,
                },
                caller,
            )?;
            if chunk.next_offset.is_some() || chunk.total_bytes > 8192 {
                return Err(invalid("Delivery verification must fit within 8 KiB"));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&chunk.base64)
                .map_err(|_| invalid("Invalid retained verification encoding"))?;
            let verification: Verification = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("Invalid retained delivery verification contract"))?;
            let file = self.artifacts.file(
                ArtifactFile {
                    path: params.path.clone(),
                    run_id: params.run_id.clone(),
                    check: params.check.clone(),
                    artifact: params.artifact.clone(),
                    file: verification.file.clone(),
                    manifest_sha256: params.manifest_sha256.clone(),
                    offset: 0,
                    max_bytes: 1,
                },
                caller,
            )?;
            let started_ms = self.artifacts.delivery_run_start(
                &params.path,
                &params.run_id,
                &params.check,
                &catalog.run_metadata_sha256,
                caller,
            )?;
            validate_observation(
                &params,
                &verification,
                &file.sha256,
                started_ms,
                catalog.run_finished_at_epoch_ms.unwrap_or(0),
            )?;
            if params.kind == Kind::LocalExecutable
                && verification.access
                    != format!(
                        "artifact://{}/{}/{}/{}/{}",
                        catalog.worktree_id,
                        params.run_id,
                        params.check,
                        params.artifact,
                        verification.file
                    )
            {
                return Err(invalid(
                    "Local executable access must name the exact retained executable",
                ));
            }
            receipt.verification_sha256 = Some(chunk.sha256);
            receipt.qualification = Qualification::Qualified;
            receipt.qualified = true;
            receipt.verified_at_ms = Some(verification.checked_at_ms);
            receipt.delivered_at_ms = Some(verification.checked_at_ms);
            receipt.access = Some(verification.access);
            receipt.reason = None;
        }
        let identity = serde_json::to_vec(&(
            &receipt.release_id,
            &receipt.worktree_id,
            &receipt.kind,
            &receipt.target,
            &receipt.manifest_sha256,
            &receipt.run_metadata_sha256,
            &receipt.artifact,
            &receipt.artifact_sha256,
            &receipt.verification_sha256,
        ))
        .map_err(|_| invalid("Cannot encode delivery identity"))?;
        let digest = Sha256::digest(identity)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        receipt.receipt_id = format!("delivery-{digest}");
        let stored = receipt.clone();
        let actor = actor.to_owned();
        self.database.transaction(move |transaction| {
            let existing = transaction.query_row("SELECT receipt_json FROM release_evidence WHERE receipt_id=?1", [&stored.receipt_id], |row| row.get::<_, String>(0)).optional()?;
            if let Some(existing) = existing {
                return serde_json::from_str(&existing).map_err(|_| invalid("Stored delivery receipt is invalid").into());
            }
            let current: String = transaction.query_row("SELECT status FROM releases WHERE release_id=?1", [&stored.release_id], |row| row.get(0))?;
            if !matches!(current.as_str(), "planned" | "requested") || current != status {
                return Err(invalid("Release changed or is already delivered; create a new release").into());
            }
            let encoded = serde_json::to_string(&stored).map_err(|_| invalid("Cannot encode delivery receipt"))?;
            transaction.execute("INSERT INTO release_evidence(receipt_id,release_id,repository_id,receipt_json) VALUES(?1,?2,?3,?4)",
                rusqlite::params![stored.receipt_id,stored.release_id,stored.repository_id,encoded])?;
            if let Some(delivered_ms) = stored.delivered_at_ms {
                let delivered = timestamp(delivered_ms)?;
                transaction.execute("UPDATE releases SET status='delivered',delivered_at=?1,url=?2,updated_at=?3 WHERE release_id=?4",
                    rusqlite::params![delivered,stored.access,timestamp(now_ms)?,stored.release_id])?;
                transaction.execute("INSERT INTO plan_events(repository_id,subject_kind,subject_id,event,from_value,to_value,actor,at) VALUES(?1,'release',?2,'delivered',?3,?4,?5,?6)",
                    rusqlite::params![stored.repository_id,stored.release_id,current,stored.receipt_id,actor,timestamp(now_ms)?])?;
            }
            Ok(stored)
        }).map_err(database_error)
    }

    pub(crate) fn show(&self, params: Show) -> Result<Page, ProtocolError> {
        page(params.offset, params.limit)?;
        self.database.call(move |connection| {
            let mut query = connection.prepare("SELECT receipt_json FROM release_evidence WHERE release_id=?1 ORDER BY rowid LIMIT ?2 OFFSET ?3")?;
            let mut receipts = query.query_map(rusqlite::params![params.release_id,u32::from(params.limit)+1,params.offset], |row| row.get::<_, String>(0))?
                .map(|row| serde_json::from_str::<Receipt>(&row?).map_err(|_| invalid("Stored delivery receipt is invalid").into()))
                .collect::<Result<Vec<_>, crate::database::DatabaseError>>()?;
            let more = receipts.len() > usize::from(params.limit);
            receipts.truncate(usize::from(params.limit));
            Ok(Page { next_offset: more.then_some(params.offset + receipts.len() as u32), receipts })
        }).map_err(database_error)
    }

    pub(crate) fn receipt(
        &self,
        params: devcoordinator2_api::review::Reference,
    ) -> Result<Receipt, ProtocolError> {
        text(&params.reference, 1, 100)?;
        self.database
            .call(move |connection| {
                let encoded = connection
                    .query_row(
                        "SELECT receipt_json FROM release_evidence WHERE receipt_id=?1",
                        [&params.reference],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .ok_or_else(|| invalid("Delivery receipt not found"))?;
                serde_json::from_str(&encoded)
                    .map_err(|_| invalid("Stored delivery receipt is invalid").into())
            })
            .map_err(database_error)
    }
}

fn validate_observation(
    params: &Deliver,
    proof: &Verification,
    file_sha256: &str,
    started_ms: u64,
    finished_ms: u64,
) -> Result<(), ProtocolError> {
    if proof.version != 1
        || proof.kind != params.kind
        || proof.target != params.target
        || proof.source_sha256 != params.source_sha256
        || proof.observed_sha256 != file_sha256
        || proof.checked_at_ms < started_ms
        || proof.checked_at_ms > finished_ms
    {
        return Err(invalid(
            "Retained delivery observation mismatches kind, target, source, file digest or run timestamp",
        ));
    }
    text(&proof.access, 1, 512)?;
    let permitted = match (&params.kind, &proof.observation) {
        (Kind::Artifact, VerificationObservation::DownloadMatched)
        | (Kind::RegistryPackage, VerificationObservation::RegistryDownloadMatched) => {
            reqwest::Url::parse(&proof.access).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
            })
        }
        (Kind::LocalExecutable, VerificationObservation::ExecutableSmokePassed) => {
            proof.access.starts_with("artifact://") && !proof.access.contains(['?', '#', '@'])
        }
        _ => false,
    };
    if !permitted {
        return Err(invalid(
            "Delivery access or observation does not prove the requested delivery kind",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
