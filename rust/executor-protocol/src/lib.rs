//! Strict, versioned contracts for the DevCoordinator2 Rust execution plane.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

pub const EXECUTOR_SCHEMA: u8 = 2;
pub const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_REPORT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_EVENT_BYTES: usize = 4096;
pub const MAX_CASES: usize = 4096;
pub const MAX_FAILURE_INDEX: usize = 128;
pub const MAX_REASON_BYTES: usize = 512;
pub const MAX_ARG_BYTES: usize = 4096;
pub const MAX_CASE_ARG_BYTES: usize = 64 * 1024;
pub const MAX_COMMAND_ARGS: usize = 256;
pub const MAX_CASE_ARGS: usize = 64;

/// A schema marker that can only deserialize the current executor contract.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Schema2;

impl Serialize for Schema2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(EXECUTOR_SCHEMA)
    }
}

impl<'de> Deserialize<'de> for Schema2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        if value == EXECUTOR_SCHEMA {
            Ok(Self)
        } else {
            Err(de::Error::custom(format!(
                "executor schema must be {EXECUTOR_SCHEMA}; schema {value} is unsupported"
            )))
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValidationTier {
    Development,
    PreMerge,
    Release,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofKind {
    Complete,
    Selected,
    Retry,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckRole {
    Work,
    Preflight,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionMode {
    Process,
    Event,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    Continue,
    Stop,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseSpec {
    pub id: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckPlan {
    pub name: String,
    pub tier: ValidationTier,
    pub role: CheckRole,
    pub after: Vec<String>,
    pub requires: Vec<String>,
    pub invalidates: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub timeout_seconds: Option<u64>,
    pub completion: CompletionMode,
    pub on_failure: FailureMode,
    pub produces: Vec<String>,
    pub command: Option<Vec<String>>,
    pub discover: Option<Vec<String>>,
    pub case_command: Option<Vec<String>>,
    pub cases: Option<Vec<CaseSpec>>,
}

impl CheckPlan {
    pub fn is_fanout(&self) -> bool {
        self.case_command.is_some()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlan {
    pub schema: Schema2,
    pub run_id: String,
    pub test: String,
    pub worktree_root: String,
    pub current_dir: String,
    pub requested_tier: ValidationTier,
    pub readiness_eligible: bool,
    pub proof: ProofKind,
    pub selection: Vec<String>,
    pub origin_run_id: Option<String>,
    pub source_digest: String,
    pub config_digest: String,
    pub reused: BTreeMap<String, Vec<ArtifactReceipt>>,
    pub checks: Vec<CheckPlan>,
}

impl ExecutionPlan {
    pub fn from_json(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > MAX_REPORT_BYTES {
            return Err(ContractError::new("executor plan exceeds 2 MiB"));
        }
        let plan: Self = serde_json::from_slice(input)
            .map_err(|error| ContractError::new(format!("invalid executor JSON: {error}")))?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn from_toml(input: &str) -> Result<Self, ContractError> {
        if input.len() > MAX_REPORT_BYTES {
            return Err(ContractError::new("executor plan exceeds 2 MiB"));
        }
        let plan: Self = toml::from_str(input)
            .map_err(|error| ContractError::new(format!("invalid executor TOML: {error}")))?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn active_checks(&self) -> impl Iterator<Item = &CheckPlan> {
        self.checks
            .iter()
            .filter(|check| check.tier <= self.requested_tier)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        validate_identity("run_id", &self.run_id, 128)?;
        validate_name("test", &self.test, 32)?;
        validate_digest("source_digest", &self.source_digest)?;
        validate_digest("config_digest", &self.config_digest)?;
        if self.checks.is_empty() {
            return Err(ContractError::new("executor plan has no checks"));
        }
        if self.checks.len() > 256 {
            return Err(ContractError::new("executor plan exceeds 256 checks"));
        }
        if self.origin_run_id.is_some() != (self.proof == ProofKind::Retry) {
            return Err(ContractError::new(
                "origin_run_id is required only for retry proof",
            ));
        }
        if let Some(origin) = &self.origin_run_id {
            validate_identity("origin_run_id", origin, 128)?;
        }
        let expected_readiness = self.requested_tier == ValidationTier::Release
            && self.proof == ProofKind::Complete
            && self.selection.is_empty();
        if self.readiness_eligible != expected_readiness {
            return Err(ContractError::new(
                "readiness_eligible does not match tier, proof, and selection",
            ));
        }
        if self.proof == ProofKind::Complete && !self.selection.is_empty() {
            return Err(ContractError::new(
                "complete proof cannot carry a check selection",
            ));
        }
        if self.proof != ProofKind::Complete && self.selection.is_empty() {
            return Err(ContractError::new(
                "selected and retry proofs require a non-empty selection",
            ));
        }
        if !Path::new(&self.worktree_root).is_absolute()
            || !Path::new(&self.current_dir).is_absolute()
        {
            return Err(ContractError::new(
                "worktree_root and current_dir must be absolute paths",
            ));
        }

        let mut names = BTreeMap::new();
        for (index, check) in self.checks.iter().enumerate() {
            validate_check(check)?;
            if names.insert(check.name.as_str(), index).is_some() {
                return Err(ContractError::new(format!(
                    "duplicate check name {:?}",
                    check.name
                )));
            }
        }

        let selected: BTreeSet<&str> = self.selection.iter().map(String::as_str).collect();
        if selected.len() != self.selection.len() {
            return Err(ContractError::new("selection contains duplicate checks"));
        }
        if self.selection.len() > 256 {
            return Err(ContractError::new("selection exceeds 256 checks"));
        }
        for name in &self.selection {
            if !names.contains_key(name.as_str()) {
                return Err(ContractError::new(format!(
                    "selection references unknown check {name:?}"
                )));
            }
        }
        for (name, receipts) in &self.reused {
            let Some(index) = names.get(name.as_str()) else {
                return Err(ContractError::new(format!(
                    "reused evidence references unknown check {name:?}"
                )));
            };
            if self.checks[*index].tier > self.requested_tier {
                return Err(ContractError::new(format!(
                    "reused evidence references inactive check {name:?}"
                )));
            }
            if receipts.is_empty() {
                return Err(ContractError::new(format!(
                    "reused check {name:?} has no artifact receipts"
                )));
            }
            for receipt in receipts {
                validate_relative_path("artifact", &receipt.path)?;
                validate_digest("artifact sha256", &receipt.sha256)?;
            }
        }

        let mut graph = vec![Vec::<usize>::new(); self.checks.len()];
        let mut compiled_edges = BTreeSet::new();
        let mut requires_edges = BTreeSet::new();
        for (index, check) in self.checks.iter().enumerate() {
            let mut local_edges = BTreeSet::new();
            for dependency in check.after.iter().chain(&check.requires) {
                let Some(dep_index) = names.get(dependency.as_str()) else {
                    return Err(ContractError::new(format!(
                        "check {:?} references unknown dependency {dependency:?}",
                        check.name
                    )));
                };
                if *dep_index == index {
                    return Err(ContractError::new(format!(
                        "check {:?} depends on itself",
                        check.name
                    )));
                }
                if !local_edges.insert(*dep_index) {
                    return Err(ContractError::new(format!(
                        "check {:?} repeats dependency {dependency:?}",
                        check.name
                    )));
                }
                if self.checks[*dep_index].tier > check.tier {
                    return Err(ContractError::new(format!(
                        "check {:?} depends on higher-tier check {dependency:?}",
                        check.name
                    )));
                }
                graph[*dep_index].push(index);
                compiled_edges.insert((*dep_index, index));
            }
            for dependency in &check.requires {
                let dep_index = names[dependency.as_str()];
                requires_edges.insert((dep_index, index));
            }
        }
        for (preflight_index, preflight) in self.checks.iter().enumerate() {
            for target in &preflight.invalidates {
                let Some(target_index) = names.get(target.as_str()) else {
                    return Err(ContractError::new(format!(
                        "preflight {:?} invalidates unknown check {target:?}",
                        preflight.name
                    )));
                };
                if *target_index == preflight_index {
                    return Err(ContractError::new(format!(
                        "preflight {:?} invalidates itself",
                        preflight.name
                    )));
                }
                if self.checks[*target_index].tier < preflight.tier {
                    return Err(ContractError::new(format!(
                        "preflight {:?} invalidates lower-tier check {target:?}",
                        preflight.name
                    )));
                }
                if !compiled_edges.insert((preflight_index, *target_index))
                    && !requires_edges.contains(&(preflight_index, *target_index))
                {
                    return Err(ContractError::new(format!(
                        "preflight {:?} duplicates a dependency edge to {target:?}",
                        preflight.name
                    )));
                }
                if !requires_edges.contains(&(preflight_index, *target_index)) {
                    graph[preflight_index].push(*target_index);
                }
            }
        }
        ensure_acyclic(&graph, &self.checks)?;
        Ok(())
    }
}

fn validate_check(check: &CheckPlan) -> Result<(), ContractError> {
    validate_name("check", &check.name, 64)?;
    if check.cwd != "." {
        validate_relative_path("cwd", &check.cwd)?;
    }
    if let Some(timeout_seconds) = check.timeout_seconds
        && !(1..=21_600).contains(&timeout_seconds)
    {
        return Err(ContractError::new(format!(
            "check {:?} timeout_seconds must be null or in [1, 21600]",
            check.name
        )));
    }
    let direct = check.command.is_some();
    let dynamic = check.discover.is_some();
    let static_cases = check.cases.is_some();
    let fanout = check.case_command.is_some();
    if direct == fanout
        || (!fanout && (dynamic || static_cases))
        || (fanout && dynamic == static_cases)
    {
        return Err(ContractError::new(format!(
            "check {:?} must declare exactly one command or one discover/static fan-out",
            check.name
        )));
    }
    for command in [&check.command, &check.discover, &check.case_command]
        .into_iter()
        .flatten()
    {
        validate_command(&check.name, command)?;
    }
    if fanout && check.completion != CompletionMode::Process {
        return Err(ContractError::new(format!(
            "fan-out check {:?} must use process completion",
            check.name
        )));
    }
    if check.role == CheckRole::Preflight && check.completion != CompletionMode::Process {
        return Err(ContractError::new(format!(
            "preflight {:?} must use process completion",
            check.name
        )));
    }
    if check.role != CheckRole::Preflight && !check.invalidates.is_empty() {
        return Err(ContractError::new(format!(
            "work check {:?} cannot invalidate other checks",
            check.name
        )));
    }
    let invalidates: BTreeSet<&str> = check.invalidates.iter().map(String::as_str).collect();
    if invalidates.len() != check.invalidates.len() {
        return Err(ContractError::new(format!(
            "preflight {:?} repeats an invalidated check",
            check.name
        )));
    }
    for key in check.env.keys() {
        if key.starts_with("DEVCOORDINATOR_") {
            return Err(ContractError::new(format!(
                "check {:?} overrides reserved environment variable {key:?}",
                check.name
            )));
        }
        if key.is_empty() || key.as_bytes().contains(&b'=') || key.as_bytes().contains(&0) {
            return Err(ContractError::new(format!(
                "check {:?} contains an invalid environment name",
                check.name
            )));
        }
    }
    let produces: BTreeSet<&str> = check.produces.iter().map(String::as_str).collect();
    if check.produces.len() > 16 {
        return Err(ContractError::new(format!(
            "check {:?} declares more than 16 artifacts",
            check.name
        )));
    }
    if produces.len() != check.produces.len() {
        return Err(ContractError::new(format!(
            "check {:?} repeats an artifact path",
            check.name
        )));
    }
    for path in &check.produces {
        validate_relative_path("artifact", path)?;
    }
    if let Some(cases) = &check.cases {
        validate_cases(cases)?;
    }
    Ok(())
}

pub fn validate_cases(cases: &[CaseSpec]) -> Result<(), ContractError> {
    if cases.len() > MAX_CASES {
        return Err(ContractError::new(format!(
            "case manifest exceeds {MAX_CASES} cases"
        )));
    }
    let mut ids = BTreeSet::new();
    for case in cases {
        validate_case_id(&case.id)?;
        if !ids.insert(case.id.as_str()) {
            return Err(ContractError::new(format!(
                "duplicate case id {:?}",
                case.id
            )));
        }
        if case.args.len() > MAX_CASE_ARGS {
            return Err(ContractError::new(format!(
                "case {:?} has too many arguments",
                case.id
            )));
        }
        let mut total = 0usize;
        for arg in &case.args {
            if arg.is_empty() || arg.as_bytes().contains(&0) || arg.len() > MAX_ARG_BYTES {
                return Err(ContractError::new(format!(
                    "case {:?} contains an invalid argument",
                    case.id
                )));
            }
            total = total.saturating_add(arg.len());
        }
        if total > MAX_CASE_ARG_BYTES {
            return Err(ContractError::new(format!(
                "case {:?} arguments exceed 64 KiB",
                case.id
            )));
        }
    }
    Ok(())
}

fn validate_command(check: &str, command: &[String]) -> Result<(), ContractError> {
    if command.is_empty() || command.len() > MAX_COMMAND_ARGS {
        return Err(ContractError::new(format!(
            "check {check:?} command must contain 1..={MAX_COMMAND_ARGS} arguments"
        )));
    }
    for (index, arg) in command.iter().enumerate() {
        if index == 0 && arg.is_empty() {
            return Err(ContractError::new(format!(
                "check {check:?} command executable is empty"
            )));
        }
        if arg.as_bytes().contains(&0) || arg.len() > MAX_ARG_BYTES {
            return Err(ContractError::new(format!(
                "check {check:?} command contains an invalid argument"
            )));
        }
    }
    Ok(())
}

fn validate_name(kind: &str, value: &str, maximum: usize) -> Result<(), ContractError> {
    let valid = !value.is_empty()
        && value.len() <= maximum
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        });
    if !valid {
        return Err(ContractError::new(format!(
            "{kind} {value:?} is not a normalized lower-case name"
        )));
    }
    Ok(())
}

fn validate_identity(kind: &str, value: &str, maximum: usize) -> Result<(), ContractError> {
    let valid = !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if !valid {
        return Err(ContractError::new(format!(
            "{kind} is not a bounded normalized identity"
        )));
    }
    Ok(())
}

fn validate_case_id(value: &str) -> Result<(), ContractError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'-' | b'_' | b'.'))
        });
    if !valid {
        return Err(ContractError::new(format!(
            "case id {value:?} is not path-safe"
        )));
    }
    Ok(())
}

fn validate_digest(kind: &str, value: &str) -> Result<(), ContractError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ContractError::new(format!(
            "{kind} must be 64 lower-case hexadecimal characters"
        )));
    }
    Ok(())
}

fn validate_relative_path(kind: &str, value: &str) -> Result<(), ContractError> {
    let path = Path::new(value);
    let valid = !value.is_empty()
        && !path.is_absolute()
        && !value.contains('\\')
        && !value.as_bytes().contains(&0)
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !valid {
        return Err(ContractError::new(format!(
            "{kind} path {value:?} is not normalized repository-relative"
        )));
    }
    Ok(())
}

fn ensure_acyclic(graph: &[Vec<usize>], checks: &[CheckPlan]) -> Result<(), ContractError> {
    fn visit(
        node: usize,
        graph: &[Vec<usize>],
        marks: &mut [u8],
        checks: &[CheckPlan],
    ) -> Result<(), ContractError> {
        if marks[node] == 1 {
            return Err(ContractError::new(format!(
                "check graph contains a cycle at {:?}",
                checks[node].name
            )));
        }
        if marks[node] == 2 {
            return Ok(());
        }
        marks[node] = 1;
        for &next in &graph[node] {
            visit(next, graph, marks, checks)?;
        }
        marks[node] = 2;
        Ok(())
    }

    let mut marks = vec![0; graph.len()];
    for node in 0..graph.len() {
        visit(node, graph, &mut marks, checks)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseManifest {
    pub schema: Schema2,
    pub cases: Vec<CaseSpec>,
}

impl CaseManifest {
    pub fn from_json(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > MAX_MANIFEST_BYTES {
            return Err(ContractError::new("case manifest exceeds 2 MiB"));
        }
        let manifest: Self = serde_json::from_slice(input)
            .map_err(|error| ContractError::new(format!("invalid case manifest: {error}")))?;
        validate_cases(&manifest.cases)?;
        Ok(manifest)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeafStatus {
    Pending,
    Running,
    Reused,
    Passed,
    Failed,
    TimedOut,
    Invalidated,
    NotMeaningful,
    Cancelled,
    Unsafe,
}

impl LeafStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::Passed | Self::Reused)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Passed,
    Failed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputStats {
    pub stdout_bytes_observed: u64,
    pub stdout_bytes_retained: u64,
    pub stdout_truncated: bool,
    pub stderr_bytes_observed: u64,
    pub stderr_bytes_retained: u64,
    pub stderr_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseReport {
    pub id: String,
    pub status: LeafStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub output_ref: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckReport {
    pub name: String,
    pub tier: ValidationTier,
    pub role: CheckRole,
    pub status: LeafStatus,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_seconds: Option<f64>,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub artifacts: Vec<ArtifactReceipt>,
    pub output_ref: String,
    pub stdout_bytes_observed: u64,
    pub stdout_bytes_retained: u64,
    pub stdout_truncated: bool,
    pub stderr_bytes_observed: u64,
    pub stderr_bytes_retained: u64,
    pub stderr_truncated: bool,
    pub case_count: u32,
    pub cases: Vec<CaseReport>,
    pub cases_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FailureIndexEntry {
    pub check: Option<String>,
    pub case: Option<String>,
    pub status: LeafStatus,
    pub reason: String,
    pub output_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReport {
    pub schema: Schema2,
    pub run_id: String,
    pub test: String,
    pub requested_tier: ValidationTier,
    pub readiness_eligible: bool,
    pub proof: ProofKind,
    pub selection: Vec<String>,
    pub origin_run_id: Option<String>,
    pub status: RunStatus,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_seconds: f64,
    pub source_digest: String,
    pub config_digest: String,
    pub source_changed: bool,
    pub unsafe_reason: Option<String>,
    pub capacity: CapacityReport,
    pub counts: BTreeMap<String, u32>,
    pub checks: Vec<CheckReport>,
    pub failure_index: Vec<FailureIndexEntry>,
    pub failure_index_truncated: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityReport {
    pub learned_capacity: Option<u32>,
    pub effective_capacity: Option<u32>,
    pub capacity_wait_count: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionEvent {
    pub schema: Schema2,
    pub run_id: String,
    pub check: String,
    pub status: EventStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    Passed,
    Failed,
    Unsafe,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractError {
    message: String,
}

impl ContractError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ContractError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, tier: ValidationTier) -> CheckPlan {
        CheckPlan {
            name: name.into(),
            tier,
            role: CheckRole::Work,
            after: Vec::new(),
            requires: Vec::new(),
            invalidates: Vec::new(),
            cwd: ".".into(),
            env: BTreeMap::new(),
            timeout_seconds: Some(30),
            completion: CompletionMode::Process,
            on_failure: FailureMode::Continue,
            produces: Vec::new(),
            command: Some(vec!["true".into()]),
            discover: None,
            case_command: None,
            cases: None,
        }
    }

    fn plan(checks: Vec<CheckPlan>) -> ExecutionPlan {
        ExecutionPlan {
            schema: Schema2,
            run_id: "run-1".into(),
            test: "complete".into(),
            worktree_root: "/tmp/repo".into(),
            current_dir: "/tmp/repo/.devcoordinator/current".into(),
            requested_tier: ValidationTier::Release,
            readiness_eligible: true,
            proof: ProofKind::Complete,
            selection: Vec::new(),
            origin_run_id: None,
            source_digest: "a".repeat(64),
            config_digest: "b".repeat(64),
            reused: BTreeMap::new(),
            checks,
        }
    }

    #[test]
    fn schema_one_is_rejected_without_fallback() {
        let json = serde_json::to_vec(&plan(vec![check("unit", ValidationTier::Development)]))
            .expect("serialize");
        let old =
            String::from_utf8(json)
                .expect("utf8")
                .replacen("\"schema\":2", "\"schema\":1", 1);
        let error = ExecutionPlan::from_json(old.as_bytes()).expect_err("schema one rejected");
        assert!(error.to_string().contains("schema must be 2"));
    }

    #[test]
    fn cycles_include_compiled_preflight_edges() {
        let mut preflight = check("preflight", ValidationTier::Development);
        preflight.role = CheckRole::Preflight;
        preflight.invalidates = vec!["work".into()];
        preflight.requires = vec!["work".into()];
        let work = check("work", ValidationTier::Development);
        let error = plan(vec![preflight, work])
            .validate()
            .expect_err("cycle rejected");
        assert!(error.to_string().contains("cycle"));
    }

    #[test]
    fn lower_tier_cannot_depend_on_higher_tier() {
        let release = check("release", ValidationTier::Release);
        let mut development = check("development", ValidationTier::Development);
        development.requires.push("release".into());
        let error = plan(vec![release, development])
            .validate()
            .expect_err("tier inversion rejected");
        assert!(error.to_string().contains("higher-tier"));
    }

    #[test]
    fn nested_or_ambiguous_fanout_is_rejected() {
        let mut invalid = check("fanout", ValidationTier::Development);
        invalid.command = Some(vec!["true".into()]);
        invalid.discover = Some(vec!["discover".into()]);
        invalid.case_command = Some(vec!["case".into()]);
        let error = plan(vec![invalid])
            .validate()
            .expect_err("ambiguous fanout rejected");
        assert!(error.to_string().contains("exactly one"));
    }

    #[test]
    fn compiled_invalidation_requires_pair_is_canonical() {
        let mut preflight = check("preflight", ValidationTier::Development);
        preflight.role = CheckRole::Preflight;
        preflight.invalidates = vec!["work".into()];
        let mut work = check("work", ValidationTier::Development);
        work.requires = vec!["preflight".into()];
        plan(vec![preflight, work])
            .validate()
            .expect("compiled invalidation edge accepted");
    }

    #[test]
    fn malformed_and_oversized_manifests_are_rejected() {
        assert!(CaseManifest::from_json(br#"{"schema":2,"cases":"bad"}"#).is_err());
        let oversized = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert!(CaseManifest::from_json(&oversized).is_err());
    }

    #[test]
    fn duplicate_case_ids_are_rejected() {
        let manifest = CaseManifest {
            schema: Schema2,
            cases: vec![
                CaseSpec {
                    id: "same".into(),
                    args: Vec::new(),
                },
                CaseSpec {
                    id: "same".into(),
                    args: Vec::new(),
                },
            ],
        };
        assert!(validate_cases(&manifest.cases).is_err());
    }
}
