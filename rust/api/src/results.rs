//! Typed operation results. The first migration checkpoint contains the shared
//! identity, repository, planning, and scalar evidence vocabulary; remaining
//! domain projections are filled in as their services are ported.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::params::{AccessRole, DecisionAspect, ReleaseKind, ReleaseStatus, TaskKind, TaskStatus};

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedInvitation {
    pub email: String,
    pub user_id: String,
    pub accepted: bool,
    pub administrator: bool,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhoAmI {
    pub local: bool,
    pub identity: Option<String>,
    pub user_id: Option<String>,
    pub administrator: bool,
    pub grants: BTreeMap<String, AccessRole>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Worktree {
    pub worktree_id: String,
    pub worktree_path: String,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub repository_id: String,
    pub root_path: String,
    pub display_name: String,
    pub registered_at: String,
    pub registered_by_uid: u32,
    pub last_seen_at: String,
    pub archived_at: Option<String>,
    pub archived_by_uid: Option<u32>,
    pub archive_note: Option<String>,
    pub merged_into_repository_id: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryListRow {
    pub repository_id: String,
    pub root_path: String,
    pub display_name: String,
    pub registered_at: String,
    pub last_seen_at: String,
    pub archived_at: Option<String>,
    pub archived_by_uid: Option<u32>,
    pub archive_note: Option<String>,
    pub merged_into_repository_id: Option<String>,
    pub worktrees: Vec<Worktree>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryList {
    pub repositories: Vec<RepositoryListRow>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredRepository {
    pub repository_id: String,
    pub worktree_id: String,
    pub root_path: String,
    pub worktree_path: String,
    pub display_name: String,
    pub registered: bool,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElaborationRequest {
    pub task_id: String,
    pub title: String,
    pub outcome: String,
    pub status: TaskStatus,
    pub kind: TaskKind,
    pub requested_at: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub task_id: String,
    pub repository_id: String,
    pub parent_task_id: Option<String>,
    pub release_id: Option<String>,
    pub seq: u32,
    pub position: u32,
    pub title: String,
    pub outcome: String,
    pub impact: Option<String>,
    pub unblock_condition: Option<String>,
    pub verification: Option<String>,
    pub technical_note: Option<String>,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub estimated_loc: Option<u32>,
    pub elaboration_needed: bool,
    pub created_at: String,
    pub created_by: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskMutation {
    pub task_id: String,
    pub repository_id: String,
    pub seq: u32,
    pub position: u32,
    pub title: String,
    pub status: TaskStatus,
    pub kind: TaskKind,
    pub release_id: Option<String>,
    pub parent_task_id: Option<String>,
    pub estimated_loc: Option<u32>,
    pub elaboration_needed: bool,
    pub preview_requested: bool,
    pub elaboration_requests: Vec<ElaborationRequest>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanEvent {
    pub event: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub actor: String,
    pub at: String,
    pub note: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskHistory {
    pub task: Task,
    pub events: Vec<PlanEvent>,
    pub events_truncated: bool,
    pub elaboration_requests: Vec<ElaborationRequest>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub release_id: String,
    pub repository_id: String,
    pub seq: u32,
    pub name: String,
    pub kind: ReleaseKind,
    pub status: ReleaseStatus,
    pub note: Option<String>,
    pub requested_at: Option<String>,
    pub delivered_at: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub decision_id: String,
    pub seq: u32,
    pub r#ref: Option<String>,
    pub aspect: DecisionAspect,
    pub title: String,
    pub body: String,
    pub technical_note: Option<String>,
    pub superseded_by: Option<String>,
    pub created_at: String,
    pub created_by: String,
}

/// Explicit migration marker. Registry completeness tests reject this from a
/// release-ready contract, but it keeps unfinished outputs visible and ledgered.
#[derive(Clone, Debug, Default, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PendingDomainResult(pub BTreeMap<String, serde_json::Value>);
