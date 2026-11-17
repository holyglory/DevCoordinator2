//! Fresh Git and filesystem observations for exact checkout removal. All
//! deletion continues through HostBackend and the storage job's native locks.
use super::*;
use crate::database::OptionalExtension;
use crate::storage::db_error;
use crate::storage::lifecycle::{BoundCheckout, CheckoutBinding, IntegrationProof};
use devcoordinator2_api::project::cleanup::CheckoutEffect;

struct GitState {
    branch: Option<String>,
    head: String,
    upstream: Option<String>,
    independent: bool,
    dirty: bool,
    untracked: u64,
    ignored: u64,
    stashes: u64,
    refs: u64,
    unpublished: Vec<String>,
    remote_recoverable: bool,
    fingerprint: String,
    merge_commit: Option<String>,
    locked: bool,
}

impl HostBackend {
    pub(super) fn observe_checkout_target(
        &self,
        binding: &CheckoutBinding,
        integration: Option<IntegrationProof>,
        context: &Context,
    ) -> Result<(Record, BoundCheckout), ProtocolError> {
        validate_path(&binding.path, &self.config, false, &self.database)?;
        self.checkout_binding_current(binding)?;
        let identity = fs::identity(&binding.path)?;
        let state = git_state(binding, integration.as_ref(), context)?;
        let snapshot = fs::checkout_snapshot(&binding.path)?;
        let mut record = self.directory_candidate(
            &binding.path,
            &binding.path,
            Some(&binding.canonical_path),
            api::Kind::Worktree,
            true,
            context,
        )?;
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
    ) -> Result<(), ProtocolError> {
        self.validate_checkout_target(record, target, context)?;
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
    let path = &binding.path;
    if !std::fs::symlink_metadata(path.join(".git"))
        .is_ok_and(|metadata| !metadata.file_type().is_symlink())
    {
        return Err(blocked("checkout_git_identity_unverified"));
    }
    let head = git_string(path, &["rev-parse", "--verify", "HEAD"], context)?;
    if !oid(&head) {
        return Err(blocked("checkout_head_unverified"));
    }
    let branch = git_string(path, &["rev-parse", "--abbrev-ref", "HEAD"], context)?;
    let branch = (branch != "HEAD").then_some(branch);
    let upstream = git_string(
        path,
        &["rev-parse", "--symbolic-full-name", "@{upstream}"],
        context,
    )
    .ok();
    let common = git_string(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        context,
    )?;
    let git_dir = git_string(
        path,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
        context,
    )?;
    let index = git_string(
        path,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        context,
    )?;
    if !Path::new(&index).starts_with(&git_dir) {
        return Err(blocked("checkout_index_outside_git_directory"));
    }
    let index_fingerprint = fs::checkout_index_fingerprint(Path::new(&index))?;
    let common_identity = fs::identity(Path::new(&common))?;
    let owner_identity = fs::identity(&binding.canonical_path)?;
    let independent = common == git_dir
        && std::fs::symlink_metadata(path.join(".git")).is_ok_and(|metadata| metadata.is_dir());
    let membership = git_worktree_bytes(path, context)?;
    use std::os::unix::ffi::OsStrExt;
    let mut selected_member = false;
    let mut locked = false;
    for field in membership.split(|byte| *byte == 0) {
        if let Some(member) = field.strip_prefix(b"worktree ") {
            selected_member = member == path.as_os_str().as_bytes();
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
        let canonical_common = git_string(
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
    let status = observed_git_bytes(
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
    let ignored = observed_git_bytes(
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
    let refs = observed_git_bytes(
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
    let stash = observed_git_bytes(
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
    let remote_url = git_bytes(path, &["remote", "get-url", "--", "origin"], context).ok();
    let remote = git_bytes(
        path,
        &["ls-remote", "--heads", "--tags", "--", "origin"],
        context,
    )
    .ok();
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
    let reachable = |hash: &str| {
        remote_heads.iter().any(|remote| {
            hash == remote
                || git_bytes(
                    path,
                    &["merge-base", "--is-ancestor", hash, remote],
                    context,
                )
                .is_ok()
        })
    };
    let mut merge_commit = None;
    let mut integrated = false;
    if let (Some(proof), Some(url), Some(branch)) =
        (integration, remote_url.as_ref(), branch.as_ref())
    {
        if proof.original_head == head
            && proof.branch_name == *branch
            && proof.remote_identity == hash(url)
        {
            let expected = format!("refs/heads/{}", proof.integration_branch);
            if let Some(bytes) = &remote {
                integrated = String::from_utf8_lossy(bytes).lines().any(|line| {
                    let fields = line.split_whitespace().collect::<Vec<_>>();
                    fields.len() == 2
                        && fields[1] == expected
                        && (fields[0] == proof.integration_head
                            || git_bytes(
                                path,
                                &[
                                    "merge-base",
                                    "--is-ancestor",
                                    &proof.merge_commit,
                                    fields[0],
                                ],
                                context,
                            )
                            .is_ok())
                });
            }
            if integrated {
                merge_commit = Some(proof.merge_commit.clone());
            }
        }
    }
    let remote_recoverable = reachable(&head) || integrated;
    let mut unpublished = Vec::new();
    if independent {
        for (name, hash) in &local {
            let approved_original = integrated && hash == &head;
            if !approved_original && !reachable(hash) {
                unpublished.push(name.clone());
            }
        }
    }
    let remote_recoverable = remote_recoverable && (!independent || unpublished.is_empty());
    let fingerprint = hash(
        json(&(
            branch.clone(),
            &head,
            &upstream,
            (&common, common_identity, owner_identity),
            &git_dir,
            (&index, &index_fingerprint),
            &status,
            &ignored,
            &refs,
            &stash,
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
        refs: local_ref_count,
        unpublished,
        remote_recoverable,
        fingerprint,
        merge_commit,
        locked,
    })
}

fn git_string(path: &Path, arguments: &[&str], context: &Context) -> Result<String, ProtocolError> {
    String::from_utf8(git_bytes(path, arguments, context)?)
        .map(|value| value.trim().to_owned())
        .map_err(|_| blocked("checkout_git_unverified"))
}

/// Git can exit successfully after warning that it skipped unreadable paths
/// or refs. That output cannot authorize a complete checkout review.
fn observed_git_bytes(
    path: &Path,
    arguments: &[&str],
    context: &Context,
    incomplete: &str,
) -> Result<Vec<u8>, ProtocolError> {
    let (stdout, stderr) = git_bytes_with_stderr(path, arguments, context)?;
    if !stderr.is_empty() {
        return Err(blocked(incomplete));
    }
    Ok(stdout)
}

fn oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
