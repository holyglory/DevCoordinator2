//! Explicit associations to already owned build allocations. Paths never infer ownership.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
fn default_limit() -> u16 {
    50
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct List {
    pub project_id: String,
    pub worktree_id: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u16,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adopt {
    pub project_id: String,
    pub worktree_id: String,
    pub artifact_id: String,
    pub expected_checkout_revision: u32,
    pub expected_artifact_revision: u64,
    pub expected_revision: u32,
    pub ownership: String,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detach {
    pub project_id: String,
    pub worktree_id: String,
    pub association_id: String,
    pub expected_revision: u32,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Association {
    pub association_id: String,
    pub project_id: String,
    pub worktree_id: String,
    pub artifact_id: String,
    pub revision: u32,
    pub artifact_revision: u64,
    pub ownership: String,
    pub active: bool,
    #[serde(default)]
    pub identity_valid: bool,
    pub other_checkout_ids: Vec<String>,
    pub other_checkout_count: u32,
    pub has_other_owners: bool,
    pub location: super::Location,
    pub provenance: String,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub associations: Vec<Association>,
    pub next_cursor: Option<String>,
}
