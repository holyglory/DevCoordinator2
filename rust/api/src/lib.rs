//! Typed local-control protocol shared by the daemon, CLI, MCP adapter, and edge.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

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
    pub mcp_name: Option<&'static str>,
    pub input_schema: fn() -> Value,
    pub output_schema: fn() -> Value,
}

fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema is serializable")
}

const READ_PUBLIC: OperationPolicy =
    OperationPolicy::new(Scope::Public, Role::Anonymous, Effect::Read, true);

pub static OPERATIONS: &[OperationDefinition] = &[OperationDefinition {
    name: "ping",
    description: "Report daemon, schema, protocol, source, and socket identity.",
    policy: READ_PUBLIC,
    mcp_name: None,
    input_schema: schema_for::<EmptyParams>,
    output_schema: schema_for::<PingData>,
}];

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
    if operation(&request.operation).is_none() {
        return Err(ProtocolError::new(
            ErrorCode::OperationUnknown,
            format!("unknown operation {:?}", request.operation),
        ));
    }
    if let Some(identity) = &request.client.identity
        && (identity.len() > 254 || !identity.contains('@'))
    {
        return Err(ProtocolError::new(
            ErrorCode::ProtocolInvalid,
            "client.identity must be an e-mail",
        ));
    }
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
            "mcpName": definition.mcp_name,
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
    fn contract_has_one_typed_ping_definition() {
        let document = contract_document();
        assert_eq!(document["protocol"], 2);
        assert_eq!(document["operations"][0]["name"], "ping");
        assert!(document["operations"][0]["inputSchema"].is_object());
        assert!(document["operations"][0]["outputSchema"].is_object());
    }
}
