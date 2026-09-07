use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub expected_revision: String,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub configured: bool,
    pub active_revision: String,
    pub stored_revision: Option<String>,
    pub pending_reload: bool,
    pub authorization_count: usize,
    pub reloadable_settings: Vec<String>,
    pub restart_required_settings: Vec<String>,
    pub validation_error: Option<String>,
    pub history: Vec<Change>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub sequence: u64,
    pub action: String,
    pub outcome: String,
    pub previous_revision: String,
    pub revision: String,
    pub at: String,
}
