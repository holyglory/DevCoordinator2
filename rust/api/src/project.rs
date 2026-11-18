//! Persistent product projects, explicit source membership, and cached explorer views.
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
pub mod artifacts;
pub mod cleanup;
pub mod relocation;
fn default_limit() -> u16 {
    50
}
fn default_true() -> bool {
    true
}
/// Missing means keep; an explicit JSON null means clear.
fn patch<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct List {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub after_project_id: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u16,
}
impl Default for List {
    fn default() -> Self {
        Self {
            query: None,
            after_project_id: None,
            limit: default_limit(),
        }
    }
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub project_id: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchItem {
    pub project_id: String,
    pub branch_id: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutItem {
    pub project_id: String,
    pub worktree_id: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub project_id: String,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub lifecycle: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u16,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchReference {
    pub project_id: String,
    pub branch_id: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u16,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchUpdate {
    pub project_id: String,
    pub branch_id: String,
    pub expected_revision: u32,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub intent: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub outcome: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub owner: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_action: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub task_id: Option<Option<String>>,
    #[serde(default)]
    pub lifecycle: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutUpdate {
    pub project_id: String,
    pub worktree_id: String,
    pub expected_revision: u32,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub intent: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub outcome: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub owner: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_action: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub task_id: Option<Option<String>>,
    #[serde(default)]
    pub lifecycle: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configure {
    #[serde(default)]
    pub project_id: Option<String>,
    pub expected_revision: u32,
    pub name: String,
    pub slug: String,
    pub roots: Vec<RootInput>,
    pub repositories: Vec<RepositoryInput>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootInput {
    pub kind: String,
    pub label: String,
    pub path: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryInput {
    /// Existing canonical source identity. Its histories are never rewritten.
    pub repository_id: String,
    pub name: String,
    #[serde(default)]
    pub github_url: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
    /// Registered physical clones explicitly assigned to this source.
    pub physical_repository_ids: Vec<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refresh {
    pub project_id: String,
    pub idempotency_key: String,
    #[serde(default = "default_true")]
    pub include_remote: bool,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobReference {
    pub project_id: String,
    pub job_id: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub project_id: String,
    pub job_id: String,
    pub kind: String,
    pub state: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub revision: u32,
    pub retirement: Option<RetirementReceipt>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchRetire {
    pub project_id: String,
    pub branch_id: String,
    pub expected_revision: u32,
    pub expected_head: String,
    pub cleanup_job_id: String,
    pub worktree_id: String,
    pub idempotency_key: String,
    pub pull_request_number: u64,
    #[serde(default)]
    pub integration_branch: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementReceipt {
    pub branch_id: String,
    pub repository_id: String,
    pub branch_name: String,
    pub head_sha: String,
    pub local_cleanup_receipt_id: String,
    pub local_branch_receipt_ids: Vec<String>,
    pub remote_receipt_id: String,
    pub observed_at: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectList {
    pub projects: Vec<ProjectSummary>,
    pub next_project_id: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSummary {
    pub project_id: String,
    pub name: String,
    pub slug: String,
    pub revision: u32,
    pub repository_count: u32,
    pub checkout_count: u32,
    pub branch_count: u32,
    pub local_bytes: Option<u64>,
    pub unmeasured_count: u32,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub project: ProjectSummary,
    pub roots: Vec<Root>,
    pub repositories: Vec<Repository>,
    pub branches: Vec<Branch>,
    pub checkouts: Vec<Checkout>,
    pub next_branch_cursor: Option<String>,
    pub next_checkout_cursor: Option<String>,
    pub storage: Storage,
    pub activity: Vec<Event>,
    pub permissions: Permissions,
    pub revision: u32,
    pub refreshing: bool,
    pub refreshed_at: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Root {
    pub kind: String,
    pub label: String,
    pub relative_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_path: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub repository_id: String,
    pub name: String,
    pub github_url: Option<String>,
    pub default_branch: Option<String>,
    pub primary_checkout_id: Option<String>,
    pub refresh_state: String,
    pub last_success: Option<String>,
    #[serde(default)]
    pub github_observation: Option<GithubObservation>,
    pub error: Option<String>,
    pub permissions: Permissions,
    #[serde(default)]
    pub details_omitted: bool,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubObservation {
    pub job_id: String,
    pub state: String,
    pub last_success: Option<String>,
}
#[derive(Clone, Debug, Default, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    pub edit: bool,
    pub cleanup: bool,
    pub admin: bool,
}
#[derive(Clone, Debug, Default, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub allocated_bytes: Option<u64>,
    pub source_bytes: Option<u64>,
    pub build_bytes: Option<u64>,
    pub shared_bytes: Option<u64>,
    pub unmeasured_count: u32,
    pub measured_at: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    pub worktree_id: String,
    pub repository_id: String,
    pub relative_path: String,
    pub root_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_path: Option<String>,
    pub role: String,
    pub branch_id: Option<String>,
    pub branch_name: Option<String>,
    pub head: Option<String>,
    pub dirty: Option<bool>,
    pub exists: Option<bool>,
    pub lifecycle: String,
    pub owner: Option<String>,
    pub intent: Option<String>,
    pub outcome: Option<String>,
    pub next_action: Option<String>,
    pub task_id: Option<String>,
    pub revision: u32,
    pub source_bytes: Option<u64>,
    pub build_bytes: Option<u64>,
    pub local_bytes: Option<u64>,
    pub artifact_ids: Vec<String>,
    pub protection_reasons: Vec<String>,
    pub can_remove: bool,
    pub locations: Vec<Location>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default = "unregistered")]
    pub registration_state: String,
    #[serde(default)]
    pub immutable_generation: bool,
    #[serde(default)]
    pub deployment_references: Vec<DeploymentReference>,
    #[serde(default)]
    pub details_omitted: bool,
}
fn unregistered() -> String {
    "unregistered".into()
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentReference {
    pub deployment_id: String,
    pub name: String,
    pub generation: Option<u32>,
    pub kind: String,
    pub state: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub kind: String,
    pub relative_path: String,
    pub root_kind: String,
    pub allocated_bytes: Option<u64>,
    pub artifact_id: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub branch_id: String,
    pub repository_id: String,
    pub branch_name: String,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub remote_present: Option<bool>,
    pub local_present: bool,
    pub lifecycle: String,
    pub intent: Option<String>,
    pub outcome: Option<String>,
    pub owner: Option<String>,
    pub next_action: Option<String>,
    pub task_id: Option<String>,
    pub revision: u32,
    pub worktree_ids: Vec<String>,
    pub local_bytes: Option<u64>,
    pub source_bytes: Option<u64>,
    pub build_bytes: Option<u64>,
    pub pr: Option<PullRequest>,
    pub merge_proof: Option<MergeProof>,
    #[serde(default)]
    pub cleanup_receipt: Option<CleanupReceiptCandidate>,
    #[serde(default)]
    pub details_omitted: bool,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupReceiptCandidate {
    pub job_id: String,
    pub worktree_id: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub state: String,
    pub merged: bool,
    pub head_sha: String,
    pub merge_commit_sha: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeProof {
    pub kind: String,
    pub head_sha: String,
    pub merged_into: String,
    pub observed_at: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchPage {
    pub branches: Vec<Branch>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutPage {
    pub checkouts: Vec<Checkout>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub events: Vec<Event>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub event_id: String,
    pub repository_id: Option<String>,
    pub target_id: Option<String>,
    pub kind: String,
    pub summary: String,
    pub at: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitPage {
    pub repository_id: String,
    pub branch_id: String,
    pub commits: Vec<Commit>,
    pub next_cursor: Option<String>,
    pub refresh_required: bool,
    pub refreshed_at: Option<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub authored_at: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRefresh {
    pub project_id: String,
    pub branch_id: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub repository_ids: Vec<String>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Memberships {
    pub memberships: Vec<Membership>,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Membership {
    pub project_id: String,
    pub project_name: String,
    pub project_slug: String,
    pub repository_id: String,
    pub source_repository_id: String,
}
