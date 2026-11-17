//! Explicit trusted-local relocation. This is not a public Console mover.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Prepare {
    pub project_id: String,
    /// The registered physical repository identity.
    pub repository_id: String,
    pub worktree_id: String,
    pub old_path: String,
    pub new_path: String,
    pub expected_checkout_revision: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub relocation_id: String,
    pub expected_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub relocation_id: String,
    pub project_id: String,
    pub repository_id: String,
    pub worktree_id: String,
    pub old_path: String,
    pub new_path: String,
    pub fingerprint: String,
    pub state: String,
    pub checkout_revision: u64,
    pub prepared_at: String,
    pub verified_at: Option<String>,
    pub committed_at: Option<String>,
}
