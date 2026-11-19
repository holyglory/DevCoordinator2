//! Fresh Git and filesystem observations for exact checkout removal. All
//! deletion continues through HostBackend and the storage job's native locks.
use super::*;
use crate::database::OptionalExtension;
use crate::repository::GIT_AUDIT_OUTPUT_LIMIT;
use crate::storage::db_error;
use crate::storage::lifecycle::{BoundCheckout, CheckoutBinding, IntegrationProof};
use devcoordinator2_api::project::cleanup::CheckoutEffect;

#[derive(Clone)]
struct GitState {
    branch: Option<String>,
    head: String,
    upstream: Option<String>,
    independent: bool,
    dirty: bool,
    untracked: u64,
    ignored: u64,
    stashes: u64,
    reflog_entries: u64,
    unpreserved_history: Vec<String>,
    refs: u64,
    unpublished: Vec<String>,
    remote_recoverable: bool,
    fingerprint: String,
    merge_commit: Option<String>,
    locked: bool,
}

/// One native command or one bounded index read per maintenance step. Cached
/// responses are private observation state, never returned as repository data.
type GitProbeKey = (PathBuf, Vec<String>, bool);
type GitProbeResult = Result<(Vec<u8>, Vec<u8>), (devcoordinator2_api::ErrorCode, String, String)>;
#[derive(Default)]
struct GitProbe {
    results: std::collections::BTreeMap<GitProbeKey, GitProbeResult>,
    files: std::collections::BTreeMap<PathBuf, fs::CheckoutIndexCursor>,
    file_results: std::collections::BTreeMap<PathBuf, Option<String>>,
    used: bool,
}
const GIT_PENDING: &str = "checkout_git_observation_pending";
impl GitProbe {
    fn read(
        &mut self,
        path: &Path,
        args: &[&str],
        context: &Context,
        worktrees: bool,
    ) -> Result<(Vec<u8>, Vec<u8>), ProtocolError> {
        let key = (
            path.to_path_buf(),
            args.iter().map(|s| (*s).to_owned()).collect(),
            worktrees,
        );
        if let Some(result) = self.results.get(&key) {
            return match result {
                Ok(bytes) => Ok(bytes.clone()),
                Err((code, message, detail)) => {
                    Err(ProtocolError::new(*code, message.clone()).with_detail(detail.clone()))
                }
            };
        }
        if self.used {
            return Err(blocked(GIT_PENDING));
        }
        self.used = true;
        // Complete staged listings scale with tracked paths, like worktree lists.
        let limit = if worktrees || matches!(args, ["ls-files", "--stage", "-z"]) {
            WORKTREE_LIST_OUTPUT_LIMIT
        } else {
            GIT_AUDIT_OUTPUT_LIMIT
        };
        let result = git_bytes_with_stderr_limit(path, args, context, Some(limit));
        self.results.insert(
            key,
            match &result {
                Ok(bytes) => Ok(bytes.clone()),
                Err(error) => Err((error.code, error.message.clone(), error.detail.clone())),
            },
        );
        result
    }
    fn bytes(
        &mut self,
        path: &Path,
        args: &[&str],
        context: &Context,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.read(path, args, context, false).map(|(out, _)| out)
    }
    fn optional(
        &mut self,
        path: &Path,
        args: &[&str],
        context: &Context,
    ) -> Result<Option<Vec<u8>>, ProtocolError> {
        match self.bytes(path, args, context) {
            Err(error) if error.message == GIT_PENDING => Err(error),
            result => Ok(result.ok()),
        }
    }
    fn string(
        &mut self,
        path: &Path,
        args: &[&str],
        context: &Context,
    ) -> Result<String, ProtocolError> {
        String::from_utf8(self.bytes(path, args, context)?)
            .map(|value| value.trim().to_owned())
            .map_err(|_| blocked("checkout_git_unverified"))
    }
    fn observed(
        &mut self,
        path: &Path,
        args: &[&str],
        context: &Context,
        incomplete: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        let (stdout, stderr) = self.read(path, args, context, false)?;
        if !stderr.is_empty() {
            return Err(blocked(incomplete));
        }
        Ok(stdout)
    }
    fn index(&mut self, path: &Path) -> Result<Option<String>, ProtocolError> {
        self.file_fingerprint(path, false)
    }
    fn reflog(&mut self, path: &Path) -> Result<Option<String>, ProtocolError> {
        self.file_fingerprint(path, true)
    }
    fn file_fingerprint(
        &mut self,
        path: &Path,
        reflog: bool,
    ) -> Result<Option<String>, ProtocolError> {
        if let Some(result) = self.file_results.get(path) {
            return Ok(result.clone());
        }
        if self.used {
            return Err(blocked(GIT_PENDING));
        }
        self.used = true;
        if let std::collections::btree_map::Entry::Vacant(entry) = self.files.entry(path.to_owned())
        {
            entry.insert(if reflog {
                fs::CheckoutIndexCursor::new_reflog(path)?
            } else {
                fs::CheckoutIndexCursor::new(path)?
            });
        }
        match self
            .files
            .get_mut(path)
            .expect("private Git file cursor initialized")
            .step(256 * 1024, Duration::from_millis(25))?
        {
            Some(result) => {
                self.file_results.insert(path.to_owned(), result.clone());
                Ok(result)
            }
            None => Err(blocked(GIT_PENDING)),
        }
    }
    fn bytes_hashed(&self) -> u64 {
        self.files.values().fold(0u64, |total, file| {
            total.saturating_add(file.bytes_hashed())
        })
    }
    fn step(
        &mut self,
        binding: &CheckoutBinding,
        integration: Option<&IntegrationProof>,
        context: &Context,
    ) -> Result<Option<GitState>, ProtocolError> {
        self.used = false;
        match git_state_probed(binding, integration, context, self) {
            Err(error) if error.message == GIT_PENDING => Ok(None),
            result => result.map(Some),
        }
    }
}

/// Ephemeral handles and digest state are deliberately not restored after a
/// daemon restart. A recovered durable review starts observation again and
/// cannot publish a partial proof acquired before the interruption.
#[derive(Default)]
pub struct CheckoutObservationCursor {
    git_before: GitProbe,
    before: Option<GitState>,
    snapshot: Option<fs::CheckoutSnapshotCursor>,
    content: Option<fs::CheckoutSnapshot>,
    git_after: GitProbe,
    pub(crate) entries_hashed: u64,
    pub(crate) bytes_hashed: u64,
}
impl CheckoutObservationCursor {
    pub(crate) fn content_ready(&self) -> bool {
        self.content.is_some()
    }
}

impl HostBackend {
    pub(super) fn observe_checkout_target(
        &self,
        binding: &CheckoutBinding,
        integration: Option<IntegrationProof>,
        context: &Context,
    ) -> Result<(Record, BoundCheckout), ProtocolError> {
        let mut cursor = CheckoutObservationCursor::default();
        loop {
            if let Some(result) = self.observe_checkout_target_step(
                binding,
                integration.clone(),
                context,
                &mut cursor,
                DiscoveryBudget::default(),
            )? {
                return Ok(result);
            }
        }
    }

    pub(super) fn observe_checkout_target_step(
        &self,
        binding: &CheckoutBinding,
        integration: Option<IntegrationProof>,
        context: &Context,
        cursor: &mut CheckoutObservationCursor,
        budget: DiscoveryBudget,
    ) -> Result<Option<(Record, BoundCheckout)>, ProtocolError> {
        validate_path(&binding.path, &self.config, false, &self.database)?;
        self.checkout_binding_current(binding)?;
        if cursor.before.is_none() {
            cursor.before = cursor
                .git_before
                .step(binding, integration.as_ref(), context)?;
            cursor.bytes_hashed = cursor.git_before.bytes_hashed();
            return Ok(None);
        }
        if cursor.content.is_none() {
            if cursor.snapshot.is_none() {
                cursor.snapshot = Some(fs::CheckoutSnapshotCursor::new(&binding.path)?);
            }
            let walk = cursor
                .snapshot
                .as_mut()
                .expect("checkout snapshot initialized");
            cursor.content = walk.step(budget.entries, 256 * 1024, budget.elapsed)?;
            let (entries, bytes, _) = walk.progress();
            cursor.entries_hashed = entries;
            cursor.bytes_hashed = cursor.git_before.bytes_hashed().saturating_add(bytes);
            return Ok(None);
        }
        let state = cursor
            .git_after
            .step(binding, integration.as_ref(), context)?;
        cursor.bytes_hashed = cursor
            .git_before
            .bytes_hashed()
            .saturating_add(cursor.snapshot.as_ref().map_or(0, |walk| walk.progress().1))
            .saturating_add(cursor.git_after.bytes_hashed());
        let Some(state) = state else {
            return Ok(None);
        };
        if state.fingerprint
            != cursor
                .before
                .as_ref()
                .expect("checkout git observed")
                .fingerprint
        {
            return Err(blocked("checkout_git_changed"));
        }
        let snapshot = cursor.content.take().expect("checkout content observed");
        let identity = fs::identity(&binding.path)?;
        if identity != (snapshot.measurement.device, snapshot.measurement.inode) {
            return Err(blocked("identity_changed"));
        }
        self.checkout_binding_current(binding)?;
        let result =
            self.checkout_record(binding, integration, context, state, snapshot, identity)?;
        Ok(Some(result))
    }

    fn checkout_record(
        &self,
        binding: &CheckoutBinding,
        integration: Option<IntegrationProof>,
        context: &Context,
        state: GitState,
        snapshot: fs::CheckoutSnapshot,
        identity: (u64, u64),
    ) -> Result<(Record, BoundCheckout), ProtocolError> {
        let mut record = candidate(
            api::Kind::Worktree,
            api::Effect::SourceWorktree,
            binding
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            Locator::Directory {
                path: binding.path.clone(),
                device: identity.0,
                inode: identity.1,
                root: binding.path.clone(),
                git_root: Some(binding.canonical_path.clone()),
            },
            context,
        )?;
        record.resource_key = format!("fs:{}:{}", identity.0, identity.1);
        record.artifact.filesystem_id = Some(format!("fs-{}", identity.0));
        record.artifact.allocated_bytes = Some(snapshot.measurement.bytes);
        record.last_activity_signature = format!(
            "{}:{}:{}",
            snapshot.measurement.newest_modified_ns,
            snapshot.measurement.bytes,
            snapshot.measurement.entries
        );
        record.private_aliases.push(binding.path.clone());
        record.artifact.ownership = "recognized".into();
        if let Err(error) = self.validate_mount_consumers(&record, context, &[]) {
            record.blockers.push(error.message);
        }
        record.artifact.repository_id = Some(binding.physical_repository_id.clone());
        record.artifact.repository_name = context
            .repositories
            .iter()
            .find(|repository| repository.id == binding.physical_repository_id)
            .map(|repository| repository.name.clone());
        record.blockers.retain(|blocker| {
            !matches!(
                blocker.as_str(),
                "dirty_worktree"
                    | "unique_worktree_commits"
                    | "git_baseline_unverified"
                    | "unreviewed_ignored_files"
                    | "canonical_checkout"
            )
        });
        record.ancestor_keys = binding
            .path
            .ancestors()
            .skip(1)
            .filter_map(|ancestor| {
                fs::identity(ancestor)
                    .ok()
                    .map(|(device, inode)| format!("fs:{device}:{inode}"))
            })
            .collect();
        let active_uses = checkout_uses(&binding.path, &record, context)?;
        let mut protections = Vec::new();
        if let Some(role) = &binding.protected_role {
            protections.push(format!("protected_{role}_checkout"));
        }
        if binding.path == binding.primary_path
            || binding.path == Path::new("/home/DevCoordinator2")
        {
            protections.push("canonical_checkout".into());
        }
        if snapshot.protected {
            protections.push("protected_source_or_credentials".into());
        }
        if state.locked {
            protections.push("locked_worktree".into());
        }
        if context.protected_resources.contains(&record.resource_key)
            || context.protected_ancestors.contains(&record.resource_key)
            || record
                .ancestor_keys
                .iter()
                .any(|key| context.protected_resources.contains(key))
        {
            protections.push("protected_shared_data".into());
        }
        let mut losses = Vec::new();
        if state.dirty {
            losses.push("uncommitted_changes".into());
        }
        if state.untracked > 0 {
            losses.push("untracked_files".into());
        }
        if state.ignored > 0 {
            losses.push("ignored_files".into());
        }
        if !state.remote_recoverable {
            losses.push("remote_recoverability_unverified".into());
        }
        if !state.unpreserved_history.is_empty() {
            losses.push("unpreserved_local_history".into());
        }
        if state.independent
            && state.stashes > 0
            && state
                .unpublished
                .iter()
                .any(|name| name.starts_with("stash@"))
        {
            losses.push("unpublished_stash_entries".into());
        }
        if state.independent && !state.unpublished.is_empty() {
            losses.push("unpublished_local_refs".into());
        }
        let mut blockers = active_uses
            .iter()
            .chain(&protections)
            .cloned()
            .collect::<Vec<_>>();
        if snapshot.nested_git {
            blockers.push("nested_repository".into());
        }
        blockers.extend(
            record
                .blockers
                .iter()
                .filter(|blocker| blocker.as_str() != "disposal_not_authorized")
                .cloned(),
        );
        blockers.sort();
        blockers.dedup();
        let observed = CheckoutEffect {
            worktree_id: binding.worktree_id.clone(),
            repository_id: binding.repository_id.clone(),
            revision: binding.revision,
            path: String::new(),
            path_is_absolute: false,
            branch_name: state.branch,
            head: Some(state.head),
            upstream: state.upstream,
            independent_clone: state.independent,
            dirty: state.dirty,
            untracked_files: state.untracked,
            ignored_files: state.ignored,
            stash_entries: state.stashes,
            reflog_entries: state.reflog_entries,
            unpreserved_git_history: state.unpreserved_history,
            local_refs: state.refs,
            unpublished_refs: state.unpublished,
            nested_repositories: snapshot.nested_git,
            remote_recoverable: state.remote_recoverable,
            integration_merge_commit: state.merge_commit,
            active_uses,
            protections,
            possible_data_loss: losses,
            blockers,
            estimated_reclaimable_bytes: record.artifact.allocated_bytes,
            content_fingerprint: snapshot.fingerprint,
            retained_external_artifacts: 0,
            retained_external_bytes: Some(0),
            task_stays_open: binding.task_id.is_some()
                || !matches!(
                    binding.lifecycle.as_str(),
                    "merged" | "abandoned" | "retired"
                ),
        };
        let target = BoundCheckout {
            binding: binding.clone(),
            artifact_id: record.artifact.artifact_id.clone(),
            device: identity.0,
            inode: identity.1,
            independent: state.independent,
            observed,
            git_fingerprint: state.fingerprint,
            integration,
            expires_at_ms: context.now_ms + super::super::SAFETY_FRESH_MS,
            data_loss_approved: false,
        };
        Ok((record, target))
    }

    pub(super) fn validate_checkout_target(
        &self,
        record: &Record,
        target: &BoundCheckout,
        context: &Context,
    ) -> Result<(), ProtocolError> {
        let binding = &target.binding;
        if context.now_ms > target.expires_at_ms {
            return Err(blocked("plan_expired"));
        }
        if !target.observed.blockers.is_empty() {
            return Err(blocked("checkout_has_protected_or_active_uses"));
        }
        if binding.path == binding.primary_path
            || binding.path == Path::new("/home/DevCoordinator2")
            || binding.protected_role.is_some()
        {
            return Err(blocked("canonical_checkout"));
        }
        self.checkout_binding_current(binding)?;
        // Linked outputs are retained. A missing or changed association must
        // still be reviewed again before the exact checkout can be removed.
        self.database
            .read(|connection| {
                let (count, bytes, shared_inside) =
                    crate::storage::lifecycle::checkout_artifact_retention(connection, binding)?;
                if shared_inside {
                    return Err(crate::database::DatabaseError::Domain(blocked(
                        "shared_artifact_inside_checkout",
                    )));
                }
                if count != target.observed.retained_external_artifacts
                    || bytes != target.observed.retained_external_bytes
                {
                    return Err(crate::database::DatabaseError::Domain(blocked(
                        "checkout_external_outputs_changed",
                    )));
                }
                Ok(())
            })
            .map_err(db_error)?;
        if fs::identity(&binding.path)? != (target.device, target.inode) {
            return Err(blocked("identity_changed"));
        }
        let state = git_state(binding, target.integration.as_ref(), context)?;
        if state.fingerprint != target.git_fingerprint || state.independent != target.independent {
            return Err(blocked("checkout_git_changed"));
        }
        let snapshot = fs::checkout_snapshot(&binding.path)?;
        if snapshot.fingerprint != target.observed.content_fingerprint {
            return Err(blocked("checkout_contents_changed"));
        }
        if snapshot.protected {
            return Err(blocked("protected_source_or_credentials"));
        }
        if snapshot.nested_git {
            return Err(blocked("nested_repository"));
        }
        if !checkout_uses(&binding.path, record, context)?.is_empty() {
            return Err(blocked("checkout_active_use"));
        }
        if !target.observed.possible_data_loss.is_empty() && !target.data_loss_approved {
            return Err(blocked("exact_data_loss_approval_required"));
        }
        Ok(())
    }

    pub(super) fn remove_checkout_target(
        &self,
        record: &Record,
        target: &BoundCheckout,
        context: &Context,
        job_id: &str,
    ) -> Result<(), ProtocolError> {
        let started = std::time::Instant::now();
        self.validate_checkout_target(record, target, context)?;
        let running = self
            .database
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT state='running' FROM storage_jobs WHERE job_id=$1",
                        [job_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(crate::database::DatabaseError::from)
            })
            .map_err(db_error)?;
        if !running {
            return Err(blocked("cleanup_cancelled"));
        }
        if context
            .now_ms
            .saturating_add(started.elapsed().as_millis() as u64)
            > target.expires_at_ms
        {
            return Err(blocked("plan_expired"));
        }
        // The existing nofollow, inode-bound storage deletion engine removes
        // files. Git then removes only the exact worktree registration. Using
        // Git alone for file deletion would lose the path-replacement guard.
        fs::remove_tree(
            &target.binding.path,
            (target.device, target.inode),
            true,
            true,
        )?;
        if !target.independent {
            let path = target
                .binding
                .path
                .to_str()
                .ok_or_else(|| blocked("non_utf8_path"))?;
            git_bytes(
                &target.binding.canonical_path,
                &["worktree", "remove", "--", path],
                context,
            )
            .map_err(|_| unavailable("worktree_removal_failed"))?;
        }
        if !self.checkout_absent(target, context)? {
            return Err(blocked("checkout_removal_unverified"));
        }
        Ok(())
    }

    fn checkout_binding_current(&self, binding: &CheckoutBinding) -> Result<(), ProtocolError> {
        let binding = binding.clone();
        let repository_id = binding.physical_repository_id.clone();
        let relocating=self.database.read(move|connection| {
            Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM repository_relocations WHERE repository_id=$1 AND state IN ('prepared','verified'))",[repository_id],|row|row.get::<_,bool>(0))?)
        }).map_err(db_error)?;
        if relocating {
            return Err(blocked("checkout_relocation_pending"));
        }
        let current=self.database.read(move |connection| {
            Ok(connection.query_row("SELECT c.absolute_path,c.physical_repository_id,c.revision,c.protected_role,r.root_path,p.absolute_path FROM project_checkouts c JOIN repositories r ON r.repository_id=c.physical_repository_id JOIN project_sources s ON s.project_id=c.project_id AND s.repository_id=c.repository_id LEFT JOIN project_checkouts p ON p.project_id=s.project_id AND p.worktree_id=s.primary_checkout_id WHERE c.project_id=$1 AND c.worktree_id=$2",crate::database::params![binding.project_id,binding.worktree_id],|row|Ok((PathBuf::from(row.get::<_,String>(0)?),row.get::<_,String>(1)?,row.get::<_,u64>(2)?,row.get::<_,Option<String>>(3)?,PathBuf::from(row.get::<_,String>(4)?),row.get::<_,Option<String>>(5)?.map(PathBuf::from)))).optional()?)
        }).map_err(db_error)?;
        let Some((path, physical, revision, protected, canonical, primary)) = current else {
            return Err(blocked("checkout_membership_changed"));
        };
        if path != binding.path
            || physical != binding.physical_repository_id
            || revision != binding.revision
            || protected != binding.protected_role
            || canonical != binding.canonical_path
            || primary.is_some_and(|primary| primary != binding.primary_path)
        {
            return Err(blocked("checkout_membership_changed"));
        }
        Ok(())
    }

    pub(super) fn checkout_absent(
        &self,
        target: &BoundCheckout,
        context: &Context,
    ) -> Result<bool, ProtocolError> {
        if !fs::absent(&target.binding.path)? {
            return Ok(false);
        }
        if target.independent {
            return Ok(true);
        }
        let worktrees = git_worktree_bytes(&target.binding.canonical_path, context)?;
        use std::os::unix::ffi::OsStrExt;
        Ok(!worktrees.split(|byte| *byte == 0).any(|field| {
            field.strip_prefix(b"worktree ") == Some(target.binding.path.as_os_str().as_bytes())
        }))
    }

    pub(super) fn resume_checkout_target(
        &self,
        target: &BoundCheckout,
        context: &Context,
    ) -> Result<(), ProtocolError> {
        if !fs::absent(&target.binding.path)? {
            return Err(blocked("checkout_partial_removal_requires_replan"));
        }
        if target.independent {
            return Ok(());
        }
        if self.checkout_absent(target, context)? {
            return Ok(());
        }
        // Git accepts a missing exact worktree and removes its own registration;
        // prune is deliberately avoided because it acts on unrelated worktrees.
        git_bytes(
            &target.binding.canonical_path,
            &[
                "worktree",
                "remove",
                "--",
                target
                    .binding
                    .path
                    .to_str()
                    .ok_or_else(|| blocked("non_utf8_path"))?,
            ],
            context,
        )?;
        if !self.checkout_absent(target, context)? {
            return Err(blocked("checkout_git_registration_remains"));
        }
        Ok(())
    }
}

fn checkout_uses(
    path: &Path,
    record: &Record,
    context: &Context,
) -> Result<Vec<String>, ProtocolError> {
    let mut uses = Vec::new();
    if context
        .current_paths
        .iter()
        .any(|current| current.starts_with(path) || path.starts_with(current))
    {
        uses.push("current_or_rollback_deployment".into());
    }
    if context
        .active_worktrees
        .iter()
        .any(|current| current.starts_with(path) || path.starts_with(current))
    {
        uses.push("active_test_or_worktree".into());
    }
    if context
        .leased_artifacts
        .contains(&record.artifact.artifact_id)
        || context.leased_resources.contains(&record.resource_key)
        || context.leased_ancestors.contains(&record.resource_key)
        || record
            .ancestor_keys
            .iter()
            .any(|key| context.leased_resources.contains(key))
    {
        uses.push("active_lease".into());
    }
    if processes_reference(&[path.into()])? {
        uses.push("active_process".into());
    }
    if fs::mount_targets()?
        .iter()
        .any(|mount| mount == path || mount.starts_with(path))
    {
        uses.push("mounted_path".into());
    }
    Ok(uses)
}

fn git_state(
    binding: &CheckoutBinding,
    integration: Option<&IntegrationProof>,
    context: &Context,
) -> Result<GitState, ProtocolError> {
    let mut probe = GitProbe::default();
    loop {
        if let Some(state) = probe.step(binding, integration, context)? {
            return Ok(state);
        }
    }
}

fn git_state_probed(
    binding: &CheckoutBinding,
    integration: Option<&IntegrationProof>,
    context: &Context,
    probe: &mut GitProbe,
) -> Result<GitState, ProtocolError> {
    let path = &binding.path;
    if !std::fs::symlink_metadata(path.join(".git"))
        .is_ok_and(|metadata| !metadata.file_type().is_symlink())
    {
        return Err(blocked("checkout_git_identity_unverified"));
    }
    let head = probe.string(path, &["rev-parse", "--verify", "HEAD"], context)?;
    if !oid(&head) {
        return Err(blocked("checkout_head_unverified"));
    }
    let branch = probe.string(path, &["rev-parse", "--abbrev-ref", "HEAD"], context)?;
    let branch = (branch != "HEAD").then_some(branch);
    let upstream = probe.optional(
        path,
        &["rev-parse", "--symbolic-full-name", "@{upstream}"],
        context,
    )?;
    let upstream = upstream
        .map(|bytes| {
            String::from_utf8(bytes)
                .map(|text| text.trim().to_owned())
                .map_err(|_| blocked("checkout_git_unverified"))
        })
        .transpose()?;
    let common = probe.string(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        context,
    )?;
    let git_dir = probe.string(
        path,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
        context,
    )?;
    let index = probe.string(
        path,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        context,
    )?;
    if !Path::new(&index).starts_with(&git_dir) {
        return Err(blocked("checkout_index_outside_git_directory"));
    }
    let index_fingerprint = probe.index(Path::new(&index))?;
    let staged = probe.observed(
        path,
        &["ls-files", "--stage", "-z"],
        context,
        "checkout_index_unverified",
    )?;
    let common_identity = fs::identity(Path::new(&common))?;
    let owner_identity = fs::identity(&binding.canonical_path)?;
    let independent = common == git_dir
        && std::fs::symlink_metadata(path.join(".git")).is_ok_and(|metadata| metadata.is_dir());
    let mut private_reflog_roots = Vec::new();
    let private_head_log = if independent {
        None
    } else {
        let log = Path::new(&git_dir).join("logs/HEAD");
        match std::fs::symlink_metadata(&log) {
            Ok(_) => {
                let digest = probe.reflog(&log)?;
                private_reflog_roots = probe
                    .files
                    .get(&log)
                    .map(fs::CheckoutIndexCursor::reflog_roots)
                    .unwrap_or_default();
                digest
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(blocked("checkout_reflogs_unverified")),
        }
    };
    let membership = probe
        .read(
            path,
            &["worktree", "list", "--porcelain", "-z"],
            context,
            true,
        )?
        .0;
    use std::os::unix::ffi::OsStrExt;
    let mut selected_member = false;
    let mut locked = false;
    let mut other_member_exists = false;
    let mut retained_worktree_heads = Vec::new();
    for field in membership.split(|byte| *byte == 0) {
        if let Some(member) = field.strip_prefix(b"worktree ") {
            selected_member = member == path.as_os_str().as_bytes();
            other_member_exists = !selected_member
                && fs::identity(Path::new(std::ffi::OsStr::from_bytes(member))).is_ok();
        } else if let Some(head) = field.strip_prefix(b"HEAD ") {
            let head = std::str::from_utf8(head)
                .map_err(|_| blocked("checkout_worktree_history_unverified"))?;
            if !oid(head) {
                return Err(blocked("checkout_worktree_history_unverified"));
            }
            if other_member_exists {
                retained_worktree_heads.push(head.to_owned());
            }
        } else if selected_member && (field == b"locked" || field.starts_with(b"locked ")) {
            locked = true;
        }
    }
    if independent {
        if membership
            .split(|byte| *byte == 0)
            .filter(|field| field.starts_with(b"worktree "))
            .count()
            != 1
        {
            return Err(blocked("clone_has_linked_worktrees"));
        }
    } else {
        let canonical_common = probe.string(
            &binding.canonical_path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            context,
        )?;
        if canonical_common != common {
            return Err(blocked("worktree_membership_changed"));
        }
        use std::os::unix::ffi::OsStrExt;
        if !membership
            .split(|byte| *byte == 0)
            .any(|field| field.strip_prefix(b"worktree ") == Some(path.as_os_str().as_bytes()))
        {
            return Err(blocked("worktree_membership_changed"));
        }
    }
    let status = probe.observed(
        path,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        context,
        "checkout_status_unverified",
    )?;
    let dirty = !status.is_empty();
    let untracked = status
        .split(|byte| *byte == 0)
        .filter(|field| field.starts_with(b"?? "))
        .count() as u64;
    let ignored = probe.observed(
        path,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
        context,
        "ignored_files_unverified",
    )?;
    let ignored_count = ignored
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .count() as u64;
    let refs = probe.observed(
        path,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)",
            "refs/",
        ],
        context,
        "checkout_refs_unverified",
    )?;
    let mut local = Vec::new();
    for row in refs
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
    {
        let mut fields = row.split(|byte| *byte == 0);
        let name = String::from_utf8(fields.next().unwrap_or_default().to_vec())
            .map_err(|_| blocked("checkout_refs_unverified"))?;
        let hash = String::from_utf8(fields.next().unwrap_or_default().to_vec())
            .map_err(|_| blocked("checkout_refs_unverified"))?;
        if !oid(&hash) || fields.next().is_some() {
            return Err(blocked("checkout_refs_unverified"));
        }
        local.push((name, hash));
    }
    let stash = probe.observed(
        path,
        &["stash", "list", "--format=%H"],
        context,
        "checkout_stashes_unverified",
    )?;
    let local_ref_count = local.len() as u64;
    let stash_heads =
        String::from_utf8(stash.clone()).map_err(|_| blocked("checkout_stashes_unverified"))?;
    let mut stashes = 0;
    for (index, hash) in stash_heads.lines().enumerate() {
        if !oid(hash) {
            return Err(blocked("checkout_stashes_unverified"));
        }
        local.push((format!("stash@{{{index}}}"), hash.into()));
        stashes += 1;
    }
    let reflog = probe.observed(
        path,
        if independent {
            &["reflog", "show", "--all", "--format=%H%x00%gD%x00%gs"]
        } else {
            &["reflog", "show", "--format=%H%x00%gD%x00%gs", "HEAD"]
        },
        context,
        "checkout_reflogs_unverified",
    )?;
    let mut reflog_heads = private_reflog_roots
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut reflog_entries = 0u64;
    for row in reflog
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
    {
        let fields = row.splitn(3, |byte| *byte == 0).collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(blocked("checkout_reflogs_unverified"));
        }
        let entry =
            std::str::from_utf8(fields[0]).map_err(|_| blocked("checkout_reflogs_unverified"))?;
        if !oid(entry) {
            return Err(blocked("checkout_reflogs_unverified"));
        }
        reflog_entries = reflog_entries.saturating_add(1);
        reflog_heads.insert(entry.to_owned());
    }
    // This runs only in the admitted maintenance lane, with the native Git
    // runner's hard time/output caps. Any incomplete/corrupt object inspection
    // fails review; it cannot be waived as uncertain remote recoverability.
    let unreachable = if independent {
        probe.observed(
            path,
            &[
                "fsck",
                "--full",
                "--unreachable",
                "--no-reflogs",
                "--no-dangling",
                "--no-progress",
            ],
            context,
            "checkout_objects_unverified",
        )?
    } else {
        Vec::new()
    };
    let remote_url = probe.optional(path, &["remote", "get-url", "--", "origin"], context)?;
    let remote = probe.optional(
        path,
        &["ls-remote", "--heads", "--tags", "--", "origin"],
        context,
    )?;
    let mut remote_heads = Vec::new();
    if let Some(bytes) = &remote {
        for line in String::from_utf8_lossy(bytes).lines() {
            let mut fields = line.split_whitespace();
            let hash = fields
                .next()
                .ok_or_else(|| blocked("remote_refs_unverified"))?;
            let name = fields
                .next()
                .ok_or_else(|| blocked("remote_refs_unverified"))?;
            if !oid(hash)
                || !(name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
                || fields.next().is_some()
            {
                return Err(blocked("remote_refs_unverified"));
            }
            remote_heads.push(hash.to_owned());
        }
    }
    let mut merge_commit = None;
    let mut integrated = false;
    if let (Some(proof), Some(url), Some(branch)) =
        (integration, remote_url.as_ref(), branch.as_ref())
        && proof.original_head == head
        && proof.branch_name == *branch
        && proof.remote_identity == hash(url)
    {
        let expected = format!("refs/heads/{}", proof.integration_branch);
        if let Some(bytes) = &remote {
            for line in String::from_utf8_lossy(bytes).lines() {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                if fields.len() == 2
                    && fields[1] == expected
                    && (fields[0] == proof.integration_head
                        || probe
                            .optional(
                                path,
                                &[
                                    "merge-base",
                                    "--is-ancestor",
                                    &proof.merge_commit,
                                    fields[0],
                                ],
                                context,
                            )?
                            .is_some())
                {
                    integrated = true;
                    break;
                }
            }
        }
        if integrated {
            merge_commit = Some(proof.merge_commit.clone());
        }
    }
    let remote_recoverable = reachable(probe, path, &head, &remote_heads, context)? || integrated;
    let mut unpublished = Vec::new();
    if independent {
        for (name, hash) in &local {
            let approved_original = integrated && hash == &head;
            if !approved_original && !reachable(probe, path, hash, &remote_heads, context)? {
                unpublished.push(name.clone());
            }
        }
    }
    let mut preservation_roots = if independent {
        remote_heads.clone()
    } else {
        local
            .iter()
            .map(|(_, oid)| oid.clone())
            .chain(retained_worktree_heads.iter().cloned())
            .collect::<Vec<_>>()
    };
    if integrated {
        preservation_roots.push(head.clone());
    }
    preservation_roots.sort();
    preservation_roots.dedup();
    let mut unpreserved_history = std::collections::BTreeSet::new();
    for entry in reflog_heads {
        if !reachable(probe, path, &entry, &preservation_roots, context)? {
            unpreserved_history.insert(format!("commit:{entry}"));
        }
    }
    let mut unreachable_objects = std::collections::BTreeSet::new();
    for row in std::str::from_utf8(&unreachable)
        .map_err(|_| blocked("checkout_objects_unverified"))?
        .lines()
    {
        let fields = row.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3
            || fields[0] != "unreachable"
            || !matches!(fields[1], "commit" | "tree" | "blob" | "tag")
            || !oid(fields[2])
        {
            return Err(blocked("checkout_objects_unverified"));
        }
        unreachable_objects.insert(format!("{}:{}", fields[1], fields[2]));
        if fields[1] != "commit"
            || !reachable(probe, path, fields[2], &preservation_roots, context)?
        {
            unpreserved_history.insert(format!("{}:{}", fields[1], fields[2]));
        }
    }
    let unpreserved_history = unpreserved_history.into_iter().collect::<Vec<_>>();
    let remote_recoverable = remote_recoverable
        && (!independent || unpublished.is_empty())
        && unpreserved_history.is_empty();
    let fingerprint = hash(
        json(&(
            branch.clone(),
            &head,
            &upstream,
            (&common, common_identity, owner_identity),
            &git_dir,
            (&index, &index_fingerprint, &staged, &private_head_log),
            &status,
            &ignored,
            (&refs, &membership, &retained_worktree_heads),
            (&stash, &reflog, &unreachable_objects, &unpreserved_history),
            &remote_url,
            &remote,
            integrated,
            locked,
        ))?
        .as_bytes(),
    );
    Ok(GitState {
        branch,
        head,
        upstream,
        independent,
        dirty,
        untracked,
        ignored: ignored_count,
        stashes,
        reflog_entries,
        unpreserved_history,
        refs: local_ref_count,
        unpublished,
        remote_recoverable,
        fingerprint,
        merge_commit,
        locked,
    })
}

fn reachable(
    probe: &mut GitProbe,
    path: &Path,
    hash: &str,
    remote_heads: &[String],
    context: &Context,
) -> Result<bool, ProtocolError> {
    for remote in remote_heads {
        if hash == remote
            || probe
                .optional(
                    path,
                    &["merge-base", "--is-ancestor", hash, remote],
                    context,
                )?
                .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
