//! Typed local-control protocol shared by the daemon, CLI, MCP adapter, and edge.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

pub mod params;
pub mod results;

pub const PROTOCOL_VERSION: u8 = 2;
pub const MAX_REQUEST_BYTES: usize = 65_536;
pub const MAX_RESPONSE_BYTES: usize = 262_144;
pub const MAX_ERROR_DETAIL_BYTES: usize = 4_096;
pub const READ_TIMEOUT_SECONDS: u64 = 5;
pub const WRITE_TIMEOUT_SECONDS: u64 = 10;

#[derive(Clone, Copy, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Codex,
    Claude,
    Cursor,
    Antigravity,
    Human,
    Edge,
    #[default]
    Other,
}

#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientContext {
    #[serde(default)]
    pub kind: ClientKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    pub protocol: u8,
    pub id: String,
    pub operation: String,
    #[serde(default = "empty_object")]
    pub params: Value,
    #[serde(default)]
    pub client: ClientContext,
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ProtocolInvalid,
    ProtocolUnsupported,
    RequestTooLarge,
    OperationUnknown,
    ParamsInvalid,
    RepositoryNotFound,
    RepositoryArchived,
    RepositoryArchiveBlocked,
    RepositoryConfigInvalid,
    WorktreeBusy,
    TestNotFound,
    TestLogUnavailable,
    LogNotFound,
    LogExpired,
    CursorStale,
    StructuredEvidenceInvalid,
    TestEvidenceExpired,
    TestEvidenceNotFound,
    TestEvidenceTampered,
    TestArtifactExpired,
    TestArtifactNotFound,
    TestArtifactTampered,
    TestStartFailed,
    TestsDraining,
    UnitStopFailed,
    DeploymentNotFound,
    Busy,
    DeploymentApplyFailed,
    DeploymentActionFailed,
    ObservedOnly,
    RollbackUnavailable,
    PermissionDenied,
    UserNotFound,
    TaskNotFound,
    ReleaseNotFound,
    DecisionNotFound,
    InternalError,
    DaemonUnavailable,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = serde_json::to_value(self).map_err(|_| fmt::Error)?;
        f.write_str(value.as_str().ok_or(fmt::Error)?)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseEnvelope {
    Success {
        protocol: u8,
        id: String,
        ok: True,
        data: Value,
    },
    Failure {
        protocol: u8,
        id: String,
        ok: False,
        error: ErrorBody,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct True;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct False;

impl Serialize for True {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected true"))
        }
    }
}

impl Serialize for False {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for False {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Err(serde::de::Error::custom("expected false"))
        } else {
            Ok(Self)
        }
    }
}

impl JsonSchema for True {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "true".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        serde_json::json!({"const": true})
            .try_into()
            .expect("valid schema")
    }
}

impl JsonSchema for False {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "false".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        serde_json::json!({"const": false})
            .try_into()
            .expect("valid schema")
    }
}

impl ResponseEnvelope {
    pub fn success(id: impl Into<String>, data: impl Serialize) -> Result<Self, ProtocolError> {
        Ok(Self::Success {
            protocol: PROTOCOL_VERSION,
            id: id.into(),
            ok: True,
            data: serde_json::to_value(data).map_err(ProtocolError::serialization)?,
        })
    }

    pub fn failure(id: impl Into<String>, error: ProtocolError) -> Self {
        Self::Failure {
            protocol: PROTOCOL_VERSION,
            id: id.into(),
            ok: False,
            error: error.into_body(),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Success { .. })
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: String,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: String::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = truncate_utf8(&detail.into(), MAX_ERROR_DETAIL_BYTES);
        self
    }

    fn serialization(error: serde_json::Error) -> Self {
        Self::new(ErrorCode::InternalError, "could not serialize response")
            .with_detail(error.to_string())
    }

    fn into_body(self) -> ErrorBody {
        ErrorBody {
            code: self.code,
            message: self.message,
            detail: truncate_utf8(&self.detail, MAX_ERROR_DETAIL_BYTES),
        }
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Public,
    #[serde(rename = "self")]
    Self_,
    Server,
    Repository,
    Deployment,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Anonymous,
    #[serde(rename = "self")]
    Self_,
    Viewer,
    Operator,
    Administrator,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Read,
    Append,
    Reversible,
    Destructive,
    External,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct OperationPolicy {
    pub scope: Scope,
    pub role: Role,
    pub effect: Effect,
    pub idempotent: bool,
}

impl OperationPolicy {
    pub const fn new(scope: Scope, role: Role, effect: Effect, idempotent: bool) -> Self {
        Self {
            scope,
            role,
            effect,
            idempotent,
        }
    }

    pub const fn read_only(self) -> bool {
        matches!(self.effect, Effect::Read)
    }

    pub const fn destructive(self) -> bool {
        matches!(self.effect, Effect::Destructive)
    }

    pub const fn open_world(self) -> bool {
        matches!(self.effect, Effect::External)
    }
}

#[derive(Clone, Debug, Default, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyParams {}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PingData {
    pub daemon_version: String,
    pub protocol_version: u8,
    pub schema_version: u32,
    pub executor_schema: u8,
    pub source_commit: String,
    pub socket: String,
}

#[derive(Clone, Copy, Debug)]
pub struct OperationDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub policy: OperationPolicy,
    pub mcp_names: &'static [&'static str],
    pub input_schema: fn() -> Value,
    pub output_schema: fn() -> Value,
    pub validate_params: fn(&Value) -> Result<(), ProtocolError>,
}

fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema is serializable")
}

fn validate_params<T>(value: &Value) -> Result<(), ProtocolError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value::<T>(value.clone())
        .map(|_| ())
        .map_err(|error| {
            ProtocolError::new(ErrorCode::ParamsInvalid, "operation parameters are invalid")
                .with_detail(error.to_string())
        })
}

const READ_PUBLIC: OperationPolicy =
    OperationPolicy::new(Scope::Public, Role::Anonymous, Effect::Read, true);
const READ_SERVER_ADMIN: OperationPolicy =
    OperationPolicy::new(Scope::Server, Role::Administrator, Effect::Read, true);
const APPEND_SERVER_ADMIN: OperationPolicy =
    OperationPolicy::new(Scope::Server, Role::Administrator, Effect::Append, false);
const IDEMPOTENT_APPEND_SERVER_ADMIN: OperationPolicy =
    OperationPolicy::new(Scope::Server, Role::Administrator, Effect::Append, true);
const REVERSIBLE_SERVER_ADMIN: OperationPolicy =
    OperationPolicy::new(Scope::Server, Role::Administrator, Effect::Reversible, true);
const DESTRUCTIVE_SERVER_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Server,
    Role::Administrator,
    Effect::Destructive,
    true,
);
const READ_REPOSITORY_ADMIN: OperationPolicy =
    OperationPolicy::new(Scope::Repository, Role::Administrator, Effect::Read, true);
const APPEND_REPOSITORY_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Repository,
    Role::Administrator,
    Effect::Append,
    false,
);
const REVERSIBLE_REPOSITORY_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Repository,
    Role::Administrator,
    Effect::Reversible,
    false,
);
const IDEMPOTENT_REVERSIBLE_REPOSITORY_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Repository,
    Role::Administrator,
    Effect::Reversible,
    true,
);
const DESTRUCTIVE_REPOSITORY_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Repository,
    Role::Administrator,
    Effect::Destructive,
    false,
);
const IDEMPOTENT_DESTRUCTIVE_REPOSITORY_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Repository,
    Role::Administrator,
    Effect::Destructive,
    true,
);
const READ_REPOSITORY_VIEWER: OperationPolicy =
    OperationPolicy::new(Scope::Repository, Role::Viewer, Effect::Read, true);
const READ_REPOSITORY_OPERATOR: OperationPolicy =
    OperationPolicy::new(Scope::Repository, Role::Operator, Effect::Read, true);
const READ_DEPLOYMENT_VIEWER: OperationPolicy =
    OperationPolicy::new(Scope::Deployment, Role::Viewer, Effect::Read, true);
const REVERSIBLE_DEPLOYMENT_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Deployment,
    Role::Administrator,
    Effect::Reversible,
    false,
);
const IDEMPOTENT_REVERSIBLE_DEPLOYMENT_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Deployment,
    Role::Administrator,
    Effect::Reversible,
    true,
);
const REVERSIBLE_DEPLOYMENT_OPERATOR: OperationPolicy =
    OperationPolicy::new(Scope::Deployment, Role::Operator, Effect::Reversible, false);
const IDEMPOTENT_REVERSIBLE_DEPLOYMENT_OPERATOR: OperationPolicy =
    OperationPolicy::new(Scope::Deployment, Role::Operator, Effect::Reversible, true);
const DESTRUCTIVE_DEPLOYMENT_ADMIN: OperationPolicy = OperationPolicy::new(
    Scope::Deployment,
    Role::Administrator,
    Effect::Destructive,
    true,
);
const READ_SELF: OperationPolicy =
    OperationPolicy::new(Scope::Self_, Role::Self_, Effect::Read, true);
const APPEND_SELF: OperationPolicy =
    OperationPolicy::new(Scope::Self_, Role::Self_, Effect::Append, false);
const DESTRUCTIVE_SELF: OperationPolicy =
    OperationPolicy::new(Scope::Self_, Role::Self_, Effect::Destructive, true);
const EXTERNAL_SELF: OperationPolicy =
    OperationPolicy::new(Scope::Self_, Role::Self_, Effect::External, false);
const IDEMPOTENT_EXTERNAL_SELF: OperationPolicy =
    OperationPolicy::new(Scope::Self_, Role::Self_, Effect::External, true);

macro_rules! operation {
    ($name:literal, $description:literal, $policy:expr, [$($mcp:literal),* $(,)?], $input:ty, $output:ty) => {
        OperationDefinition {
            name: $name,
            description: $description,
            policy: $policy,
            mcp_names: &[$($mcp),*],
            input_schema: schema_for::<$input>,
            output_schema: schema_for::<$output>,
            validate_params: validate_params::<$input>,
        }
    };
}

pub static OPERATIONS: &[OperationDefinition] = &[
    operation!(
        "ping",
        "Report daemon, schema, protocol, source, and socket identity.",
        READ_PUBLIC,
        [],
        EmptyParams,
        PingData
    ),
    operation!(
        "user.whoami",
        "Show the current public identity and grants.",
        READ_PUBLIC,
        [],
        params::Empty,
        results::WhoAmI
    ),
    operation!(
        "user.accept_invitation",
        "Accept an invitation for the edge-authenticated identity.",
        APPEND_SELF,
        [],
        params::AcceptInvitation,
        results::AcceptedInvitation
    ),
    operation!(
        "user.list",
        "List public Console users and invitations.",
        READ_SERVER_ADMIN,
        [],
        params::Empty,
        results::UserList
    ),
    operation!(
        "user.invite",
        "Invite a public Console user.",
        APPEND_SERVER_ADMIN,
        [],
        params::InviteUser,
        results::InvitedUser
    ),
    operation!(
        "user.remove",
        "Remove one public Console user.",
        DESTRUCTIVE_SERVER_ADMIN,
        [],
        params::EmailOnly,
        results::RemovedUser
    ),
    operation!(
        "grant.set",
        "Set one deployment access role.",
        IDEMPOTENT_REVERSIBLE_DEPLOYMENT_ADMIN,
        [],
        params::SetGrant,
        results::GrantSet
    ),
    operation!(
        "grant.remove",
        "Remove one deployment access grant.",
        DESTRUCTIVE_DEPLOYMENT_ADMIN,
        [],
        params::RemoveGrant,
        results::GrantRemoved
    ),
    operation!(
        "repository.register",
        "Register the repository containing a path.",
        IDEMPOTENT_APPEND_SERVER_ADMIN,
        [],
        params::PathOnly,
        results::RegisteredRepository
    ),
    operation!(
        "repository.list",
        "List registered repositories.",
        READ_SERVER_ADMIN,
        ["repository_list"],
        params::RepositoryList,
        results::RepositoryList
    ),
    operation!(
        "repository.status",
        "Show one registered repository.",
        READ_REPOSITORY_ADMIN,
        [],
        params::PathOnly,
        results::RepositoryStatus
    ),
    operation!(
        "repository.archive",
        "Archive a repository after its blockers are cleared.",
        REVERSIBLE_SERVER_ADMIN,
        ["repository_archive"],
        params::ArchiveRepository,
        results::Repository
    ),
    operation!(
        "repository.unarchive",
        "Restore one archived repository.",
        REVERSIBLE_SERVER_ADMIN,
        ["repository_unarchive"],
        params::UnarchiveRepository,
        results::Repository
    ),
    operation!(
        "test.start",
        "Start or supersede one governed validation run.",
        DESTRUCTIVE_REPOSITORY_ADMIN,
        ["test_start"],
        params::StartTest,
        results::TestStarted
    ),
    operation!(
        "test.retry",
        "Retry one failed check from a completed run.",
        DESTRUCTIVE_REPOSITORY_ADMIN,
        ["test_retry"],
        params::RetryTest,
        results::TestStarted
    ),
    operation!(
        "test.status",
        "Show the current governed validation result.",
        READ_REPOSITORY_ADMIN,
        ["test_status"],
        params::PathOnly,
        results::TestSummary
    ),
    operation!(
        "test.log.catalog",
        "List retained log metadata without content.",
        READ_REPOSITORY_ADMIN,
        ["test_log_catalog"],
        params::LogCatalog,
        results::LogCatalog
    ),
    operation!(
        "test.log.tail",
        "Read a bounded final-line log slice.",
        READ_REPOSITORY_ADMIN,
        ["test_log_tail"],
        params::LogTail,
        results::LogContent
    ),
    operation!(
        "test.log.search",
        "Search one retained stream literally.",
        READ_REPOSITORY_ADMIN,
        ["test_log_search"],
        params::LogSearch,
        results::LogSearch
    ),
    operation!(
        "test.log.range",
        "Read one exact bounded log range.",
        READ_REPOSITORY_ADMIN,
        ["test_log_range"],
        params::LogRange,
        results::LogContent
    ),
    operation!(
        "test.log.failure_context",
        "Read deterministic bounded failure context.",
        READ_REPOSITORY_ADMIN,
        ["test_log_failure_context"],
        params::LogFailureContext,
        results::LogFailureContext
    ),
    operation!(
        "test.log.retention.get",
        "Show governed-log retention settings.",
        READ_SERVER_ADMIN,
        ["test_log_retention_show"],
        params::Empty,
        results::Retention
    ),
    operation!(
        "test.log.retention.set",
        "Change governed-log retention settings.",
        DESTRUCTIVE_SERVER_ADMIN,
        ["test_log_retention_set"],
        params::SetRetention,
        results::Retention
    ),
    operation!(
        "test.evidence.get",
        "List retained journey evidence.",
        READ_REPOSITORY_ADMIN,
        ["test_evidence_get"],
        params::EvidenceReference,
        results::EvidenceGet
    ),
    operation!(
        "test.evidence.image",
        "Read one verified screenshot chunk.",
        READ_REPOSITORY_ADMIN,
        ["test_evidence_image"],
        params::EvidenceImage,
        results::ImageChunk
    ),
    operation!(
        "test.artifact.catalog",
        "List retained artifact-tree metadata.",
        READ_REPOSITORY_ADMIN,
        ["test_artifact_catalog"],
        params::ArtifactCatalog,
        results::ArtifactCatalog
    ),
    operation!(
        "test.artifact.file",
        "Read one verified retained artifact chunk.",
        READ_REPOSITORY_ADMIN,
        ["test_artifact_file"],
        params::ArtifactFile,
        results::ArtifactChunk
    ),
    operation!(
        "test.evidence.feedback.create",
        "Create screenshot-anchored project feedback.",
        APPEND_REPOSITORY_ADMIN,
        ["test_evidence_feedback_create"],
        params::CreateFeedback,
        results::FeedbackCreated
    ),
    operation!(
        "test.evidence.feedback.reply",
        "Reply to screenshot-anchored feedback.",
        APPEND_REPOSITORY_ADMIN,
        ["test_evidence_feedback_reply"],
        params::FeedbackReply,
        results::FeedbackMutation
    ),
    operation!(
        "test.evidence.feedback.edit",
        "Edit the caller's feedback comment.",
        REVERSIBLE_REPOSITORY_ADMIN,
        ["test_evidence_feedback_edit"],
        params::FeedbackEdit,
        results::FeedbackMutation
    ),
    operation!(
        "test.evidence.feedback.state",
        "Resolve or reopen screenshot feedback.",
        IDEMPOTENT_REVERSIBLE_REPOSITORY_ADMIN,
        ["test_evidence_feedback_state"],
        params::FeedbackStateChange,
        results::FeedbackMutation
    ),
    operation!(
        "test.evidence.feedback.delete",
        "Delete the caller's screenshot annotation.",
        IDEMPOTENT_DESTRUCTIVE_REPOSITORY_ADMIN,
        ["test_evidence_feedback_delete"],
        params::FeedbackDelete,
        results::FeedbackMutation
    ),
    operation!(
        "test.stop",
        "Cancel the current governed run.",
        IDEMPOTENT_DESTRUCTIVE_REPOSITORY_ADMIN,
        ["test_stop"],
        params::StopTest,
        results::StopTest
    ),
    operation!(
        "test.list",
        "List current governed runs.",
        READ_SERVER_ADMIN,
        ["test_list"],
        params::Empty,
        results::TestList
    ),
    operation!(
        "test.capacity.get",
        "Show host-wide validation capacity.",
        READ_SERVER_ADMIN,
        ["test_capacity_show"],
        params::Empty,
        results::Capacity
    ),
    operation!(
        "test.capacity.set",
        "Set or clear the validation capacity cap.",
        REVERSIBLE_SERVER_ADMIN,
        ["test_capacity_set", "test_capacity_clear"],
        params::SetCapacity,
        results::Capacity
    ),
    operation!(
        "deployment.list",
        "List visible deployments.",
        READ_DEPLOYMENT_VIEWER,
        ["deployment_list"],
        params::DeploymentList,
        results::DeploymentList
    ),
    operation!(
        "deployment.status",
        "Show one deployment.",
        READ_DEPLOYMENT_VIEWER,
        ["deployment_status"],
        params::DeploymentReference,
        results::DeploymentStatus
    ),
    operation!(
        "deployment.logs",
        "Read bounded deployment logs.",
        READ_DEPLOYMENT_VIEWER,
        ["deployment_logs"],
        params::DeploymentLogs,
        results::DeploymentLog
    ),
    operation!(
        "deployment.apply",
        "Apply a declared deployment.",
        REVERSIBLE_DEPLOYMENT_ADMIN,
        ["deployment_apply"],
        params::DeploymentReference,
        results::DeploymentStatus
    ),
    operation!(
        "deployment.rollback",
        "Roll back to the prior generation.",
        REVERSIBLE_DEPLOYMENT_ADMIN,
        ["deployment_rollback"],
        params::DeploymentReference,
        results::DeploymentStatus
    ),
    operation!(
        "deployment.start",
        "Start a deployment or selected service.",
        IDEMPOTENT_REVERSIBLE_DEPLOYMENT_OPERATOR,
        ["deployment_start"],
        params::DeploymentControl,
        results::DeploymentStatus
    ),
    operation!(
        "deployment.stop",
        "Stop a deployment or selected service.",
        IDEMPOTENT_REVERSIBLE_DEPLOYMENT_OPERATOR,
        ["deployment_stop"],
        params::DeploymentControl,
        results::DeploymentStatus
    ),
    operation!(
        "deployment.restart",
        "Restart a deployment or selected service.",
        REVERSIBLE_DEPLOYMENT_OPERATOR,
        ["deployment_restart"],
        params::DeploymentControl,
        results::DeploymentStatus
    ),
    operation!(
        "deployment.set_domain",
        "Set or clear a deployment route.",
        IDEMPOTENT_REVERSIBLE_DEPLOYMENT_ADMIN,
        ["deployment_set_domain"],
        params::SetDomain,
        results::DomainChanged
    ),
    operation!(
        "deployment.remove",
        "Remove a deployment with an explicit data outcome.",
        DESTRUCTIVE_DEPLOYMENT_ADMIN,
        [],
        params::RemoveDeployment,
        results::DeploymentRemoved
    ),
    operation!(
        "health.summary",
        "Show host health and active alerts.",
        READ_SERVER_ADMIN,
        ["health_summary"],
        params::Empty,
        results::HealthSummary
    ),
    operation!(
        "health.repositories",
        "Show visible per-repository resource use.",
        READ_REPOSITORY_VIEWER,
        ["health_repositories"],
        params::Empty,
        results::HealthRepositories
    ),
    operation!(
        "health.repository",
        "Show one repository's resource use.",
        READ_REPOSITORY_VIEWER,
        ["health_repository"],
        params::PathOnly,
        results::HealthRepository
    ),
    operation!(
        "health.history",
        "Show bounded resource history.",
        READ_REPOSITORY_VIEWER,
        [],
        params::HealthHistory,
        results::HealthHistory
    ),
    operation!(
        "health.containers",
        "List every container with truthful attribution.",
        READ_SERVER_ADMIN,
        ["health_containers"],
        params::Empty,
        results::ContainerList
    ),
    operation!(
        "health.container_remove",
        "Remove one exact unmanaged container.",
        DESTRUCTIVE_SERVER_ADMIN,
        [],
        params::RemoveContainer,
        results::RemovedContainer
    ),
    operation!(
        "plan.overview",
        "Show releases and the active completion plan.",
        READ_REPOSITORY_VIEWER,
        ["plan_overview"],
        params::PlanReference,
        results::PlanOverview
    ),
    operation!(
        "task.history",
        "Show one task and its permanent history.",
        READ_REPOSITORY_VIEWER,
        ["task_history"],
        params::TaskHistory,
        results::TaskHistory
    ),
    operation!(
        "task.create",
        "Create one completion-ledger task.",
        APPEND_REPOSITORY_ADMIN,
        ["task_create"],
        params::TaskCreate,
        results::TaskCreated
    ),
    operation!(
        "task.update",
        "Append a task state or wording change.",
        DESTRUCTIVE_REPOSITORY_ADMIN,
        ["task_update"],
        params::TaskUpdate,
        results::TaskMutation
    ),
    operation!(
        "release.create",
        "Create a planned release.",
        APPEND_REPOSITORY_ADMIN,
        ["release_create"],
        params::ReleaseCreate,
        results::Release
    ),
    operation!(
        "release.update",
        "Change a planned release.",
        DESTRUCTIVE_REPOSITORY_ADMIN,
        [],
        params::ReleaseUpdate,
        results::Release
    ),
    operation!(
        "release.request",
        "Request a preview deployment.",
        APPEND_REPOSITORY_ADMIN,
        [],
        params::ReleaseRequest,
        results::Release
    ),
    operation!(
        "release.deliver",
        "Attach real delivery evidence to a release.",
        APPEND_REPOSITORY_ADMIN,
        ["release_deliver"],
        params::ReleaseDeliver,
        results::ReleaseDelivered
    ),
    operation!(
        "decision.tail",
        "Read the rolling decision summary and latest decisions.",
        READ_REPOSITORY_VIEWER,
        ["decision_tail"],
        params::DecisionTail,
        results::DecisionTail
    ),
    operation!(
        "decision.search",
        "Search permanent repository decisions.",
        READ_REPOSITORY_VIEWER,
        ["decision_search"],
        params::DecisionSearch,
        results::DecisionSearch
    ),
    operation!(
        "decision.record",
        "Record a permanent repository decision.",
        APPEND_REPOSITORY_ADMIN,
        ["decision_record"],
        params::DecisionRecord,
        results::DecisionRecorded
    ),
    operation!(
        "decision.summarize",
        "Store a new rolling decision summary.",
        APPEND_REPOSITORY_ADMIN,
        ["decision_summarize"],
        params::DecisionSummarize,
        results::DecisionSummarized
    ),
    operation!(
        "usage.repositories",
        "Show privacy-preserving usage across visible repositories.",
        READ_REPOSITORY_OPERATOR,
        [],
        params::UsageRepositories,
        results::UsageRepositories
    ),
    operation!(
        "usage.repository",
        "Show privacy-preserving usage for one repository.",
        READ_REPOSITORY_OPERATOR,
        [],
        params::UsageRepository,
        results::UsageRepository
    ),
    operation!(
        "progress.repositories",
        "Show delivery progress across visible repositories.",
        READ_REPOSITORY_OPERATOR,
        [],
        params::Empty,
        results::ProgressRepositories
    ),
    operation!(
        "progress.repository",
        "Show delivery progress for one repository.",
        READ_REPOSITORY_OPERATOR,
        [],
        params::ProgressRepository,
        results::ProgressRepository
    ),
    operation!(
        "telegram.link",
        "Link one Telegram chat to an identity.",
        EXTERNAL_SELF,
        [],
        params::TelegramLink,
        results::TelegramLinked
    ),
    operation!(
        "telegram.subscribe",
        "Subscribe one linked chat to notices.",
        EXTERNAL_SELF,
        [],
        params::TelegramSubscription,
        results::TelegramSubscription
    ),
    operation!(
        "telegram.unsubscribe",
        "Remove one notice subscription.",
        IDEMPOTENT_EXTERNAL_SELF,
        [],
        params::TelegramSubscription,
        results::TelegramSubscription
    ),
    operation!(
        "telegram.list",
        "List the caller's linked chats and subscriptions.",
        READ_SELF,
        [],
        params::Empty,
        results::TelegramList
    ),
    operation!(
        "bug.report",
        "Report or count a Coordinator defect while the daemon may be unavailable.",
        APPEND_SELF,
        ["bug_report"],
        params::BugReport,
        results::BugRecord
    ),
    operation!(
        "bug.list",
        "List open Coordinator defects.",
        READ_SELF,
        ["bug_list"],
        params::Empty,
        results::BugList
    ),
    operation!(
        "bug.close",
        "Close one Coordinator defect.",
        DESTRUCTIVE_SELF,
        ["bug_close"],
        params::BugClose,
        results::BugClosed
    ),
];

pub fn operation(name: &str) -> Option<&'static OperationDefinition> {
    OPERATIONS.iter().find(|definition| definition.name == name)
}

pub fn parse_request(raw: &[u8]) -> Result<RequestEnvelope, ProtocolError> {
    if raw.len() > MAX_REQUEST_BYTES {
        return Err(ProtocolError::new(
            ErrorCode::RequestTooLarge,
            "request exceeds 64 KiB frame cap",
        ));
    }
    let value: Value = serde_json::from_slice(raw).map_err(|error| {
        ProtocolError::new(ErrorCode::ProtocolInvalid, "request is not valid JSON")
            .with_detail(error.to_string())
    })?;
    let protocol = value.get("protocol").and_then(Value::as_u64);
    if protocol != Some(u64::from(PROTOCOL_VERSION)) {
        return Err(ProtocolError::new(
            ErrorCode::ProtocolUnsupported,
            format!(
                "protocol {} is not supported; install a protocol-2 client",
                protocol.map_or_else(|| "missing".to_owned(), |v| v.to_string())
            ),
        ));
    }
    let request: RequestEnvelope = serde_json::from_value(value).map_err(|error| {
        ProtocolError::new(ErrorCode::ProtocolInvalid, "request envelope is invalid")
            .with_detail(error.to_string())
    })?;
    if request.id.is_empty() {
        return Err(ProtocolError::new(
            ErrorCode::ProtocolInvalid,
            "missing request id",
        ));
    }
    if request.operation.is_empty() {
        return Err(ProtocolError::new(
            ErrorCode::ProtocolInvalid,
            "missing operation",
        ));
    }
    if !request.params.is_object() {
        return Err(ProtocolError::new(
            ErrorCode::ProtocolInvalid,
            "params must be an object",
        ));
    }
    let Some(definition) = operation(&request.operation) else {
        return Err(ProtocolError::new(
            ErrorCode::OperationUnknown,
            format!("unknown operation {:?}", request.operation),
        ));
    };
    if let Some(identity) = &request.client.identity
        && (identity.len() > 254 || !identity.contains('@'))
    {
        return Err(ProtocolError::new(
            ErrorCode::ProtocolInvalid,
            "client.identity must be an e-mail",
        ));
    }
    (definition.validate_params)(&request.params)?;
    Ok(request)
}

pub fn encode_response(response: &ResponseEnvelope) -> Vec<u8> {
    let mut encoded = serde_json::to_vec(response).expect("response envelope is serializable");
    encoded.push(b'\n');
    if encoded.len() <= MAX_RESPONSE_BYTES {
        return encoded;
    }
    let fallback = ResponseEnvelope::failure(
        response_id(response),
        ProtocolError::new(ErrorCode::InternalError, "response exceeded size cap"),
    );
    let mut bounded = serde_json::to_vec(&fallback).expect("fallback is serializable");
    bounded.push(b'\n');
    bounded
}

fn response_id(response: &ResponseEnvelope) -> &str {
    match response {
        ResponseEnvelope::Success { id, .. } | ResponseEnvelope::Failure { id, .. } => id,
    }
}

pub fn contract_document() -> Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "product": "devcoordinator2",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL_VERSION,
        "transport": {
            "framing": "one newline-terminated UTF-8 JSON request and response per Unix connection",
            "maxRequestBytes": MAX_REQUEST_BYTES,
            "maxResponseBytes": MAX_RESPONSE_BYTES,
            "readTimeoutSeconds": READ_TIMEOUT_SECONDS,
            "writeTimeoutSeconds": WRITE_TIMEOUT_SECONDS
        },
        "requestEnvelope": schema_for::<RequestEnvelope>(),
        "operations": OPERATIONS.iter().map(|definition| serde_json::json!({
            "name": definition.name,
            "description": definition.description,
            "policy": definition.policy,
            "mcpNames": definition.mcp_names,
            "inputSchema": (definition.input_schema)(),
            "outputSchema": (definition.output_schema)()
        })).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_two_round_trip_is_strict() {
        let request = br#"{"protocol":2,"id":"abc","operation":"ping","params":{},"client":{"kind":"codex"}}"#;
        let parsed = parse_request(request).expect("valid request");
        assert_eq!(parsed.operation, "ping");
        assert_eq!(parsed.client.kind, ClientKind::Codex);

        let unknown =
            br#"{"protocol":2,"id":"abc","operation":"ping","params":{},"client":{},"extra":true}"#;
        assert_eq!(
            parse_request(unknown).unwrap_err().code,
            ErrorCode::ProtocolInvalid
        );
    }

    #[test]
    fn protocol_one_is_rejection_only() {
        let request = br#"{"protocol":1,"id":"old","command":"ping","args":{},"client":{}}"#;
        let error = parse_request(request).unwrap_err();
        assert_eq!(error.code, ErrorCode::ProtocolUnsupported);
    }

    #[test]
    fn operation_parameters_reject_unknown_fields_before_dispatch() {
        let request = br#"{"protocol":2,"id":"abc","operation":"user.whoami","params":{"extra":true},"client":{}}"#;
        assert_eq!(
            parse_request(request).unwrap_err().code,
            ErrorCode::ParamsInvalid
        );
    }

    #[test]
    fn contract_has_one_typed_ping_definition() {
        let document = contract_document();
        assert_eq!(document["protocol"], 2);
        assert_eq!(document["operations"][0]["name"], "ping");
        assert!(document["operations"][0]["inputSchema"].is_object());
        assert!(document["operations"][0]["outputSchema"].is_object());
    }

    #[test]
    fn registry_names_and_mcp_tools_are_unique() {
        let mut operations = std::collections::BTreeSet::new();
        let mut tools = std::collections::BTreeSet::new();
        for definition in OPERATIONS {
            assert!(operations.insert(definition.name), "duplicate operation");
            for tool in definition.mcp_names {
                assert!(tools.insert(tool), "duplicate MCP tool");
            }
        }
        assert_eq!(OPERATIONS.len(), 75);
        assert_eq!(tools.len(), 53);
    }

    #[test]
    fn every_operation_has_a_strict_concrete_contract() {
        let contract = contract_document();
        for operation in contract["operations"].as_array().expect("operations") {
            assert_eq!(
                operation["inputSchema"]["additionalProperties"], false,
                "{} input is not strict",
                operation["name"]
            );
            let output = operation["outputSchema"].to_string();
            assert!(!output.contains("ObjectData"));
            assert!(!output.contains("PendingDomainResult"));
        }
    }
}
