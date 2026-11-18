//! Explicit trusted-local, inactive, same-filesystem checkout relocation.
//! Cross-filesystem copies and deployment-bound moves are unavailable.
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
    /// New plans use `rename`; historical copy receipts may retain `verified_copy`.
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub pending_deployments: Vec<String>,
    pub state: String,
    pub checkout_revision: u64,
    pub prepared_at: String,
    pub verified_at: Option<String>,
    pub committed_at: Option<String>,
}
