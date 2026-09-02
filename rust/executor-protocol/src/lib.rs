//! Strict, versioned contracts for the DevCoordinator2 Rust execution plane.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

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
pub const MAX_DIAGNOSTIC_SOURCES: usize = 8;
pub const MAX_DIAGNOSTIC_EVENTS: usize = 4096;
pub const MAX_DIAGNOSTIC_PREVIEW_BYTES: usize = 256;
pub const MAX_DIAGNOSTIC_NAME_BYTES: usize = 256;
pub const MAX_DIAGNOSTIC_PATH_BYTES: usize = 512;
pub const MAX_DIAGNOSTIC_LOG_REFS: usize = 16;

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticReportFormat {
    Junit,
    PlaywrightJson,
    RustJson,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticReportSource {
    pub format: DiagnosticReportFormat,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogPhase {
    Executor,
    Check,
    Discovery,
    Case,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogRef {
    pub run_id: String,
    pub check: Option<String>,
    pub phase: LogPhase,
    pub case: Option<String>,
    pub stream: LogStream,
}

impl LogRef {
    pub fn validate(&self) -> Result<(), ContractError> {
        validate_identity("log run_id", &self.run_id, 128)?;
        match self.phase {
            LogPhase::Executor => {
                if self.check.is_some() || self.case.is_some() {
                    return Err(ContractError::new(
                        "executor log references cannot name a check or case",
                    ));
                }
            }
            LogPhase::Check | LogPhase::Discovery => {
                let Some(check) = &self.check else {
                    return Err(ContractError::new(
                        "check and discovery log references require a check",
                    ));
                };
                validate_name("log check", check, 64)?;
                if self.case.is_some() {
                    return Err(ContractError::new(
                        "only case log references may name a case",
                    ));
                }
            }
            LogPhase::Case => {
                let Some(check) = &self.check else {
                    return Err(ContractError::new("case log references require a check"));
                };
                let Some(case) = &self.case else {
                    return Err(ContractError::new("case log references require a case"));
                };
                validate_name("log check", check, 64)?;
                validate_case_id(case)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Assertion,
    Compiler,
    Panic,
    Exception,
    StackFrame,
    Timeout,
    Cancellation,
    ProcessExit,
    BrowserConsole,
    NetworkRequest,
    StructuredEvidenceInvalid,
    LogStorage,
    SourceChanged,
    Artifact,
    Dependency,
    Internal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    DeadlineExceeded,
    UserCancelled,
    Superseded,
    RunCancelled,
    DaemonInterrupted,
    UnsafeStop,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOrigin {
    ExplicitEvent,
    Junit,
    PlaywrightJson,
    RustJson,
    Executor,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticValueType {
    Null,
    Boolean,
    Number,
    String,
    Json,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticValue {
    #[serde(rename = "type")]
    pub value_type: DiagnosticValueType,
    pub preview: Option<String>,
    pub sha256: String,
    pub byte_count: u64,
    pub truncated: bool,
    pub redacted: bool,
}

impl DiagnosticValue {
    pub fn validate(&self) -> Result<(), ContractError> {
        validate_digest("diagnostic value sha256", &self.sha256)?;
        if self.redacted && self.preview.is_some() {
            return Err(ContractError::new(
                "redacted diagnostic values cannot include a preview",
            ));
        }
        if let Some(preview) = &self.preview {
            if preview.len() > MAX_DIAGNOSTIC_PREVIEW_BYTES
                || preview.chars().any(char::is_control)
                || u64::try_from(preview.len()).unwrap_or(u64::MAX) > self.byte_count
            {
                return Err(ContractError::new(
                    "diagnostic value preview is not a bounded single-line value",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
}

impl SourceLocation {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.file.len() > MAX_DIAGNOSTIC_PATH_BYTES {
            return Err(ContractError::new(
                "diagnostic source path exceeds 512 bytes",
            ));
        }
        validate_relative_path("diagnostic source", &self.file)?;
        if self.line == 0 || self.column == Some(0) {
            return Err(ContractError::new(
                "diagnostic source line and column are one-based",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticExit {
    pub code: Option<i32>,
    pub signal: Option<u8>,
}

impl DiagnosticExit {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.code.is_some() && self.signal.is_some() {
            return Err(ContractError::new(
                "diagnostic exit cannot contain both a code and signal",
            ));
        }
        if self.signal == Some(0) || self.signal.is_some_and(|signal| signal > 127) {
            return Err(ContractError::new("diagnostic signal must be in [1, 127]"));
        }
        Ok(())
    }
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
    #[serde(default)]
    pub diagnostic_sources: Vec<DiagnosticReportSource>,
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
    pub log_dir: String,
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
            || !Path::new(&self.log_dir).is_absolute()
        {
            return Err(ContractError::new(
                "worktree_root, current_dir, and log_dir must be absolute paths",
            ));
        }
        let normalized_root = normalize_absolute_path(Path::new(&self.worktree_root))
            .ok_or_else(|| ContractError::new("worktree_root cannot escape its filesystem root"))?;
        let normalized_log = normalize_absolute_path(Path::new(&self.log_dir))
            .ok_or_else(|| ContractError::new("log_dir cannot escape its filesystem root"))?;
        if normalized_log == normalized_root || !normalized_log.starts_with(&normalized_root) {
            return Err(ContractError::new(
                "log_dir must be contained below worktree_root",
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
    if check.diagnostic_sources.len() > MAX_DIAGNOSTIC_SOURCES {
        return Err(ContractError::new(format!(
            "check {:?} declares more than {MAX_DIAGNOSTIC_SOURCES} diagnostic sources",
            check.name
        )));
    }
    let mut diagnostic_sources = BTreeSet::new();
    for source in &check.diagnostic_sources {
        if source.path.len() > MAX_DIAGNOSTIC_PATH_BYTES {
            return Err(ContractError::new(format!(
                "check {:?} diagnostic source path exceeds 512 bytes",
                check.name
            )));
        }
        validate_relative_path("diagnostic report", &source.path)?;
        if !diagnostic_sources.insert((source.format, source.path.as_str())) {
            return Err(ContractError::new(format!(
                "check {:?} repeats a diagnostic source",
                check.name
            )));
        }
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

fn validate_fingerprint(value: &str) -> Result<(), ContractError> {
    let Some(digest) = value.strip_prefix("sha256:") else {
        return Err(ContractError::new(
            "diagnostic fingerprint must use the sha256 scheme",
        ));
    };
    validate_digest("diagnostic fingerprint", digest)
}

fn validate_diagnostic_name(kind: &str, value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > MAX_DIAGNOSTIC_NAME_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ContractError::new(format!(
            "{kind} is not a bounded single-line name"
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

fn normalize_absolute_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Some(normalized)
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
    pub exit: DiagnosticExit,
    pub termination_reason: Option<TerminationReason>,
    pub source: Option<SourceLocation>,
    pub error_category: ErrorCategory,
    pub expected: Option<DiagnosticValue>,
    pub actual: Option<DiagnosticValue>,
    pub fingerprint: String,
    pub occurrences: u32,
    pub log_refs: Vec<LogRef>,
    pub origin: DiagnosticOrigin,
}

impl FailureIndexEntry {
    pub fn validate(&self) -> Result<(), ContractError> {
        if let Some(check) = &self.check {
            validate_name("diagnostic check", check, 64)?;
        }
        if let Some(case) = &self.case {
            if self.check.is_none() {
                return Err(ContractError::new(
                    "diagnostic case cannot be present without a check",
                ));
            }
            validate_diagnostic_name("diagnostic case", case)?;
        }
        if !matches!(
            self.status,
            LeafStatus::Failed
                | LeafStatus::TimedOut
                | LeafStatus::Invalidated
                | LeafStatus::NotMeaningful
                | LeafStatus::Cancelled
                | LeafStatus::Unsafe
        ) {
            return Err(ContractError::new(
                "failure diagnostics require a terminal non-success status",
            ));
        }
        self.exit.validate()?;
        if let Some(source) = &self.source {
            source.validate()?;
        }
        if let Some(expected) = &self.expected {
            expected.validate()?;
        }
        if let Some(actual) = &self.actual {
            actual.validate()?;
        }
        validate_fingerprint(&self.fingerprint)?;
        if self.occurrences == 0 {
            return Err(ContractError::new(
                "diagnostic occurrence count must be positive",
            ));
        }
        if self.log_refs.len() > MAX_DIAGNOSTIC_LOG_REFS {
            return Err(ContractError::new(format!(
                "diagnostic has more than {MAX_DIAGNOSTIC_LOG_REFS} log references"
            )));
        }
        let mut refs = BTreeSet::new();
        for log_ref in &self.log_refs {
            log_ref.validate()?;
            if !refs.insert(log_ref) {
                return Err(ContractError::new("diagnostic repeats a log reference"));
            }
        }
        Ok(())
    }
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticEvent {
    pub schema: Schema2,
    pub run_id: String,
    pub check: String,
    pub case: Option<String>,
    pub status: LeafStatus,
    pub exit: DiagnosticExit,
    pub termination_reason: Option<TerminationReason>,
    pub source: Option<SourceLocation>,
    pub error_category: ErrorCategory,
    pub expected: Option<DiagnosticValue>,
    pub actual: Option<DiagnosticValue>,
    pub log_refs: Vec<LogRef>,
}

impl DiagnosticEvent {
    pub fn from_json(
        input: &[u8],
        expected_run_id: &str,
        expected_check: &str,
        expected_case: Option<&str>,
    ) -> Result<Self, ContractError> {
        if input.len() > MAX_EVENT_BYTES {
            return Err(ContractError::new("diagnostic event exceeds 4096 bytes"));
        }
        let event: Self = serde_json::from_slice(input)
            .map_err(|_| ContractError::new("diagnostic event is not valid schema-2 JSON"))?;
        event.validate(expected_run_id, expected_check, expected_case)?;
        Ok(event)
    }

    pub fn validate(
        &self,
        expected_run_id: &str,
        expected_check: &str,
        expected_case: Option<&str>,
    ) -> Result<(), ContractError> {
        validate_identity("diagnostic run_id", &self.run_id, 128)?;
        validate_name("diagnostic check", &self.check, 64)?;
        if self.run_id != expected_run_id
            || self.check != expected_check
            || self.case.as_deref() != expected_case
        {
            return Err(ContractError::new(
                "diagnostic event identity does not match its execution leaf",
            ));
        }
        if let Some(case) = &self.case {
            validate_case_id(case)?;
        }
        if !matches!(
            self.status,
            LeafStatus::Failed | LeafStatus::TimedOut | LeafStatus::Cancelled | LeafStatus::Unsafe
        ) {
            return Err(ContractError::new(
                "diagnostic events require a failed, timed_out, cancelled, or unsafe status",
            ));
        }
        self.exit.validate()?;
        if let Some(source) = &self.source {
            source.validate()?;
        }
        if let Some(expected) = &self.expected {
            expected.validate()?;
        }
        if let Some(actual) = &self.actual {
            actual.validate()?;
        }
        if self.log_refs.len() > MAX_DIAGNOSTIC_LOG_REFS {
            return Err(ContractError::new(format!(
                "diagnostic event has more than {MAX_DIAGNOSTIC_LOG_REFS} log references"
            )));
        }
        let mut refs = BTreeSet::new();
        for log_ref in &self.log_refs {
            log_ref.validate()?;
            if log_ref.run_id != self.run_id || log_ref.check.as_deref() != Some(&self.check) {
                return Err(ContractError::new(
                    "diagnostic event log reference belongs to another leaf",
                ));
            }
            if log_ref.case != self.case {
                return Err(ContractError::new(
                    "diagnostic event case log reference belongs to another case",
                ));
            }
            if !refs.insert(log_ref) {
                return Err(ContractError::new(
                    "diagnostic event repeats a log reference",
                ));
            }
        }
        Ok(())
    }
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
            diagnostic_sources: Vec::new(),
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
            log_dir: "/tmp/repo/.devcoordinator/test/logs/runs/run-1".into(),
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

    #[test]
    fn log_directory_is_required_and_must_remain_below_the_worktree() {
        let valid = plan(vec![check("unit", ValidationTier::Development)]);
        valid.validate().expect("contained log directory accepted");

        let json = serde_json::to_value(&valid).expect("serialize plan");
        let mut missing = json.clone();
        missing.as_object_mut().expect("object").remove("log_dir");
        let encoded = serde_json::to_vec(&missing).expect("encode missing plan");
        assert!(ExecutionPlan::from_json(&encoded).is_err());

        let mut escaped = valid.clone();
        escaped.log_dir = "/tmp/repo/../outside".into();
        assert!(escaped.validate().is_err());

        let mut normalized = valid;
        normalized.log_dir =
            "/tmp/repo/.devcoordinator/../.devcoordinator/test/logs/runs/run-1".into();
        normalized
            .validate()
            .expect("lexically normalized contained path accepted");
    }

    #[test]
    fn diagnostic_sources_are_strict_bounded_and_relative() {
        let mut unit = check("unit", ValidationTier::Development);
        unit.diagnostic_sources = vec![DiagnosticReportSource {
            format: DiagnosticReportFormat::Junit,
            path: "junit/results.xml".into(),
        }];
        plan(vec![unit.clone()])
            .validate()
            .expect("one relative source accepted");

        unit.diagnostic_sources
            .push(unit.diagnostic_sources[0].clone());
        assert!(plan(vec![unit]).validate().is_err());

        let mut escaped = check("unit", ValidationTier::Development);
        escaped.diagnostic_sources = vec![DiagnosticReportSource {
            format: DiagnosticReportFormat::PlaywrightJson,
            path: "../report.json".into(),
        }];
        assert!(plan(vec![escaped]).validate().is_err());
    }

    fn log_ref(case: Option<&str>) -> LogRef {
        LogRef {
            run_id: "run-1".into(),
            check: Some("unit".into()),
            phase: if case.is_some() {
                LogPhase::Case
            } else {
                LogPhase::Check
            },
            case: case.map(str::to_owned),
            stream: LogStream::Stderr,
        }
    }

    #[test]
    fn log_references_enforce_phase_identity() {
        log_ref(Some("parser-17"))
            .validate()
            .expect("complete case selector accepted");
        let mut missing_case = log_ref(Some("parser-17"));
        missing_case.case = None;
        assert!(missing_case.validate().is_err());
        let executor = LogRef {
            run_id: "run-1".into(),
            check: Some("unit".into()),
            phase: LogPhase::Executor,
            case: None,
            stream: LogStream::Stdout,
        };
        assert!(executor.validate().is_err());
    }

    #[test]
    fn diagnostic_event_is_identity_bound_and_rejects_raw_prose() {
        let event = serde_json::json!({
            "schema": 2,
            "run_id": "run-1",
            "check": "unit",
            "case": "parser-17",
            "status": "failed",
            "exit": {"code": 1, "signal": null},
            "termination_reason": null,
            "source": {"file": "src/parser.rs", "line": 81, "column": 9},
            "error_category": "assertion",
            "expected": null,
            "actual": null,
            "log_refs": [log_ref(Some("parser-17"))]
        });
        let encoded = serde_json::to_vec(&event).expect("event JSON");
        DiagnosticEvent::from_json(&encoded, "run-1", "unit", Some("parser-17"))
            .expect("matching event accepted");
        assert!(DiagnosticEvent::from_json(&encoded, "other", "unit", Some("parser-17")).is_err());

        let mut with_message = event;
        with_message
            .as_object_mut()
            .expect("object")
            .insert("message".into(), serde_json::json!("raw stack trace"));
        let encoded = serde_json::to_vec(&with_message).expect("event JSON");
        assert!(DiagnosticEvent::from_json(&encoded, "run-1", "unit", Some("parser-17")).is_err());
    }

    #[test]
    fn diagnostic_values_and_sources_are_bounded() {
        let value = DiagnosticValue {
            value_type: DiagnosticValueType::String,
            preview: Some("ready".into()),
            sha256: "a".repeat(64),
            byte_count: 5,
            truncated: false,
            redacted: false,
        };
        value.validate().expect("bounded value accepted");
        let mut multiline = value;
        multiline.preview = Some("first\nsecond".into());
        assert!(multiline.validate().is_err());

        SourceLocation {
            file: "src/parser.rs".into(),
            line: 1,
            column: None,
        }
        .validate()
        .expect("relative source accepted");
        assert!(
            SourceLocation {
                file: "/etc/passwd".into(),
                line: 1,
                column: None,
            }
            .validate()
            .is_err()
        );
    }
}
