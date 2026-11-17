//! A target-only observation, never a partial whole-inventory discovery.
use super::{
    Backend, DiscoveryBudget, HostBackend, git_bytes, git_bytes_with_stderr, git_worktree_bytes,
    ignored_review,
};
use crate::storage::{
    blocked, fs, hash, json,
    model::{Context, IgnoredReview, Locator, Record},
};
use devcoordinator2_api::{ProtocolError, storage as api};
use std::{collections::BTreeSet, path::Path};

#[derive(Default)]
pub struct ReobserveCursor {
    host: Option<HostReobserve>,
    review_files: Option<Vec<api::IgnoredFileReview>>,
}

struct HostReobserve {
    context: Context,
    context_signature: String,
    baseline: String,
    hashing: Option<ignored_review::JobReviewCursor>,
    review: Option<IgnoredReview>,
    measurement: fs::MeasurementCursor,
    first: Option<fs::Measurement>,
    candidate: Option<Record>,
    phase: u8,
}

impl ReobserveCursor {
    pub(crate) fn with_review(files: Option<Vec<api::IgnoredFileReview>>) -> Self {
        Self {
            host: None,
            review_files: files,
        }
    }

    #[cfg(feature = "root-acceptance")]
    pub(crate) fn awaiting_final_context(&self) -> bool {
        self.host.as_ref().is_some_and(|state| state.phase == 3)
    }

    #[cfg(feature = "root-acceptance")]
    pub(crate) fn awaiting_ignored_hash(&self) -> bool {
        self.host.as_ref().is_some_and(|state| {
            state.phase == 0
                && state
                    .hashing
                    .as_ref()
                    .is_some_and(|hashing| hashing.bytes_hashed() > 0)
        })
    }

    pub(crate) fn needs_context(&self) -> bool {
        self.host.as_ref().is_none_or(|state| state.phase == 3)
    }
}

// Context is re-read after admission and again after measurement. Bind source
// ownership and safety inputs related to this target; unrelated repository
// activity must not make a target-only operation unusable. Full fresh guards
// below independently inspect every current use, mount, pin and lease.
fn context_signature(context: &Context, record: &Record) -> Result<String, ProtocolError> {
    let aliases = &record.private_aliases;
    let related = |path: &std::path::PathBuf| {
        aliases
            .iter()
            .any(|alias| path.starts_with(alias) || alias.starts_with(path))
    };
    let repositories = context
        .repositories
        .iter()
        .filter(|r| record.artifact.repository_id.as_ref() == Some(&r.id))
        .map(|r| (&r.id, &r.root, r.execution_uid))
        .collect::<Vec<_>>();
    let worktrees = context
        .worktrees
        .iter()
        .filter(|(_, path)| related(path))
        .collect::<Vec<_>>();
    let current = context
        .current_paths
        .iter()
        .filter(|path| related(path))
        .collect::<Vec<_>>();
    let active = context
        .active_worktrees
        .iter()
        .filter(|path| related(path))
        .collect::<Vec<_>>();
    let keys = std::iter::once(&record.resource_key)
        .chain(&record.ancestor_keys)
        .collect::<BTreeSet<_>>();
    let resources = |values: &[String]| {
        values
            .iter()
            .filter(|key| keys.contains(key))
            .cloned()
            .collect::<Vec<_>>()
    };
    let value = serde_json::json!({
        "repositories": repositories, "worktrees": worktrees,
        "current_paths": current, "active_worktrees": active,
        "protected_resources": resources(&context.protected_resources),
        "protected_descendant": context.protected_ancestors.contains(&record.resource_key),
        "leased_resources": resources(&context.leased_resources),
        "leased_descendant": context.leased_ancestors.contains(&record.resource_key),
        "leased_target": context.leased_artifacts.contains(&record.artifact.artifact_id),
        "ignored_review": context.ignored_reviews.get(&record.resource_key),
    });
    Ok(hash(json(&value)?.as_bytes()))
}

fn ancestors(record: &Record) -> Result<Vec<String>, ProtocolError> {
    let mut keys = BTreeSet::new();
    for alias in &record.private_aliases {
        for parent in alias.ancestors().skip(1) {
            let (device, inode) = fs::identity(parent)?;
            keys.insert(format!("fs:{device}:{inode}"));
        }
    }
    Ok(keys.into_iter().collect())
}

impl HostBackend {
    pub(super) fn revalidation_membership(
        &self,
        record: &Record,
        path: &Path,
        main: &Path,
        context: &Context,
    ) -> Result<(), ProtocolError> {
        let repository = context
            .repositories
            .iter()
            .find(|r| r.root == main && record.artifact.repository_id.as_ref() == Some(&r.id))
            .ok_or_else(|| blocked("git_owner_unknown"))?;
        if !context
            .worktrees
            .iter()
            .any(|(id, worktree)| id == &repository.id && worktree == path)
        {
            return Err(blocked("worktree_membership_changed"));
        }
        // A database row or stale Git worktree entry alone cannot prove that
        // this directory still belongs to the registered source repository.
        let membership = git_worktree_bytes(main, context)?;
        use std::os::unix::ffi::OsStrExt;
        if !membership
            .split(|b| *b == 0)
            .any(|field| field.strip_prefix(b"worktree ") == Some(path.as_os_str().as_bytes()))
        {
            return Err(blocked("worktree_membership_changed"));
        }
        let common = |root: &Path| -> Result<std::path::PathBuf, ProtocolError> {
            let bytes = git_bytes(
                root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                context,
            )?;
            let text = std::str::from_utf8(&bytes).map_err(|_| blocked("git_owner_unknown"))?;
            let path = Path::new(text.trim_end_matches('\n'));
            if !path.is_absolute() {
                return Err(blocked("git_owner_unknown"));
            }
            Ok(path.into())
        };
        if fs::identity(&common(path)?)? != fs::identity(&common(main)?)? {
            return Err(blocked("worktree_membership_changed"));
        }
        Ok(())
    }

    pub(super) fn reobserve_worktree_step(
        &self,
        record: &Record,
        context: &Context,
        cursor: &mut ReobserveCursor,
        budget: DiscoveryBudget,
    ) -> Result<Option<Record>, ProtocolError> {
        let Locator::Directory {
            path,
            device,
            inode,
            root,
            git_root: Some(main),
        } = &record.locator
        else {
            return Err(blocked("revalidation_target_unsupported"));
        };
        if record.artifact.kind != api::Kind::Worktree
            || record.artifact.effect != api::Effect::SourceWorktree
            || record.discovered_effect != api::Effect::SourceWorktree
        {
            return Err(blocked("revalidation_target_unsupported"));
        }
        if fs::identity(path)? != (*device, *inode)
            || record.resource_key != format!("fs:{device}:{inode}")
        {
            return Err(blocked("identity_changed"));
        }
        for alias in &record.private_aliases {
            if fs::identity(alias)? != (*device, *inode) {
                return Err(blocked("identity_changed"));
            }
        }
        if ancestors(record)? != record.ancestor_keys {
            return Err(blocked("revalidation_inputs_changed"));
        }
        if cursor.host.is_none() {
            self.revalidation_membership(record, path, main, context)?;
            let mut context = context.clone();
            context.discovering = true;
            // Never reuse a whole-scan container snapshot, including on restart.
            context.docker_snapshot = Some(std::sync::Arc::new(self.container_rows()?));
            context.docker_unavailable = false;
            context.discovery_measurements.clear();
            let baseline = self.worktree_baseline(path, main, &context)?;
            let files = cursor.review_files.clone().or_else(|| {
                record
                    .ignored_review
                    .as_ref()
                    .filter(|review| review.job.is_some())
                    .map(|review| {
                        review
                            .files
                            .iter()
                            .map(|file| api::IgnoredFileReview {
                                path: file.path.to_string_lossy().into_owned(),
                                sha256: file.sha256.clone(),
                            })
                            .collect()
                    })
            });
            if cursor.review_files.is_none()
                && record
                    .ignored_review
                    .as_ref()
                    .and_then(|review| review.job.as_ref())
                    .is_some_and(|job| job.source_baseline != baseline)
            {
                return Err(blocked("ignored_review_source_changed"));
            }
            let hashing = files
                .map(|files| {
                    if !record.disposal_approved || record.artifact.ownership != "cleanup_owned" {
                        return Err(blocked("ignored_review_requires_disposal_intent"));
                    }
                    if !git_bytes(
                        path,
                        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                        &context,
                    )?
                    .is_empty()
                    {
                        return Err(blocked("dirty_worktree"));
                    }
                    let (ignored, stderr) = git_bytes_with_stderr(
                        path,
                        &[
                            "ls-files",
                            "--others",
                            "--ignored",
                            "--exclude-standard",
                            "-z",
                        ],
                        &context,
                    )?;
                    if !stderr.is_empty() {
                        return Err(blocked("ignored_files_unverified"));
                    }
                    ignored_review::check_listing(&files, &ignored)?;
                    ignored_review::JobReviewCursor::new(path, files)
                })
                .transpose()?;
            let phase = if hashing.is_some() { 0 } else { 1 };
            cursor.host = Some(HostReobserve {
                context_signature: context_signature(&context, record)?,
                baseline,
                hashing,
                review: record.ignored_review.clone(),
                measurement: fs::MeasurementCursor::new(path)?,
                context,
                first: None,
                candidate: None,
                phase,
            });
            return Ok(None);
        }
        let state = cursor
            .host
            .as_mut()
            .expect("target observation initialized");
        if state.phase == 0 {
            let hashing = state.hashing.as_mut().expect("job review initialized");
            if hashing.step(budget)? {
                let review = state.hashing.take().expect("job review complete").finish(
                    (*device, *inode),
                    state.baseline.clone(),
                    state.context.now_ms,
                );
                state
                    .context
                    .ignored_reviews
                    .insert(record.resource_key.clone(), review.clone());
                state.review = Some(review);
                state.phase = 1;
            }
            return Ok(None);
        }
        if state.phase == 3 {
            let mut fresh = context.clone();
            fresh.discovering = true;
            fresh.docker_snapshot = Some(std::sync::Arc::new(self.container_rows()?));
            fresh.docker_unavailable = false;
            fresh.discovery_measurements = state.context.discovery_measurements.clone();
            let candidate = state.candidate.as_ref().expect("candidate measured");
            // Run every native guard before reporting context drift so a new
            // active consumer, pin, lease or source change keeps its real reason.
            self.validate(candidate, &fresh, &[])?;
            self.revalidation_membership(record, path, main, &fresh)?;
            if context_signature(&fresh, record)? != state.context_signature
                || self.worktree_baseline(path, main, &fresh)? != state.baseline
            {
                return Err(blocked("revalidation_inputs_changed"));
            }
            let mut result = state.candidate.take().expect("candidate measured");
            result.artifact.verified_at_ms = Some(state.context.now_ms);
            state.phase = 4;
            return Ok(Some(result));
        }
        if state.phase == 4 {
            return Err(blocked("revalidation_already_complete"));
        }
        let Some(measured) = state.measurement.step(budget.entries, budget.elapsed)? else {
            return Ok(None);
        };
        if (measured.device, measured.inode) != (*device, *inode) {
            return Err(blocked("identity_changed"));
        }
        if state.phase == 1 {
            state
                .context
                .discovery_measurements
                .insert(path.clone(), measured.clone());
            let mut candidate = self.directory_candidate(
                path,
                root,
                Some(main),
                api::Kind::Worktree,
                true,
                &state.context,
            )?;
            if candidate.locator != record.locator
                || candidate.fingerprint != record.fingerprint
                || candidate.resource_key != record.resource_key
                || candidate.artifact.artifact_id != record.artifact.artifact_id
            {
                return Err(blocked("identity_changed"));
            }
            candidate.artifact = api::Artifact {
                allocated_bytes: candidate.artifact.allocated_bytes,
                verified_at_ms: Some(state.context.now_ms),
                ..record.artifact.clone()
            };
            candidate.private_aliases = record.private_aliases.clone();
            candidate.disposal_approved = record.disposal_approved;
            candidate.disposal_reason = record.disposal_reason.clone();
            candidate.ignored_review = state.review.clone();
            candidate.owner_deployment = record.owner_deployment.clone();
            if candidate.disposal_approved {
                candidate
                    .blockers
                    .retain(|b| b != "disposal_not_authorized");
            }
            self.validate(&candidate, &state.context, &[])?;
            if let Some(blocker) = candidate
                .blockers
                .iter()
                .find(|b| b.as_str() != "disposal_not_authorized")
            {
                return Err(blocked(blocker));
            }
            candidate.ancestor_keys = ancestors(&candidate)?;
            state.candidate = Some(candidate);
            state.first = Some(measured);
            // Repeat the bounded walk; activity during observation is a failed
            // refresh, never a timestamp-only success.
            state.measurement = fs::MeasurementCursor::new(path)?;
            state.phase = 2;
        } else {
            if state.first.as_ref() != Some(&measured) {
                return Err(blocked("activity_changed"));
            }
            state
                .context
                .discovery_measurements
                .insert(path.clone(), measured);
            state.phase = 3;
        }
        Ok(None)
    }
}
