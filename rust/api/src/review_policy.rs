//! Provider-neutral review policy and the versioned reminder delivery contract.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub repository_id: String,
    pub workstream_id: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub repository_id: String,
    pub workstream_id: Option<String>,
    pub review_interval_ms: Option<u64>,
    pub escalation_interval_ms: Option<u64>,
    pub active: bool,
    /// Earlier start retained during migration; never a completion receipt.
    pub window_start_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlarmCapability {
    pub alarm_namespace: String,
    pub capability_revision: u8,
    /// Absolute UTC lease expiry in Unix milliseconds.
    pub lease_expires_at: u64,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Register {
    pub repository_id: String,
    pub workstream_id: Option<String>,
    pub owner_thread_id: String,
    pub mode: DeliveryMode,
    pub alarm_namespace: String,
    pub capability_revision: u8,
    /// Absolute UTC lease expiry in Unix milliseconds.
    pub lease_expires_at: u64,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    CodexAlarm,
}

#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct Policy {
    pub repository_id: String,
    pub workstream_id: Option<String>,
    pub review_interval_ms: u64,
    pub escalation_interval_ms: u64,
    pub active: bool,
    pub window_start_ms: u64,
    pub window_end_ms: u64,
    pub last_completed_receipt: Option<String>,
    pub due: bool,
    pub escalated: bool,
    pub delivery_route: String,
}

#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reminder {
    pub version: u8,
    pub reminder_id: String,
    pub repository_id: String,
    pub workstream_id: Option<String>,
    pub owner_thread_id: String,
    pub alarm_namespace: String,
    pub window_start_ms: u64,
    pub window_end_ms: u64,
    pub last_completed_receipt: Option<String>,
    pub escalation: bool,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingRequest {
    pub alarm_namespace: String,
    #[serde(default)]
    pub after_id: u64,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
pub struct Pending {
    pub cursor: u64,
    pub reminders: Vec<Reminder>,
    pub next_after_id: Option<u64>,
}
