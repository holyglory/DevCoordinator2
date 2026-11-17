//! Exact checkout removal contracts. Plans describe effects before execution;
//! an approval is a durable receipt for one unchanged plan, never a force flag.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub worktree_id: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub project_id: String,
    pub idempotency_key: String,
    pub expected_project_revision: u64,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub include_absolute_paths: bool,
}

#[derive(Clone, Debug, Default, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewProgress {
    pub targets_total: u32,
    pub targets_done: u32,
    pub bytes_hashed: u64,
    pub entries_hashed: u64,
    pub current_worktree_id: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewJob {
    pub project_id: String,
    pub job: crate::storage::Job,
    pub progress: ReviewProgress,
    /// Present only after the entire review completes. A blocked plan remains
    /// informational; no partial or failed review enables an effect.
    pub plan: Option<RemovalPlan>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutEffect {
    pub worktree_id: String,
    pub repository_id: String,
    pub revision: u64,
    /// Relative to the project's source/build roots unless an administrator
    /// explicitly requested absolute paths.
    pub path: String,
    pub path_is_absolute: bool,
    pub branch_name: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub independent_clone: bool,
    pub dirty: bool,
    pub untracked_files: u64,
    pub ignored_files: u64,
    pub stash_entries: u64,
    /// All clone reflogs, or only the selected linked worktree's HEAD reflog.
    #[serde(default)]
    pub reflog_entries: u64,
    /// Deduplicated kind:OID labels whose survival after this exact removal
    /// could not be proved. Source bytes and reflog messages are never returned.
    #[serde(default)]
    pub unpreserved_git_history: Vec<String>,
    pub local_refs: u64,
    pub unpublished_refs: Vec<String>,
    pub nested_repositories: bool,
    pub remote_recoverable: bool,
    pub integration_merge_commit: Option<String>,
    pub active_uses: Vec<String>,
    pub protections: Vec<String>,
    pub possible_data_loss: Vec<String>,
    pub blockers: Vec<String>,
    pub estimated_reclaimable_bytes: Option<u64>,
    /// External outputs survive this exact-checkout operation. Ownership of an
    /// association does not itself authorize deleting a different path.
    #[serde(default)]
    pub retained_external_artifacts: u32,
    #[serde(default)]
    pub retained_external_bytes: Option<u64>,
    pub content_fingerprint: String,
    /// Removing a parked or unfinished checkout does not finish its task.
    pub task_stays_open: bool,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemovalPlan {
    pub project_id: String,
    pub project_revision: u64,
    pub plan_id: String,
    pub fingerprint: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub targets: Vec<CheckoutEffect>,
    pub blockers: Vec<String>,
    pub requires_approval: bool,
    pub ready: bool,
    pub estimated_reclaimable_bytes: Option<u64>,
    pub effect: String,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approve {
    pub project_id: String,
    pub plan_id: String,
    pub fingerprint: String,
    pub idempotency_key: String,
    /// Trusted local agents cite the recorded human decision authorizing this
    /// exact loss; an agent's assertion or force flag is not consent.
    #[serde(default)]
    pub consent_decision_id: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalReceipt {
    pub receipt_id: String,
    pub project_id: String,
    pub plan_id: String,
    pub fingerprint: String,
    pub approved_at_ms: u64,
    pub expires_at_ms: u64,
    pub effect: String,
    pub consent_decision_id: Option<String>,
    /// A cited local conversation is trusted recorded provenance; the server
    /// does not claim to authenticate messages in a separate agent runtime.
    pub user_consent_source: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub project_id: String,
    pub plan_id: String,
    pub fingerprint: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub approval_receipt_id: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobReference {
    pub project_id: String,
    pub job_id: String,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutResult {
    pub worktree_id: String,
    pub artifact_id: String,
    pub fully_removed: bool,
    pub task_stays_open: bool,
    pub receipt_id: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemovalJob {
    pub project_id: String,
    pub plan_id: String,
    pub job: crate::storage::Job,
    pub targets: Vec<CheckoutResult>,
}
