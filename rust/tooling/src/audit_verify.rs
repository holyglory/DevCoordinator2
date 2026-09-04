//! Semantic verifier for full-repository audit artifacts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::audit_common::{
    interaction_checklist_missing, is_separator_row, parse_markdown_table_dicts, section_bodies,
    section_order, split_markdown_row,
};
use crate::audit_evidence::{
    EvidenceRecords, evidence_references, validate_references, validate_visual_evidence_manifest,
};
use crate::audit_findings::{canonical_json_sha256, manifest_report_names, strict_json_object};
use crate::audit_ledger::{read_bytes_nofollow, validate_directory_nofollow, write_bytes_nofollow};
use crate::audit_queue::{is_test_source_path, validate_repo_relative_path_token};

const REQUIRED_SECTIONS: [&str; 9] = [
    "run id",
    "batch id",
    "batch summary",
    "file coverage",
    "implementation inventory",
    "interface inventory",
    "findings",
    "no finding notes",
    "open questions",
];
const IMPLEMENTATION_HEADERS: [&str; 8] = [
    "file/unit",
    "contract id",
    "contract/responsibility",
    "entrypoints/source anchors",
    "implementation/data/side-effect trace",
    "failure/edge/permission/recovery trace",
    "verification evidence",
    "result",
];
const INTERFACE_HEADERS: [&str; 5] = [
    "file",
    "surface",
    "visible text/control/message",
    "expected behavior path",
    "actual implementation notes",
];
const LEAD_SECTIONS: [&str; 5] = [
    "run id",
    "worker",
    "cross-file contract trace",
    "findings",
    "open questions",
];
const LEAD_HEADERS: [&str; 13] = [
    "contract id",
    "batch contract ids",
    "contract/source anchors",
    "entry-registration",
    "core-logic",
    "data-lifecycle",
    "integration-boundary",
    "authorization-trust",
    "failure-recovery",
    "observable-outcome",
    "operational-lifecycle",
    "verification",
    "result",
];
const LEAD_TRACE_FIELDS: [&str; 9] = [
    "entry-registration",
    "core-logic",
    "data-lifecycle",
    "integration-boundary",
    "authorization-trust",
    "failure-recovery",
    "observable-outcome",
    "operational-lifecycle",
    "verification",
];
const REQUIRED_FINDING_FIELDS: [&str; 6] = [
    "files",
    "evidence",
    "interface evidence",
    "expected behavior/standard",
    "gap",
    "suggested direction",
];
const BASIS_KINDS: [&str; 9] = [
    "user-requirement",
    "acceptance-criterion",
    "recorded-decision",
    "public-contract",
    "interface-promise",
    "caller-contract",
    "schema-invariant",
    "operational-contract",
    "source-inferred",
];
const RESULT_VALUES: [&str; 3] = ["PASS", "GAP", "BLOCKED"];
const RESULT_ARRAY_KEYS: [&str; 29] = [
    "missing",
    "extra",
    "duplicate",
    "unchecked",
    "missing_batch_reports",
    "duplicate_batch_reports",
    "unassigned_reports",
    "report_location_mismatches",
    "run_id_mismatches",
    "report_hash_mismatches",
    "current_hash_mismatches",
    "current_hash_errors",
    "source_text_errors",
    "verification_warnings",
    "unresolved_scope_warnings",
    "excluded_file_mismatches",
    "completion_marker_mismatches",
    "effort_ledger_mismatches",
    "batch_id_mismatches",
    "malformed_rows",
    "lead_reconciliation_issues",
    "implementation_inventory_issues",
    "interface_inventory_issues",
    "finding_schema_issues",
    "placeholder_omissions",
    "interface_control_omissions",
    "missing_sections",
    "section_shape_mismatches",
    "semantic_report_issues",
];

static SHA256_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-fA-F]{64}$").expect("constant SHA-256 regex"));
static BATCH_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bbatch_(\d{3,})\b").expect("constant batch-id regex"));
static BATCH_CONTRACT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(batch_\d{3,}):C\d{3,}$").expect("constant batch-contract regex")
});
static LEAD_CONTRACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^lead:C\d{3,}$").expect("constant lead-contract regex"));
static FINDING_HEADING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^###\s+P[0-3]\s+-\s+\S").expect("constant finding-heading regex")
});
static FINDING_FIELD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-\s*([^:]+):\s*(.*)$").expect("constant finding-field regex"));
static BACKTICK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`]+)`").expect("constant backtick regex"));
static BASIS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bBasis:\s*([a-z][a-z-]*)\s*(?:[-–—:])\s*(.+?)\s+Discovery:")
        .expect("constant basis regex")
});
static DISCOVERY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bDiscovery:\s*(parsed|manual)\s*(?:[-–—:])\s*(.+?)\s*$")
        .expect("constant discovery regex")
});
static TRACE_STATUS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(pass|gap|blocked|not applicable)\s*(?:[-–—:])\s*(.+?)\s*$")
        .expect("constant trace-status regex")
});
static EVIDENCE_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bevidence-type:\s*(test|runtime|source-only)\b")
        .expect("constant evidence-type regex")
});
static EVIDENCE_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bevidence-ref:\s*([^;|]+)").expect("constant evidence-ref regex")
});
static EXPECTATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(counterfactual|invariance):\s*([^;|]+(?:\s+[^;|]+)*)")
        .expect("constant expectation regex")
});
static OUTCOME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:outcome|result):\s*([^;|]+)").expect("constant outcome regex")
});
static BEHAVIOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:assert(?:s|ed|ing)?|accept(?:s|ed|ing)?|block(?:s|ed|ing)?|calculat(?:e|es|ed|ing)|call(?:s|ed|ing)?|compar(?:e|es|ed|ing)|confirm(?:s|ed|ing)?|cover(?:s|ed|ing)?|creat(?:e|es|ed|ing)|delet(?:e|es|ed|ing)|emit(?:s|ted|ting)?|exercis(?:e|es|ed|ing)|fail(?:s|ed|ing)?|invok(?:e|es|ed|ing)|load(?:s|ed|ing)?|persist(?:s|ed|ing)?|prov(?:e|es|ed|ing)|read(?:s|ing)?|register(?:s|ed|ing)?|reject(?:s|ed|ing)?|render(?:s|ed|ing)?|report(?:s|ed|ing)?|return(?:s|ed|ing)?|sav(?:e|es|ed|ing)|updat(?:e|es|ed|ing)|validat(?:e|es|ed|ing)|verif(?:y|ies|ied|ying)|writ(?:e|es|ten|ing))\b")
        .expect("constant behavior regex")
});
static DIRECTORY_PURPOSE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:files?|sources?|items?|entries|everything)\s+(?:under|in|from|inside)\b|\b(?:directory|folder)\s+(?:of|for|under|inside|containing)\b")
        .expect("constant directory-purpose regex")
});
static BROWSER_TEST_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|/)[^`/]*browser[^`/]*\.test\.(?:[cm]?[jt]sx?)$")
        .expect("constant browser-test regex")
});
static BROWSER_COMMAND_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bcommand:\s*[^;|]*(?:node|npm|playwright)\b")
        .expect("constant browser-command regex")
});
static BROWSER_PASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:pass(?:ed|ing)?\s*(?:=|:)?\s*[1-9]\d*|[1-9]\d*\s+pass(?:ing|ed)?)\b")
        .expect("constant browser-pass regex")
});
static BROWSER_ZERO_FAILURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:fail(?:ed|ures?)?\s*(?:=|:)?\s*0|0\s+(?:fail(?:ed|ures?)?))\b")
        .expect("constant browser-failure regex")
});
const GENERIC_PURPOSES: [&str; 13] = [
    "component",
    "config",
    "config file",
    "file",
    "fixture source",
    "message catalog",
    "misc",
    "script",
    "source",
    "source file",
    "test",
    "test file",
    "utility",
];
const GENERIC_VERIFICATION: [&str; 5] = [
    "manual source tracing is bound to manifest",
    "manifest sha-256",
    "source-defined output or side effect",
    "owned source logic",
    "fixture report",
];
static STRONG_CLAIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:persist|durab|database|transaction|read[- ]?back|save|stor|writ|integration|external|side[- ]effect|gateway|webhook|enqueue|publish|emit|send|sent|upload|success|succeed)")
        .expect("constant strong-claim regex")
});
static PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:#|//|/\*|<!--)\s*(?:TODO|FIXME|XXX)\b|\b(?:todo|unimplemented)!\s*\(|\b(?:NotImplementedError|not implemented|placeholder|stub)\b"#)
        .expect("constant placeholder regex")
});
static EMPTY_HANDLER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)on(?:Click|Submit|Change)\s*=\s*\{?\s*(?:\(\s*\)\s*=>\s*)?\{\s*\}\s*\}?")
        .expect("constant empty-handler regex")
});
static DEAD_HREF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)href\s*=\s*[\"'](?:#|javascript:void\(0\))[\"']"#)
        .expect("constant dead-href regex")
});
static BUTTON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<button\b([^>]*)>(.*?)</button>").expect("constant button regex")
});
static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]+>").expect("constant tag regex"));
static VISIBLE_TEXT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r">\s*([^<>{}\n][^<>{}]{1,100}?)\s*<").expect("constant visible-text regex")
});
static VISIBLE_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ix)\b(?:aria-label|title|placeholder|alt|label|content|android:text|text)\s*=\s*(?:\"([^\"]{2,100})\"|'([^']{2,100})')"#)
        .expect("constant visible-attribute regex")
});
static MARKDOWN_H1_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#\s+(.{2,100})\s*$").expect("constant H1 regex"));
static KEY_VALUE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*[-A-Za-z0-9_.]+\s*[:=]\s*['\"]?([^'\"\n#{}\[\]]{2,100})['\"]?\s*$"#)
        .expect("constant key-value regex")
});

#[derive(Clone, Debug)]
pub struct VerifyOptions {
    pub manifest: PathBuf,
    pub reports: Vec<PathBuf>,
    pub batch_id: Option<String>,
    pub skip_current_hash_check: bool,
    pub receipt_out: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct VerificationRun {
    pub result: Value,
    pub receipt: Option<Value>,
}

#[derive(Clone, Debug)]
struct SourceFile {
    rel_path: String,
    sha256: Option<String>,
    interface_relevant: bool,
}

#[derive(Clone, Debug)]
struct CoverageUnit {
    unit_id: String,
    rel_path: String,
    sha256: Option<String>,
    start_line: Option<usize>,
    end_line: Option<usize>,
    start_byte: Option<usize>,
    end_byte: Option<usize>,
}

#[derive(Clone, Debug)]
struct Batch {
    id: String,
    files: Vec<String>,
    units: Vec<String>,
    report: String,
}

#[derive(Clone, Debug)]
struct ManifestContext {
    raw: Map<String, Value>,
    path: PathBuf,
    root: PathBuf,
    repo_root: PathBuf,
    reports_root: PathBuf,
    run_id: String,
    sources: Vec<SourceFile>,
    units: Vec<CoverageUnit>,
    batches: Vec<Batch>,
    authorized_reports: Vec<String>,
    journey_required: bool,
}

#[derive(Clone, Debug)]
struct CoverageRow {
    file: String,
    status: String,
    sha256: String,
    purpose: String,
}

#[derive(Clone, Debug)]
struct ImplementationRow {
    file: String,
    contract_id: String,
    contract: String,
    anchors: String,
    implementation_trace: String,
    failure_trace: String,
    verification: String,
    result: String,
}

#[derive(Clone, Debug)]
struct InterfaceRow {
    file: String,
    visible_text: String,
    expected_path: String,
    implementation_notes: String,
}

#[derive(Clone, Debug)]
struct FindingBlock {
    heading: String,
    fields: BTreeMap<String, String>,
    text: String,
}

#[derive(Clone, Debug)]
struct ParsedReport {
    path: PathBuf,
    batch_id: Option<String>,
    filename_batch_id: Option<String>,
    declared_batch_ids: Vec<String>,
    run_ids: Vec<String>,
    coverage: Vec<CoverageRow>,
    malformed_coverage: Vec<String>,
    implementation: Vec<ImplementationRow>,
    malformed_implementation: Vec<String>,
    interface: Vec<InterfaceRow>,
    malformed_interface: Vec<String>,
    findings: Vec<FindingBlock>,
    findings_body: String,
    interface_body: String,
    missing_sections: Vec<String>,
    section_order: Vec<String>,
    narrative_issues: Vec<Value>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

fn plain(value: &str) -> String {
    value.trim().trim_matches('`').trim().to_owned()
}

fn normalized(value: &str) -> String {
    value
        .trim()
        .trim_matches('`')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_no_findings(value: &str) -> bool {
    matches!(
        value
            .chars()
            .map(|character| if character == '.' { ' ' } else { character })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            .as_str(),
        "no findings" | "no confirmed findings" | "none"
    )
}

fn backticks(value: &str) -> BTreeSet<String> {
    BACKTICK_RE
        .captures_iter(value)
        .map(|capture| plain(&capture[1]))
        .collect()
}

fn without_backticks(value: &str) -> String {
    BACKTICK_RE.replace_all(value, "").into_owned()
}

fn duplicates(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for value in values {
        if !seen.insert(value.clone()) {
            duplicates.insert(value);
        }
    }
    duplicates.into_iter().collect()
}

fn as_nonempty_string(value: Option<&Value>, label: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{label} must be a non-empty string."))
}

fn count_matches(raw: &Map<String, Value>, name: &str, actual: usize) -> Result<(), String> {
    if raw.get(name).and_then(Value::as_u64) == Some(actual as u64) {
        Ok(())
    } else {
        Err(format!(
            "Manifest field {name} must equal the actual count {actual}."
        ))
    }
}

fn range_value(object: &Map<String, Value>, name: &str) -> Result<Option<usize>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .and_then(|value| usize::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| format!("{name} must be a positive integer when present")),
    }
}

fn load_manifest(path: &Path) -> Result<(ManifestContext, Vec<u8>), String> {
    let bytes = read_bytes_nofollow(path, None)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Manifest does not exist: {}", path.display()))?;
    let raw = strict_json_object(&bytes, "Manifest")?;
    let root = path
        .parent()
        .ok_or_else(|| "Manifest has no parent directory".to_owned())?;
    let root = validate_directory_nofollow(root).map_err(|error| error.to_string())?;
    let manifest_path = root.join(
        path.file_name()
            .ok_or_else(|| "Manifest must name a file".to_owned())?,
    );
    let repo_root_text = as_nonempty_string(raw.get("repo_root"), "Manifest repo_root")?;
    let repo_root = validate_directory_nofollow(Path::new(&repo_root_text))
        .map_err(|_| "Manifest repo_root is missing or not a directory".to_owned())?;
    let run_id = as_nonempty_string(raw.get("run_id"), "Manifest run_id")?;
    let source_values = raw
        .get("source_files")
        .and_then(Value::as_array)
        .ok_or_else(|| "Manifest field source_files must be a list.".to_owned())?;
    let mut sources = Vec::new();
    for (index, value) in source_values.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or_else(|| format!("Manifest source_files[{index}] must be an object."))?;
        let rel_path = as_nonempty_string(
            object.get("rel_path"),
            &format!("Manifest source_files[{index}].rel_path"),
        )?;
        validate_repo_relative_path_token(&rel_path, &format!("source_files[{index}].rel_path"))?;
        let sha256 = match object.get("sha256") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|value| SHA256_RE.is_match(value))
                    .ok_or_else(|| {
                        format!(
                            "Manifest source_files[{index}].sha256 must be a 64-character SHA-256 hex digest."
                        )
                    })?
                    .to_lowercase(),
            ),
        };
        sources.push(SourceFile {
            rel_path,
            sha256,
            interface_relevant: object.get("interface_relevant") == Some(&json!(true)),
        });
    }
    let duplicate_sources = duplicates(sources.iter().map(|source| source.rel_path.clone()));
    if !duplicate_sources.is_empty() {
        return Err(format!(
            "Manifest source_files rel_path values must be unique; duplicates: {duplicate_sources:?}"
        ));
    }
    let source_names = sources
        .iter()
        .map(|source| source.rel_path.as_str())
        .collect::<BTreeSet<_>>();
    let unit_values = raw
        .get("coverage_units")
        .and_then(Value::as_array)
        .ok_or_else(|| "Manifest field coverage_units must be a list when present.".to_owned())?;
    let mut units = Vec::new();
    for (index, value) in unit_values.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or_else(|| format!("Manifest coverage_units[{index}] must be an object."))?;
        let unit_id = as_nonempty_string(
            object.get("unit_id"),
            &format!("Manifest coverage_units[{index}].unit_id"),
        )?;
        crate::audit_queue::validate_markdown_safe_token(
            &unit_id,
            &format!("coverage_units[{index}].unit_id"),
        )?;
        let rel_path = as_nonempty_string(
            object.get("rel_path"),
            &format!("Manifest coverage_units[{index}].rel_path"),
        )?;
        validate_repo_relative_path_token(&rel_path, &format!("coverage_units[{index}].rel_path"))?;
        if !source_names.contains(rel_path.as_str()) {
            return Err(format!(
                "Manifest coverage_units[{index}].rel_path is absent from source_files: {rel_path}"
            ));
        }
        let sha256 = match object.get("sha256") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|value| SHA256_RE.is_match(value))
                    .ok_or_else(|| {
                        format!(
                            "Manifest coverage_units[{index}].sha256 must be a 64-character SHA-256 hex digest."
                        )
                    })?
                    .to_lowercase(),
            ),
        };
        let start_line = range_value(object, "start_line")?;
        let end_line = range_value(object, "end_line")?;
        let start_byte = range_value(object, "start_byte")?;
        let end_byte = range_value(object, "end_byte")?;
        if (start_line.is_some() || end_line.is_some())
            && (start_byte.is_some() || end_byte.is_some())
        {
            return Err(format!(
                "Manifest coverage_units[{index}] must not mix line and byte ranges."
            ));
        }
        if start_line.is_some() != end_line.is_some()
            || start_line
                .zip(end_line)
                .is_some_and(|(start, end)| end < start)
        {
            return Err(format!(
                "Manifest coverage_units[{index}] line range must use positive start_line/end_line integers."
            ));
        }
        if start_byte.is_some() != end_byte.is_some()
            || start_byte
                .zip(end_byte)
                .is_some_and(|(start, end)| end < start)
        {
            return Err(format!(
                "Manifest coverage_units[{index}] byte range must use positive start_byte/end_byte integers."
            ));
        }
        units.push(CoverageUnit {
            unit_id,
            rel_path,
            sha256,
            start_line,
            end_line,
            start_byte,
            end_byte,
        });
    }
    let duplicate_units = duplicates(units.iter().map(|unit| unit.unit_id.clone()));
    if !duplicate_units.is_empty() {
        return Err(format!(
            "Manifest coverage_units unit_id values must be unique; duplicates: {duplicate_units:?}"
        ));
    }
    let unit_names = units
        .iter()
        .map(|unit| unit.unit_id.as_str())
        .collect::<BTreeSet<_>>();
    let batch_values = raw
        .get("batches")
        .and_then(Value::as_array)
        .ok_or_else(|| "Manifest field batches must be a list.".to_owned())?;
    let mut batches = Vec::new();
    let mut assigned_units = Vec::new();
    let mut assigned_files = BTreeSet::new();
    for (index, value) in batch_values.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or_else(|| format!("Manifest batches[{index}] must be an object."))?;
        let id = as_nonempty_string(object.get("id"), &format!("Manifest batches[{index}].id"))?;
        if !BATCH_ID_RE
            .find(&id)
            .is_some_and(|capture| capture.as_str().eq_ignore_ascii_case(&id))
        {
            return Err(format!("Manifest batches[{index}].id is invalid: {id}"));
        }
        let files = object
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("Manifest batches[{index}].files must be a list of strings."))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("Manifest batches[{index}].files must be strings."))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !duplicates(files.clone()).is_empty() {
            return Err(format!(
                "Manifest batches[{index}].files must not contain duplicates"
            ));
        }
        for file in &files {
            validate_repo_relative_path_token(file, &format!("batches[{index}].files"))?;
            if !source_names.contains(file.as_str()) {
                return Err(format!(
                    "Manifest batches reference file absent from source_files: {file}"
                ));
            }
            assigned_files.insert(file.clone());
        }
        let batch_units = object
            .get("coverage_units")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                format!("Manifest batches[{index}].coverage_units must be a list of strings")
            })?
            .iter()
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    format!("Manifest batches[{index}].coverage_units must be strings")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !duplicates(batch_units.clone()).is_empty() {
            return Err(format!(
                "Manifest batches[{index}].coverage_units must not contain duplicates"
            ));
        }
        for unit in &batch_units {
            if !unit_names.contains(unit.as_str()) {
                return Err(format!(
                    "Manifest batches reference coverage unit absent from coverage_units: {unit}"
                ));
            }
        }
        assigned_units.extend(batch_units.iter().cloned());
        let report = object
            .get("report")
            .and_then(Value::as_str)
            .unwrap_or(&format!("reports/{id}.md"))
            .to_owned();
        if report != format!("reports/{id}.md") {
            return Err(format!("Manifest batch {id} report path is not exact"));
        }
        batches.push(Batch {
            id,
            files,
            units: batch_units,
            report,
        });
    }
    let duplicate_batches = duplicates(batches.iter().map(|batch| batch.id.clone()));
    if !duplicate_batches.is_empty() {
        return Err(format!(
            "Manifest batch ids must be unique: {duplicate_batches:?}"
        ));
    }
    if assigned_files
        != source_names
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    {
        return Err("Manifest source_files are not assigned to batches exactly".to_owned());
    }
    let assigned_unit_set = assigned_units
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if assigned_unit_set != unit_names || !duplicates(assigned_units).is_empty() {
        return Err("Manifest coverage units are not assigned to batches exactly once".to_owned());
    }
    let lead = raw
        .get("lead_reconciliation")
        .and_then(Value::as_object)
        .ok_or_else(|| "Manifest field lead_reconciliation must be an object.".to_owned())?;
    if lead
        != json!({
            "required":true,"worker":"lead_reconciliation","prompt":"lead_reconciliation.md",
            "report":"reports/lead_reconciliation.md"
        })
        .as_object()
        .expect("object")
    {
        return Err(
            "Manifest field lead_reconciliation must exactly declare the required lead prompt/report contract."
                .to_owned(),
        );
    }
    let journey_required = raw
        .get("journey_audit")
        .and_then(Value::as_object)
        .and_then(|journey| journey.get("required"))
        == Some(&json!(true));
    count_matches(&raw, "source_file_count", sources.len())?;
    count_matches(&raw, "coverage_unit_count", units.len())?;
    count_matches(&raw, "batch_count", batches.len())?;
    count_matches(
        &raw,
        "interface_file_count",
        sources
            .iter()
            .filter(|source| source.interface_relevant)
            .count(),
    )?;
    for (name, field) in [
        ("scope_warning_count", "scope_warnings"),
        (
            "pruned_directory_review_hint_count",
            "pruned_directory_review_hints",
        ),
        ("tracked_deletion_count", "tracked_deletions"),
    ] {
        let values = raw
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("Manifest field {field} must be a list when present."))?;
        count_matches(&raw, name, values.len())?;
    }
    let reports_root = validate_directory_nofollow(&root.join("reports")).map_err(|error| {
        format!("Manifest-owned reports directory is missing or unsafe: {error}")
    })?;
    if raw.get("reports_dir").and_then(Value::as_str)
        != Some(reports_root.to_string_lossy().as_ref())
    {
        return Err(
            "Manifest reports_dir must resolve to its exact manifest-owned reports directory."
                .to_owned(),
        );
    }
    let authorized_reports = manifest_report_names(&raw)?;
    Ok((
        ManifestContext {
            raw,
            path: manifest_path,
            root,
            repo_root,
            reports_root,
            run_id,
            sources,
            units,
            batches,
            authorized_reports,
            journey_required,
        },
        bytes,
    ))
}

fn section_values(text: &str, name: &str) -> Vec<String> {
    section_bodies(text)
        .get(name)
        .map(|body| {
            body.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| line.trim_matches('`').to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn declared_batch_ids(text: &str) -> Vec<String> {
    section_values(text, "batch id")
        .into_iter()
        .filter_map(|value| {
            BATCH_ID_RE
                .captures(&value)
                .map(|captures| format!("batch_{}", &captures[1]).to_lowercase())
        })
        .collect()
}

fn filename_batch_id(path: &Path) -> Option<String> {
    BATCH_ID_RE
        .captures(path.file_name()?.to_str()?)
        .map(|captures| format!("batch_{}", &captures[1]).to_lowercase())
}

fn parse_finding_blocks(body: &str) -> Vec<FindingBlock> {
    let lines = body.lines().collect::<Vec<_>>();
    let indexes = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| FINDING_HEADING_RE.is_match(line.trim()).then_some(index))
        .collect::<Vec<_>>();
    let mut blocks = Vec::new();
    for (position, start) in indexes.iter().copied().enumerate() {
        let end = indexes.get(position + 1).copied().unwrap_or(lines.len());
        let mut fields = BTreeMap::new();
        for line in &lines[start + 1..end] {
            if let Some(captures) = FINDING_FIELD_RE.captures(line.trim()) {
                fields.insert(
                    captures[1].trim().to_lowercase(),
                    captures[2].trim().to_owned(),
                );
            }
        }
        blocks.push(FindingBlock {
            heading: lines[start].trim().to_owned(),
            fields,
            text: lines[start..end].join("\n").trim().to_owned(),
        });
    }
    blocks
}

fn findings_schema_issues(body: &str) -> Vec<Value> {
    let stripped = body.trim();
    if stripped.is_empty() || is_no_findings(stripped) {
        return Vec::new();
    }
    let malformed = body
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("###") && !FINDING_HEADING_RE.is_match(line))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let blocks = parse_finding_blocks(body);
    let mut issues = Vec::new();
    if !malformed.is_empty() {
        issues.push(json!({
            "section":"findings","reason":"finding headings must match '### P0/P1/P2/P3 - Short title'",
            "headings":malformed,
        }));
    }
    if blocks.is_empty() {
        issues.push(json!({
            "section":"findings","reason":"findings must use severity subsections or an explicit no-findings sentinel",
        }));
        return issues;
    }
    for block in blocks {
        let missing = REQUIRED_FINDING_FIELDS
            .iter()
            .filter(|field| !block.fields.contains_key(**field))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            issues.push(json!({
                "section":"findings","heading":block.heading,
                "reason":"finding is missing required fields","missing":missing,
            }));
        }
        for field in REQUIRED_FINDING_FIELDS {
            let value = block.fields.get(field).map(String::as_str).unwrap_or("");
            if field == "interface evidence" {
                if plain(value).len() < 4 || normalized(value) == "todo" {
                    issues.push(json!({
                        "section":"findings","heading":block.heading,"field":field,
                        "reason":"Interface findings must include concrete source-visible evidence or Not applicable",
                    }));
                }
            } else if field != "files" && (plain(value).len() < 12 || boilerplate(value)) {
                issues.push(json!({
                    "section":"findings","heading":block.heading,"field":field,
                    "reason":"finding fields require meaningful non-boilerplate evidence",
                    "actual":value,
                }));
            }
        }
    }
    issues
}

fn table_lines(text: &str, heading: &str) -> Vec<(String, Vec<String>)> {
    let mut inside = false;
    let mut rows = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.eq_ignore_ascii_case(&format!("## {heading}")) {
            inside = true;
            continue;
        }
        if inside && stripped.starts_with("## ") {
            break;
        }
        if inside && stripped.starts_with('|') {
            rows.push((stripped.to_owned(), split_markdown_row(stripped)));
        }
    }
    rows
}

fn parse_implementation(text: &str) -> (Vec<ImplementationRow>, Vec<String>) {
    let rows = table_lines(text, "Implementation Inventory");
    let mut malformed = Vec::new();
    if rows.first().map(|(_, columns)| {
        columns
            .iter()
            .map(|column| column.to_lowercase())
            .collect::<Vec<_>>()
    }) != Some(IMPLEMENTATION_HEADERS.map(str::to_owned).to_vec())
    {
        malformed
            .push("missing or invalid exact 8-column implementation inventory header".to_owned());
    }
    if rows.get(1).is_none_or(|(_, columns)| {
        columns.len() != IMPLEMENTATION_HEADERS.len() || !is_separator_row(columns)
    }) {
        malformed.push(
            "missing or invalid exact 8-column implementation inventory separator immediately after the header"
                .to_owned(),
        );
    }
    let mut parsed = Vec::new();
    for (raw, columns) in rows.into_iter().skip(2) {
        if columns.len() != IMPLEMENTATION_HEADERS.len()
            || columns.iter().any(|column| column.is_empty())
            || is_separator_row(&columns)
        {
            malformed.push(raw);
            continue;
        }
        parsed.push(ImplementationRow {
            file: plain(&columns[0]),
            contract_id: plain(&columns[1]),
            contract: columns[2].clone(),
            anchors: columns[3].clone(),
            implementation_trace: columns[4].clone(),
            failure_trace: columns[5].clone(),
            verification: columns[6].clone(),
            result: columns[7].trim().to_uppercase(),
        });
    }
    (parsed, malformed)
}

fn parse_interface(text: &str) -> (Vec<InterfaceRow>, Vec<String>) {
    let rows = table_lines(text, "Interface Inventory");
    let mut malformed = Vec::new();
    let mut parsed = Vec::new();
    for (index, (raw, columns)) in rows.into_iter().enumerate() {
        if index == 0
            && columns
                .iter()
                .map(|column| column.to_lowercase())
                .collect::<Vec<_>>()
                == INTERFACE_HEADERS.map(str::to_owned).to_vec()
        {
            continue;
        }
        if is_separator_row(&columns) {
            continue;
        }
        if columns.len() != INTERFACE_HEADERS.len()
            || columns.iter().any(|column| column.is_empty())
        {
            malformed.push(raw);
            continue;
        }
        parsed.push(InterfaceRow {
            file: plain(&columns[0]),
            visible_text: columns[2].clone(),
            expected_path: columns[3].clone(),
            implementation_notes: columns[4].clone(),
        });
    }
    (parsed, malformed)
}

fn purpose_issues(rows: &[CoverageRow]) -> Vec<Value> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(normalized(&row.purpose)).or_insert(0usize) += 1;
    }
    let mut issues = Vec::new();
    for row in rows {
        let purpose = normalized(&row.purpose);
        if DIRECTORY_PURPOSE_RE.is_match(&row.purpose) {
            issues.push(json!({
                "section":"file coverage","file":row.file,
                "reason":"file coverage purpose must describe the file role, not only a directory or group",
                "actual":row.purpose,
            }));
        } else if GENERIC_PURPOSES.contains(&purpose.as_str()) {
            issues.push(json!({
                "section":"file coverage","file":row.file,
                "reason":"file coverage purpose is too generic","actual":row.purpose,
            }));
        } else if counts.get(&purpose).copied().unwrap_or(0) >= 3 {
            let file_tokens = Path::new(row.file.split('#').next().unwrap_or(&row.file))
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|value| value.len() >= 2)
                .map(str::to_lowercase)
                .collect::<BTreeSet<_>>();
            let purpose_words = purpose
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            if file_tokens.is_disjoint(&purpose_words) {
                issues.push(json!({
                    "section":"file coverage","file":row.file,
                    "reason":"repeated file coverage purpose must include file-specific role detail",
                    "actual":row.purpose,
                }));
            }
        }
    }
    issues
}

fn parse_report(path: &Path, known_batches: &BTreeSet<String>) -> Result<ParsedReport, String> {
    let bytes = read_bytes_nofollow(path, None)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Report file could not be read: {}", path.display()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("Report file is not valid UTF-8: {}", path.display()))?;
    let bodies = section_bodies(&text);
    let ordered = section_order(&text);
    let run_ids = section_values(&text, "run id");
    let declared_batch_ids = declared_batch_ids(&text);
    let matching = declared_batch_ids
        .iter()
        .filter(|batch| known_batches.contains(*batch))
        .cloned()
        .collect::<Vec<_>>();
    let batch_id = (matching.len() == 1).then(|| matching[0].clone());
    let (implementation, malformed_implementation) = parse_implementation(&text);
    let (interface, malformed_interface) = parse_interface(&text);
    let findings_body = bodies.get("findings").cloned().unwrap_or_default();
    let mut coverage = Vec::new();
    let mut malformed_coverage = Vec::new();
    for (raw, columns) in table_lines(&text, "File Coverage") {
        if columns
            .iter()
            .take(4)
            .map(|column| column.to_lowercase())
            .collect::<Vec<_>>()
            == ["file", "status", "sha-256", "purpose"]
            || is_separator_row(&columns)
        {
            continue;
        }
        if columns.len() != 4 || columns.iter().any(|column| column.is_empty()) {
            malformed_coverage.push(raw);
            continue;
        }
        let digest = plain(&columns[2]);
        if !SHA256_RE.is_match(&digest) {
            malformed_coverage.push(format!("{raw} (invalid SHA-256 digest)"));
            continue;
        }
        coverage.push(CoverageRow {
            file: plain(&columns[0]),
            status: columns[1].trim().to_uppercase(),
            sha256: digest.to_lowercase(),
            purpose: columns[3].clone(),
        });
    }
    let mut narrative_issues = findings_schema_issues(&findings_body);
    for section in [
        "batch summary",
        "findings",
        "no finding notes",
        "open questions",
    ] {
        if bodies.get(section).is_none_or(|body| body.is_empty()) {
            narrative_issues.push(json!({"section":section,"reason":"section body is empty"}));
        }
    }
    narrative_issues.extend(purpose_issues(&coverage));
    let evidence_text = format!(
        "{}\n{}",
        bodies.get("findings").map(String::as_str).unwrap_or(""),
        bodies
            .get("no finding notes")
            .map(String::as_str)
            .unwrap_or("")
    );
    let missing_evidence = coverage
        .iter()
        .filter(|row| row.status == "CHECKED" && !evidence_text.contains(&row.file))
        .map(|row| row.file.clone())
        .collect::<Vec<_>>();
    if !missing_evidence.is_empty() {
        narrative_issues.push(json!({
            "section":"findings/no finding notes",
            "reason":"checked files must be referenced in Findings or No Finding Notes",
            "files":missing_evidence,
        }));
    }
    Ok(ParsedReport {
        path: path.to_path_buf(),
        batch_id,
        filename_batch_id: filename_batch_id(path),
        declared_batch_ids,
        run_ids,
        coverage,
        malformed_coverage,
        implementation,
        malformed_implementation,
        interface,
        malformed_interface,
        findings: parse_finding_blocks(&findings_body),
        findings_body,
        interface_body: bodies
            .get("interface inventory")
            .cloned()
            .unwrap_or_default(),
        missing_sections: REQUIRED_SECTIONS
            .iter()
            .filter(|section| !ordered.contains(&section.to_string()))
            .map(|section| (*section).to_owned())
            .collect(),
        section_order: ordered,
        narrative_issues,
    })
}

fn unit_bytes(context: &ManifestContext, unit: &CoverageUnit) -> Result<Vec<u8>, String> {
    let bytes = read_bytes_nofollow(
        &context.repo_root.join(&unit.rel_path),
        Some(&context.repo_root),
    )
    .map_err(|error| error.to_string())?
    .ok_or_else(|| format!("source file is missing: {}", unit.rel_path))?;
    if let (Some(start), Some(end)) = (unit.start_byte, unit.end_byte) {
        if end > bytes.len() {
            return Err(format!("byte range exceeds source: {}", unit.unit_id));
        }
        return Ok(bytes[start - 1..end].to_vec());
    }
    if let (Some(start), Some(end)) = (unit.start_line, unit.end_line) {
        let lines = bytes
            .split_inclusive(|byte| *byte == b'\n')
            .collect::<Vec<_>>();
        if end > lines.len() {
            return Err(format!("line range exceeds source: {}", unit.unit_id));
        }
        return Ok(lines[start - 1..end].concat());
    }
    Ok(bytes)
}

fn unit_text(context: &ManifestContext, unit: &CoverageUnit) -> String {
    String::from_utf8_lossy(&unit_bytes(context, unit).unwrap_or_default()).into_owned()
}

fn definition_patterns(path: &str) -> Vec<Regex> {
    let suffix = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_lowercase()))
        .unwrap_or_default();
    let patterns: &[&str] = match suffix.as_str() {
        ".py" => &[
            r"(?m)^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)\s*\(",
            r"(?m)^\s*class\s+([A-Za-z_]\w*)\b",
        ],
        ".js" | ".jsx" | ".mjs" | ".cjs" | ".ts" | ".tsx" | ".vue" | ".svelte" => &[
            r"(?m)^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(",
            r"(?m)^\s*(?:export\s+)?(?:default\s+)?class\s+([A-Za-z_$][\w$]*)\b",
            r"(?m)^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s+)?(?:\([^\n]*\)|[A-Za-z_$][\w$]*)\s*=>",
        ],
        ".go" => &[r"(?m)^\s*func\s+(?:\([^\n)]*\)\s*)?([A-Za-z_]\w*)\s*\("],
        ".rs" => &[
            r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_]\w*)\s*\(",
            r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait)\s+([A-Za-z_]\w*)\b",
        ],
        ".swift" => &[
            r"(?m)^\s*(?:public\s+|private\s+|internal\s+|fileprivate\s+|open\s+)*(?:static\s+|class\s+)?func\s+([A-Za-z_]\w*)\s*\(",
            r"(?m)^\s*(?:public\s+|private\s+|internal\s+|fileprivate\s+|open\s+)*(?:class|struct|enum|protocol)\s+([A-Za-z_]\w*)\b",
        ],
        ".rb" => &[
            r"(?m)^\s*def\s+(?:self\.)?([A-Za-z_]\w*[!?=]?)",
            r"(?m)^\s*(?:class|module)\s+([A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)",
        ],
        ".sh" | ".bash" | ".zsh" => {
            &[r"(?m)^\s*(?:function\s+)?([A-Za-z_]\w*)\s*(?:\(\s*\))?\s*\{"]
        }
        ".java" | ".kt" | ".kts" | ".cs" | ".c" | ".h" | ".cc" | ".cpp" | ".cxx" | ".hpp"
        | ".hh" | ".dart" => &[
            r"(?m)^\s*(?:(?:public|private|protected|internal|static|final|abstract|sealed|partial|open|data|base|interface)\s+)*(?:class|interface|struct|record|enum|trait|mixin|extension)\s+([A-Za-z_]\w*)\b",
            r"(?m)^\s*(?:(?:public|private|protected|internal|static|virtual|override|abstract|async|final|suspend|inline|extern)\s+)*(?:fun\s+)?(?:[A-Za-z_][\w<>,.?\[\]\s*&:]*\s+)+([A-Za-z_]\w*)\s*\([^;{}]*\)\s*(?:const\s*)?\{",
        ],
        ".php" => &[
            r"(?mi)^\s*(?:(?:final|abstract|readonly)\s+)*(?:class|interface|trait|enum)\s+([A-Za-z_]\w*)\b",
            r"(?mi)^\s*(?:(?:public|private|protected|static|final|abstract)\s+)*function\s+&?\s*([A-Za-z_]\w*)\s*\(",
        ],
        ".lua" => &[r"(?m)^\s*(?:local\s+)?function\s+([A-Za-z_]\w*(?:[.:][A-Za-z_]\w*)*)\s*\("],
        ".ex" | ".exs" => &[r"(?m)^\s*defp?\s+([a-z_]\w*[!?]?)\s*(?:\(|do\b)"],
        ".sql" => &[
            r#"(?im)^\s*CREATE\s+(?:OR\s+(?:REPLACE|ALTER)\s+)?(?:FUNCTION|PROCEDURE|TRIGGER|(?:MATERIALIZED\s+)?VIEW)\s+(?:IF\s+NOT\s+EXISTS\s+)?((?:\"?[A-Za-z_]\w*\"?\.)?\"?[A-Za-z_]\w*\"?)"#,
        ],
        _ => &[],
    };
    patterns
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
}

fn definition_anchors(unit: &CoverageUnit, text: &str) -> Vec<String> {
    let control = [
        "catch", "do", "else", "finally", "for", "if", "return", "switch", "try", "while", "with",
    ];
    let mut occurrences = Vec::new();
    let mut seen = BTreeSet::new();
    for pattern in definition_patterns(&unit.rel_path) {
        for captures in pattern.captures_iter(text) {
            let Some(value) = captures.get(1) else {
                continue;
            };
            if control.contains(&value.as_str().to_lowercase().as_str())
                || !seen.insert((value.as_str().to_owned(), value.start()))
            {
                continue;
            }
            let anchor = if let Some(start_byte) = unit.start_byte {
                let relative_bytes = text[..value.start()].len();
                format!("{}@B{}", value.as_str(), start_byte + relative_bytes)
            } else {
                let relative_line = text[..value.start()]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1;
                let line = unit.start_line.unwrap_or(1) + relative_line - 1;
                let line_start = text[..value.start()]
                    .rfind('\n')
                    .map_or(0, |index| index + 1);
                let column = text[line_start..value.start()].chars().count() + 1;
                format!("{}@L{line}:C{column}", value.as_str())
            };
            occurrences.push((value.start(), anchor));
        }
    }
    occurrences.sort_by_key(|(start, anchor)| (*start, anchor.clone()));
    occurrences.into_iter().map(|(_, anchor)| anchor).collect()
}

fn trace_status(value: &str) -> Option<(&str, &str)> {
    let captures = TRACE_STATUS_RE.captures(value)?;
    Some((captures.get(1)?.as_str(), captures.get(2)?.as_str()))
}

fn finding_contract_ids(block: &FindingBlock, lead: bool) -> Vec<String> {
    backticks(&block.text)
        .into_iter()
        .filter(|value| {
            if lead {
                LEAD_CONTRACT_RE.is_match(value)
            } else {
                BATCH_CONTRACT_RE.is_match(value)
            }
        })
        .collect()
}

fn typed_evidence_issues(
    verification: &str,
    result: &str,
    sources: &BTreeSet<String>,
    valid_evidence: &BTreeSet<String>,
    context: &str,
) -> Vec<Value> {
    let types = EVIDENCE_TYPE_RE
        .captures_iter(verification)
        .filter_map(|captures| captures.get(1))
        .map(|value| value.as_str().to_lowercase())
        .collect::<Vec<_>>();
    let mut issues = Vec::new();
    if types.len() != 1 {
        issues.push(json!({
            "reason":format!("{context} requires exactly one evidence-type: test|runtime|source-only declaration"),
            "verification":verification,
        }));
        return issues;
    }
    let evidence_type = &types[0];
    let ref_matches = EVIDENCE_REF_RE
        .captures_iter(verification)
        .collect::<Vec<_>>();
    let refs = if ref_matches.len() == 1 {
        let detail = ref_matches[0]
            .get(1)
            .map(|value| value.as_str())
            .unwrap_or("");
        let unexplained = BACKTICK_RE
            .replace_all(detail, "")
            .trim_matches(|character: char| character == ',' || character.is_whitespace())
            .to_owned();
        if unexplained.is_empty() {
            backticks(detail)
        } else {
            BTreeSet::new()
        }
    } else {
        BTreeSet::new()
    };
    if matches!(evidence_type.as_str(), "test" | "runtime") && refs.is_empty() {
        issues.push(json!({
            "reason":format!("{context} evidence-type {evidence_type} requires exactly one evidence-ref declaration containing concrete backticked references"),
            "verification":verification,
        }));
    } else if evidence_type == "test" {
        let invalid = refs
            .iter()
            .filter(|path| !sources.contains(*path) || !is_test_source_path(path))
            .cloned()
            .collect::<Vec<_>>();
        if !invalid.is_empty() {
            issues.push(json!({
                "reason":format!("{context} test evidence-ref values must resolve to manifest-owned test source files"),
                "invalid_evidence_refs":invalid,
            }));
        }
    } else if evidence_type == "runtime" {
        let invalid = refs
            .iter()
            .filter_map(|value| {
                value
                    .strip_prefix("evidence:")
                    .map(str::to_owned)
                    .or_else(|| Some(value.clone()))
            })
            .filter(|value| !valid_evidence.contains(value))
            .collect::<Vec<_>>();
        if !invalid.is_empty() {
            issues.push(json!({
                "reason":format!("{context} runtime evidence-ref values must bind valid visual evidence ids"),
                "invalid_evidence_refs":invalid,
            }));
        }
    }
    if result == "PASS"
        && matches!(evidence_type.as_str(), "test" | "runtime")
        && (OUTCOME_RE.captures_iter(verification).count() != 1
            || OUTCOME_RE
                .captures(verification)
                .and_then(|captures| captures.get(1))
                .is_none_or(|value| plain(value.as_str()).len() < 12))
    {
        issues.push(json!({
            "reason":format!("{context} PASS test/runtime evidence requires one explicit outcome: or result:"),
            "verification":verification,
        }));
    }
    if EXPECTATION_RE.captures_iter(verification).count() != 1
        || EXPECTATION_RE
            .captures(verification)
            .and_then(|captures| captures.get(2))
            .is_none_or(|value| plain(value.as_str()).len() < 12)
    {
        issues.push(json!({
            "reason":format!("{context} requires exactly one concrete counterfactual: or invariance: statement"),
            "verification":verification,
        }));
    }
    if !BEHAVIOR_RE.is_match(verification) {
        issues.push(json!({
            "reason":format!("{context} must name a behavior-specific check"),
            "verification":verification,
        }));
    }
    if GENERIC_VERIFICATION
        .iter()
        .any(|phrase| normalized(verification).contains(phrase))
    {
        issues.push(json!({
            "reason":format!("{context} must be behavior-specific rather than generic manifest/source prose"),
            "verification":verification,
        }));
    }
    issues
}

fn placeholder_markers(text: &str) -> Vec<String> {
    fn quoted(line: &str, byte_index: usize) -> bool {
        let mut escaped = false;
        let mut counts = [0usize; 3];
        for (_, character) in line
            .char_indices()
            .take_while(|(index, _)| *index < byte_index)
        {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if let Some(index) = ['\'', '"', '`']
                .iter()
                .position(|quote| *quote == character)
            {
                counts[index] += 1;
            }
        }
        counts.into_iter().any(|count| count % 2 == 1)
    }
    let mut markers = BTreeSet::new();
    for line in text.lines() {
        for finding in PLACEHOLDER_RE.find_iter(line) {
            if !quoted(line, finding.start()) {
                markers.insert(line.trim().chars().take(160).collect::<String>());
            }
        }
    }
    markers.into_iter().collect()
}

fn control_markers(text: &str) -> Vec<String> {
    let mut markers = BTreeSet::new();
    for capture in EMPTY_HANDLER_RE.find_iter(text) {
        markers.insert(format!("empty handler `{}`", capture.as_str().trim()));
    }
    for capture in DEAD_HREF_RE.find_iter(text) {
        markers.insert(format!("dead link `{}`", capture.as_str().trim()));
    }
    for captures in BUTTON_RE.captures_iter(text) {
        let attributes = captures.get(1).map(|value| value.as_str()).unwrap_or("");
        let body = captures.get(2).map(|value| value.as_str()).unwrap_or("");
        let stripped_body = TAG_RE.replace_all(body, "").trim().to_owned();
        if stripped_body.is_empty()
            && !attributes.to_lowercase().contains("aria-label")
            && !attributes.to_lowercase().contains("title=")
        {
            markers.insert(format!(
                "unlabeled button `{}`",
                captures
                    .get(0)
                    .map(|value| value.as_str())
                    .unwrap_or("")
                    .trim()
            ));
        }
    }
    markers.into_iter().collect()
}

fn read_json_value(path: &Path, root: &Path, label: &str) -> Result<Value, String> {
    let bytes = read_bytes_nofollow(path, Some(root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{label} is missing: {}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{label} is not valid JSON: {error}"))
}

fn completion_marker_issues(context: &ManifestContext) -> Vec<Value> {
    let path = context.root.join("queue_complete.json");
    if context.root.join("audit_complete.json").is_file() {
        return vec![json!({
            "path":context.root.join("audit_complete.json"),
            "reason":"legacy audit_complete.json must not be used as a queue or verification marker",
        })];
    }
    let marker = match read_json_value(&path, &context.root, "queue_complete.json") {
        Ok(Value::Object(marker)) => marker,
        Ok(_) => {
            return vec![json!({"path":path,"reason":"queue_complete.json must be a JSON object"})];
        }
        Err(error) => return vec![json!({"path":path,"reason":error})],
    };
    let expected = [
        ("run_id", json!(context.run_id)),
        ("phase", json!("queue_generated")),
        ("audit_verified", json!(false)),
        ("batch_count", json!(context.batches.len())),
        ("source_file_count", json!(context.sources.len())),
        ("manifest", json!("manifest.json")),
        ("audit_index", json!("audit_index.md")),
        ("effort_ledger", json!("effort_ledger.json")),
        ("excluded_files", json!("excluded_files.json")),
        ("reports_dir", json!("reports")),
        ("ownership_marker", json!(".full-repo-audit-artifacts.json")),
        (
            "marker_semantics",
            json!(
                "Queue artifacts were generated; subagent reports and effort ledger still require verifier completion."
            ),
        ),
    ];
    expected
        .into_iter()
        .filter_map(|(field, expected)| {
            (marker.get(field) != Some(&expected)).then(|| {
                json!({
                    "path":path,"field":field,"expected":expected,
                    "actual":marker.get(field).cloned().unwrap_or(Value::Null),
                })
            })
        })
        .collect()
}

fn excluded_issues(context: &ManifestContext) -> (Vec<Value>, Vec<Value>) {
    let path = context.root.join("excluded_files.json");
    let excluded = match read_json_value(&path, &context.root, "excluded_files.json") {
        Ok(Value::Array(values)) => values,
        Ok(_) => {
            return (
                Vec::new(),
                vec![json!({"path":path,"reason":"excluded_files.json must be a JSON list"})],
            );
        }
        Err(error) => return (Vec::new(), vec![json!({"path":path,"reason":error})]),
    };
    let warnings = excluded
        .iter()
        .filter(|item| item.get("scope_warning") == Some(&json!(true)))
        .cloned()
        .collect::<Vec<_>>();
    let manifest_warnings = context
        .raw
        .get("scope_warnings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut issues = Vec::new();
    if context
        .raw
        .get("excluded_file_count")
        .and_then(Value::as_u64)
        != Some(excluded.len() as u64)
    {
        issues.push(json!({
            "path":context.path,"field":"excluded_file_count","expected":excluded.len(),
            "actual":context.raw.get("excluded_file_count").cloned().unwrap_or(Value::Null),
        }));
    }
    let digest = canonical_json_sha256(&Value::Array(excluded.clone())).unwrap_or_default();
    if context
        .raw
        .get("excluded_files_sha256")
        .and_then(Value::as_str)
        != Some(&digest)
    {
        issues.push(json!({
            "path":path,"field":"excluded_files_sha256",
            "expected":context.raw.get("excluded_files_sha256").cloned().unwrap_or(Value::Null),
            "actual":digest,"reason":"excluded_files.json content differs from manifest digest",
        }));
    }
    let warning_identity = |values: &[Value]| {
        values
            .iter()
            .filter_map(Value::as_object)
            .map(|item| {
                (
                    item.get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    item.get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                )
            })
            .collect::<BTreeSet<_>>()
    };
    if warning_identity(&warnings) != warning_identity(&manifest_warnings) {
        issues.push(json!({
            "path":path,"reason":"scope warning rows differ between manifest and excluded_files.json",
        }));
    }
    (warnings, issues)
}

fn claim_issues(
    row: &Map<String, Value>,
    prefix: &str,
    basis_field: &str,
    label_field: &str,
) -> Vec<Value> {
    let basis = row.get(basis_field).and_then(Value::as_str);
    let allowed = [
        "runtime-attested",
        "tool-schema-inspected",
        "self-reported",
        "manual-fallback",
    ];
    if basis.is_none_or(|basis| !allowed.contains(&basis)) {
        return vec![json!({
            "field":format!("{prefix}.{basis_field}"),"expected":allowed,
            "actual":row.get(basis_field).cloned().unwrap_or(Value::Null),
        })];
    }
    let expected = match basis {
        Some("runtime-attested") => "runtime-attested",
        Some("manual-fallback") => "manual-fallback",
        _ => "ledger-recorded-unverified",
    };
    if row.get(label_field).and_then(Value::as_str) != Some(expected) {
        vec![json!({
            "field":format!("{prefix}.{label_field}"),"expected":expected,
            "actual":row.get(label_field).cloned().unwrap_or(Value::Null),
        })]
    } else {
        Vec::new()
    }
}

fn effort_ledger_issues(context: &ManifestContext) -> Vec<Value> {
    let path = context.root.join("effort_ledger.json");
    let ledger = match read_json_value(&path, &context.root, "effort_ledger.json") {
        Ok(Value::Object(value)) => value,
        Ok(_) => return vec![json!({"path":path,"reason":"effort_ledger.json must be an object"})],
        Err(error) => return vec![json!({"path":path,"reason":error})],
    };
    let mut issues = Vec::new();
    for (field, expected) in [
        ("run_id", json!(context.run_id)),
        ("repo_root", json!(context.repo_root.to_string_lossy())),
    ] {
        if ledger.get(field) != Some(&expected) {
            issues.push(json!({"field":field,"expected":expected,"actual":ledger.get(field)}));
        }
    }
    let fallback = ledger
        .get("fallback_mode")
        .and_then(Value::as_object)
        .and_then(|value| value.get("active"))
        .and_then(Value::as_bool);
    if fallback.is_none() {
        issues.push(json!({"field":"fallback_mode.active","reason":"must be boolean"}));
    }
    let fallback = fallback.unwrap_or(false);
    if fallback
        && ledger
            .get("fallback_mode")
            .and_then(Value::as_object)
            .and_then(|value| value.get("reason"))
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().len() < 2)
    {
        issues.push(
            json!({"field":"fallback_mode.reason","reason":"active fallback requires a reason"}),
        );
    }
    let capability = ledger
        .get("subagent_capability_check")
        .and_then(Value::as_object);
    if capability
        .and_then(|row| row.get("status"))
        .and_then(Value::as_str)
        != Some("completed")
    {
        issues.push(json!({"field":"subagent_capability_check.status","expected":"completed"}));
    }
    if let Some(capability) = capability {
        issues.extend(claim_issues(
            capability,
            "subagent_capability_check",
            "claim_basis",
            "claim_label",
        ));
        if capability
            .get("can_set_reasoning_effort")
            .and_then(Value::as_bool)
            != Some(!fallback)
        {
            issues.push(json!({
                "field":"subagent_capability_check.can_set_reasoning_effort","expected":!fallback,
            }));
        }
    }
    let lead = ledger.get("lead").and_then(Value::as_object);
    if lead
        .and_then(|row| row.get("status"))
        .and_then(Value::as_str)
        != Some("completed")
        || lead
            .and_then(|row| row.get("actual_reasoning_effort"))
            .and_then(Value::as_str)
            != Some("xhigh")
    {
        issues.push(json!({"field":"lead","reason":"lead must be completed at xhigh effort"}));
    }
    if let Some(lead) = lead {
        if lead
            .get("required_reasoning_effort")
            .and_then(Value::as_str)
            != Some("xhigh")
            || lead
                .get("agent_id")
                .and_then(Value::as_str)
                .is_none_or(|value| value.is_empty())
        {
            issues.push(json!({"field":"lead.required_reasoning_effort","expected":"xhigh with a lead agent id"}));
        }
        issues.extend(claim_issues(
            lead,
            "lead",
            "effort_claim_basis",
            "effort_claim_label",
        ));
    }
    if ledger
        .get("lead_reconciliation")
        .and_then(Value::as_object)
        .and_then(|row| row.get("status"))
        .and_then(Value::as_str)
        != Some("completed")
    {
        issues.push(json!({"field":"lead_reconciliation.status","expected":"completed"}));
    }
    let batch_rows = ledger
        .get("batches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    for batch in &context.batches {
        let matches = batch_rows
            .iter()
            .filter_map(Value::as_object)
            .filter(|row| row.get("batch_id").and_then(Value::as_str) == Some(&batch.id))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            issues.push(json!({"field":"batches","batch":batch.id,"reason":"requires exactly one ledger row"}));
            continue;
        }
        let row = matches[0];
        seen.insert(batch.id.clone());
        for (field, expected) in [
            ("status", json!("completed")),
            ("required_reasoning_effort", json!("low")),
            ("prompt", json!(format!("{}.md", batch.id))),
            ("report", json!(batch.report)),
        ] {
            if row.get(field) != Some(&expected) {
                issues.push(json!({"field":format!("batches.{}.{}",batch.id,field),"expected":expected,"actual":row.get(field)}));
            }
        }
        if fallback {
            if row.get("actual_reasoning_effort").and_then(Value::as_str) != Some("manual-fallback")
                || !row.get("agent_id").is_none_or(Value::is_null)
            {
                issues.push(json!({"field":format!("batches.{}",batch.id),"reason":"fallback batch identity is inconsistent"}));
            }
        } else if row.get("actual_reasoning_effort").and_then(Value::as_str) != Some("low")
            || row
                .get("agent_id")
                .and_then(Value::as_str)
                .is_none_or(|value| value.is_empty())
        {
            issues.push(json!({"field":format!("batches.{}",batch.id),"reason":"spawned batch identity/effort is incomplete"}));
        }
        issues.extend(claim_issues(
            row,
            &format!("batches.{}", batch.id),
            "effort_claim_basis",
            "effort_claim_label",
        ));
    }
    if seen.len() != batch_rows.len() {
        issues.push(
            json!({"field":"batches","reason":"ledger contains unknown or duplicate batch rows"}),
        );
    }
    for worker in ["journey_source_worker", "visual_journey_worker"] {
        let row = ledger.get(worker).and_then(Value::as_object);
        let expected_status = if context.journey_required {
            "completed"
        } else {
            "not-applicable"
        };
        if row
            .and_then(|row| row.get("status"))
            .and_then(Value::as_str)
            != Some(expected_status)
        {
            issues.push(json!({"field":format!("{worker}.status"),"expected":expected_status}));
        }
        if context.journey_required
            && let Some(row) = row
        {
            if row.get("required_reasoning_effort").and_then(Value::as_str) != Some("low")
                || row.get("actual_reasoning_effort").and_then(Value::as_str)
                    != Some(if fallback { "manual-fallback" } else { "low" })
                || (!fallback
                    && row
                        .get("agent_id")
                        .and_then(Value::as_str)
                        .is_none_or(|value| value.is_empty()))
            {
                issues.push(
                    json!({"field":worker,"reason":"journey worker identity/effort is incomplete"}),
                );
            }
            issues.extend(claim_issues(
                row,
                worker,
                "effort_claim_basis",
                "effort_claim_label",
            ));
        }
    }
    for (field, count_field) in [
        (
            "pruned_directory_review",
            "pruned_directory_review_hint_count",
        ),
        ("tracked_deletion_review", "tracked_deletion_count"),
        ("lead_high_risk_review", "high_risk_file_count"),
    ] {
        let count = context
            .raw
            .get(count_field)
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let expected = if count > 0 {
            "completed"
        } else {
            "not-applicable"
        };
        if ledger
            .get(field)
            .and_then(Value::as_object)
            .and_then(|row| row.get("status"))
            .and_then(Value::as_str)
            != Some(expected)
        {
            issues.push(json!({"field":format!("{field}.status"),"expected":expected}));
        }
    }
    let review_decisions = |ledger_field: &str,
                            manifest_field: &str,
                            allowed: &[&str],
                            require_evidence: bool,
                            issues: &mut Vec<Value>| {
        let expected = context
            .raw
            .get(manifest_field)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if expected.is_empty() {
            return;
        }
        let decisions = ledger
            .get(ledger_field)
            .and_then(Value::as_object)
            .and_then(|row| row.get("decisions"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for expected_row in expected.iter().filter_map(Value::as_object) {
            let path = expected_row
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("");
            let matches = decisions
                .iter()
                .filter_map(Value::as_object)
                .filter(|row| row.get("path").and_then(Value::as_str) == Some(path))
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                issues.push(json!({"field":format!("{ledger_field}.decisions"),"path":path,"reason":"requires exactly one path decision"}));
                continue;
            }
            let row = matches[0];
            if row
                .get("decision")
                .and_then(Value::as_str)
                .is_none_or(|value| !allowed.contains(&value))
                || row
                    .get("rationale")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().len() < 12)
                || (require_evidence
                    && row
                        .get("evidence")
                        .and_then(Value::as_str)
                        .is_none_or(|value| value.trim().len() < 12))
            {
                issues.push(json!({"field":format!("{ledger_field}.decisions"),"path":path,"reason":"decision, rationale, or evidence is incomplete"}));
            }
        }
        if decisions.len() != expected.len() {
            issues.push(json!({"field":format!("{ledger_field}.decisions"),"reason":"decision count differs from manifest"}));
        }
    };
    review_decisions(
        "pruned_directory_review",
        "pruned_directory_review_hints",
        &[
            "excluded-with-rationale",
            "out-of-scope-with-user-confirmation",
            "requeued",
        ],
        false,
        &mut issues,
    );
    review_decisions(
        "tracked_deletion_review",
        "tracked_deletions",
        &["verified-removal", "finding-recorded", "blocked"],
        true,
        &mut issues,
    );
    let expected_high_risk = context
        .raw
        .get("high_risk_files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !expected_high_risk.is_empty() {
        let actual = ledger
            .get("lead_high_risk_review")
            .and_then(Value::as_object)
            .and_then(|row| row.get("files"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for expected in expected_high_risk.iter().filter_map(Value::as_object) {
            let path = expected
                .get("rel_path")
                .and_then(Value::as_str)
                .unwrap_or("");
            let matches = actual
                .iter()
                .filter_map(Value::as_object)
                .filter(|row| row.get("rel_path").and_then(Value::as_str) == Some(path))
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                issues.push(json!({"field":"lead_high_risk_review.files","path":path,"reason":"requires one exact row"}));
                continue;
            }
            let row = matches[0];
            if row.get("sha256") != expected.get("sha256")
                || row.get("risk_reasons") != expected.get("risk_reasons")
                || row.get("status").and_then(Value::as_str) != Some("completed")
                || row
                    .get("evidence")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().len() < 12)
                || row
                    .get("notes")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().len() < 12)
            {
                issues.push(json!({"field":"lead_high_risk_review","path":path,"reason":"status/hash/risk reasons/evidence must match and be complete"}));
            }
        }
        if actual.len() != expected_high_risk.len() {
            issues.push(json!({"field":"lead_high_risk_review.files","reason":"file count differs from manifest"}));
        }
    }
    issues
}

#[derive(Clone, Debug)]
struct BatchContract {
    source_file: String,
    result: String,
    anchors: BTreeSet<String>,
}

struct ImplementationValidation {
    issues: Vec<Value>,
    contracts: BTreeMap<String, BatchContract>,
    finding_schema: Vec<Value>,
    placeholder_omissions: Vec<Value>,
    control_omissions: Vec<Value>,
}

fn source_set(context: &ManifestContext) -> BTreeSet<String> {
    context
        .sources
        .iter()
        .map(|source| source.rel_path.clone())
        .collect()
}

fn valid_evidence_ids(
    context: &ManifestContext,
) -> (EvidenceRecords, BTreeSet<String>, Vec<Value>) {
    let (records, issues) =
        validate_visual_evidence_manifest(&context.root, &context.run_id, false);
    let invalid = issues
        .iter()
        .filter_map(|issue| issue.get("record"))
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let global = issues
        .iter()
        .any(|issue| issue.get("record").and_then(Value::as_str).is_none());
    let valid = if global {
        BTreeSet::new()
    } else {
        records
            .keys()
            .filter(|id| !invalid.contains(*id))
            .cloned()
            .collect()
    };
    (records, valid, issues)
}

fn implementation_issues(
    context: &ManifestContext,
    reports: &[ParsedReport],
    valid_evidence: &BTreeSet<String>,
) -> ImplementationValidation {
    let sources = source_set(context);
    let units = context
        .units
        .iter()
        .map(|unit| (unit.unit_id.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut contract_counts = BTreeMap::new();
    for row in reports.iter().flat_map(|report| &report.implementation) {
        *contract_counts
            .entry(row.contract_id.clone())
            .or_insert(0usize) += 1;
    }
    let mut grouped = Vec::new();
    let mut contracts = BTreeMap::new();
    let mut finding_schema = Vec::new();
    let mut placeholder_omissions = Vec::new();
    let mut control_omissions = Vec::new();
    for report in reports {
        let Some(batch_id) = report.batch_id.as_deref() else {
            continue;
        };
        let Some(batch) = context.batches.iter().find(|batch| batch.id == batch_id) else {
            continue;
        };
        let expected = batch
            .units
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let observed = report
            .implementation
            .iter()
            .map(|row| row.file.as_str())
            .collect::<BTreeSet<_>>();
        let mut issues = Vec::new();
        if !report.malformed_implementation.is_empty() {
            issues.push(json!({
                "reason":"malformed implementation inventory rows",
                "rows":report.malformed_implementation,
            }));
        }
        let missing = expected.difference(&observed).copied().collect::<Vec<_>>();
        let extra = observed.difference(&expected).copied().collect::<Vec<_>>();
        if !missing.is_empty() {
            issues.push(json!({"reason":"missing implementation inventory rows","units":missing}));
        }
        if !extra.is_empty() {
            issues.push(
                json!({"reason":"implementation inventory rows outside this batch","units":extra}),
            );
        }
        let gap_rows = report
            .implementation
            .iter()
            .filter(|row| {
                expected.contains(row.file.as_str())
                    && matches!(row.result.as_str(), "GAP" | "BLOCKED")
            })
            .map(|row| (row.contract_id.clone(), row))
            .collect::<BTreeMap<_, _>>();
        let mut findings_by_contract: BTreeMap<String, Vec<&FindingBlock>> = BTreeMap::new();
        for finding in &report.findings {
            let finding_files = finding
                .fields
                .get("files")
                .map(|value| backticks(value))
                .unwrap_or_default();
            let unknown_files = finding_files
                .iter()
                .filter(|file| !batch.files.contains(*file))
                .cloned()
                .collect::<Vec<_>>();
            if finding_files.is_empty() || !unknown_files.is_empty() {
                issues.push(json!({
                    "reason":"finding Files must cite manifest source files inside this batch",
                    "heading":finding.heading,"unknown_files":unknown_files,
                }));
            }
            let interface_evidence = finding
                .fields
                .get("interface evidence")
                .map(String::as_str)
                .unwrap_or("");
            if normalized(interface_evidence) != "not applicable"
                && finding_files.iter().any(|file| {
                    context
                        .sources
                        .iter()
                        .find(|source| source.rel_path == *file)
                        .is_some_and(|source| source.interface_relevant)
                })
                && !finding_files.iter().any(|file| {
                    read_bytes_nofollow(&context.repo_root.join(file), Some(&context.repo_root))
                        .ok()
                        .flatten()
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .is_some_and(|source| {
                            source_contains_visible_text(&source, interface_evidence)
                        })
                })
            {
                issues.push(json!({
                    "reason":"Interface findings must include source-visible evidence",
                    "heading":finding.heading,"actual":interface_evidence,
                }));
            }
            let ids = finding_contract_ids(finding, false);
            if ids.len() != 1 || !gap_rows.contains_key(&ids[0]) {
                issues.push(json!({
                    "reason":"each atomic batch finding must cite exactly one GAP or BLOCKED implementation Contract ID",
                    "heading":finding.heading,"contract_ids":ids,
                }));
            } else {
                findings_by_contract
                    .entry(ids[0].clone())
                    .or_default()
                    .push(finding);
            }
        }
        for row in &report.implementation {
            if !expected.contains(row.file.as_str()) {
                continue;
            }
            let Some(unit) = units.get(row.file.as_str()).copied() else {
                continue;
            };
            let unit_text = unit_text(context, unit);
            let definition_anchors = definition_anchors(unit, &unit_text);
            let id_batch = BATCH_CONTRACT_RE
                .captures(&row.contract_id)
                .and_then(|captures| captures.get(1))
                .map(|value| value.as_str());
            if id_batch != Some(batch_id) {
                issues.push(json!({
                    "reason":"implementation inventory Contract ID must use the owning batch_<3+ digits>:C<3+ digits> namespace",
                    "unit":row.file,"contract_id":row.contract_id,"expected_batch":batch_id,
                }));
            }
            if contract_counts.get(&row.contract_id).copied().unwrap_or(0) > 1 {
                issues.push(json!({
                    "reason":"implementation inventory Contract IDs must be unique",
                    "unit":row.file,"contract_id":row.contract_id,
                }));
            }
            if !RESULT_VALUES.contains(&row.result.as_str()) {
                issues.push(json!({
                    "reason":"implementation inventory Result must be PASS, GAP, or BLOCKED",
                    "unit":row.file,"contract_id":row.contract_id,"actual":row.result,
                }));
            }
            if boilerplate(&row.contract) || plain(&row.contract).len() < 12 {
                issues.push(json!({
                    "reason":"implementation responsibility must be meaningful non-boilerplate text",
                    "unit":row.file,"contract_id":row.contract_id,"actual":row.contract,
                }));
            }
            let basis = BASIS_RE.captures_iter(&row.contract).collect::<Vec<_>>();
            let discovery = DISCOVERY_RE
                .captures_iter(&row.contract)
                .collect::<Vec<_>>();
            if basis.len() != 1
                || !basis[0].get(1).is_some_and(|value| {
                    BASIS_KINDS.contains(&value.as_str().to_lowercase().as_str())
                })
            {
                issues.push(json!({
                    "reason":"implementation responsibility requires exactly one allowed Basis declaration",
                    "unit":row.file,"contract_id":row.contract_id,"actual":row.contract,
                }));
            }
            if let Some(basis) = basis.first() {
                let kind = basis
                    .get(1)
                    .map(|value| value.as_str().to_lowercase())
                    .unwrap_or_default();
                let references = basis
                    .get(2)
                    .map(|value| backticks(value.as_str()))
                    .unwrap_or_default();
                if references.is_empty()
                    || (kind != "source-inferred"
                        && references.iter().any(|reference| {
                            let source = reference.split('#').next().unwrap_or(reference);
                            !sources.contains(source)
                        }))
                {
                    issues.push(json!({
                        "reason":"authoritative implementation Basis references must resolve to manifest-owned source artifacts",
                        "unit":row.file,"contract_id":row.contract_id,"actual":row.contract,
                    }));
                }
            }
            if discovery.len() != 1 {
                issues.push(json!({
                    "reason":"implementation responsibility requires exactly one Discovery: parsed|manual declaration",
                    "unit":row.file,"contract_id":row.contract_id,"actual":row.contract,
                }));
            }
            let anchor_refs = backticks(&row.anchors);
            let excluded = [
                row.file.as_str(),
                unit.rel_path.as_str(),
                row.contract_id.as_str(),
            ]
            .into_iter()
            .collect::<BTreeSet<_>>();
            let mut valid_anchors = anchor_refs
                .iter()
                .filter(|anchor| !excluded.contains(anchor.as_str()))
                .filter(|anchor| {
                    definition_anchors.contains(anchor)
                        || unit_text.contains(anchor.as_str())
                        || (unit_text.trim().is_empty()
                            && unit
                                .sha256
                                .as_deref()
                                .is_some_and(|digest| digest.eq_ignore_ascii_case(anchor)))
                })
                .cloned()
                .collect::<BTreeSet<_>>();
            if valid_anchors.is_empty() {
                issues.push(json!({
                    "reason":"implementation source anchors must cite a concrete token or occurrence from the assigned unit",
                    "unit":row.file,"contract_id":row.contract_id,"actual":row.anchors,
                }));
            }
            if let Some(discovery) = discovery.first() {
                let mode = discovery.get(1).map(|value| value.as_str().to_lowercase());
                let values = discovery
                    .get(2)
                    .map(|value| backticks(value.as_str()))
                    .unwrap_or_default();
                if mode.as_deref() == Some("parsed")
                    && (values.is_empty()
                        || !values
                            .iter()
                            .all(|value| definition_anchors.contains(value)))
                {
                    issues.push(json!({
                        "reason":"Discovery: parsed must cite parser-recognized occurrence-aware anchors",
                        "unit":row.file,"contract_id":row.contract_id,
                    }));
                }
                if mode.as_deref() == Some("manual")
                    && (values.is_empty() || !values.iter().all(|value| unit_text.contains(value)))
                {
                    issues.push(json!({
                        "reason":"Discovery: manual must cite an exact raw token from the assigned unit",
                        "unit":row.file,"contract_id":row.contract_id,
                    }));
                }
                if values.is_disjoint(&valid_anchors) {
                    issues.push(json!({
                        "reason":"Discovery must cite a validated source anchor from the assigned unit",
                        "unit":row.file,"contract_id":row.contract_id,
                    }));
                }
            }
            let mut statuses = Vec::new();
            for (field, value) in [
                ("implementation_trace", row.implementation_trace.as_str()),
                ("failure_trace", row.failure_trace.as_str()),
                ("verification_evidence", row.verification.as_str()),
            ] {
                if let Some((status, detail)) = trace_status(value) {
                    statuses.push((field, status.to_lowercase()));
                    if plain(detail).len() < 12 || boilerplate(detail) {
                        issues.push(json!({
                            "reason":"implementation trace fields require concrete evidence",
                            "unit":row.file,"contract_id":row.contract_id,"field":field,
                        }));
                    }
                } else {
                    issues.push(json!({
                        "reason":"implementation trace fields must begin with pass, gap, blocked, or not applicable plus evidence",
                        "unit":row.file,"contract_id":row.contract_id,"field":field,"actual":value,
                    }));
                }
            }
            let derived = if statuses.iter().any(|(_, status)| status == "gap") {
                "GAP"
            } else if statuses.iter().any(|(_, status)| status == "blocked") {
                "BLOCKED"
            } else {
                "PASS"
            };
            if RESULT_VALUES.contains(&row.result.as_str()) && row.result != derived {
                issues.push(json!({
                    "reason":"implementation inventory Result contradicts its trace statuses",
                    "unit":row.file,"contract_id":row.contract_id,"expected":derived,"actual":row.result,
                }));
            }
            if row.result == "PASS"
                && (!statuses
                    .iter()
                    .any(|(field, status)| *field == "implementation_trace" && status == "pass")
                    || !statuses.iter().any(|(field, status)| {
                        *field == "verification_evidence" && status == "pass"
                    }))
            {
                issues.push(json!({
                    "reason":"implementation inventory PASS requires pass implementation and verification trace statuses",
                    "unit":row.file,"contract_id":row.contract_id,
                }));
            }
            let trace_refs = backticks(&row.implementation_trace);
            let verification_refs = backticks(&row.verification);
            if !valid_anchors.is_empty()
                && !valid_anchors
                    .iter()
                    .any(|anchor| trace_refs.contains(anchor))
            {
                issues.push(json!({
                    "reason":"implementation trace must repeat a validated source anchor",
                    "unit":row.file,"contract_id":row.contract_id,
                }));
            }
            if !valid_anchors.is_empty()
                && !valid_anchors
                    .iter()
                    .any(|anchor| verification_refs.contains(anchor))
            {
                issues.push(json!({
                    "reason":"implementation verification must repeat a validated source anchor",
                    "unit":row.file,"contract_id":row.contract_id,
                }));
            }
            for issue in typed_evidence_issues(
                &row.verification,
                &row.result,
                &sources,
                valid_evidence,
                "implementation verification",
            ) {
                issues.push(json!({
                    "unit":row.file,"contract_id":row.contract_id,"detail":issue,
                }));
            }
            let evidence_type = EVIDENCE_TYPE_RE
                .captures(&row.verification)
                .and_then(|captures| captures.get(1))
                .map(|value| value.as_str().to_lowercase());
            if row.result == "PASS"
                && evidence_type.as_deref() == Some("test")
                && backticks(&row.verification)
                    .iter()
                    .any(|reference| BROWSER_TEST_RE.is_match(reference))
                && !(BROWSER_COMMAND_RE.is_match(&row.verification)
                    && BROWSER_PASS_RE.is_match(&row.verification)
                    && BROWSER_ZERO_FAILURE_RE.is_match(&row.verification))
            {
                issues.push(json!({
                    "reason":"browser-test PASS evidence must record the exact command plus a completed nonzero pass and zero-failure summary",
                    "unit":row.file,"contract_id":row.contract_id,
                }));
            }
            if row.result == "PASS"
                && evidence_type.as_deref() == Some("source-only")
                && !is_test_source_path(&unit.rel_path)
                && STRONG_CLAIM_RE.is_match(&without_backticks(&format!(
                    "{} {}",
                    row.contract, row.implementation_trace
                )))
            {
                issues.push(json!({
                    "reason":"PASS persistence, integration, external-effect, or success claims require test or runtime evidence, not source-only",
                    "unit":row.file,"contract_id":row.contract_id,
                }));
            }
            if matches!(row.result.as_str(), "GAP" | "BLOCKED")
                && findings_by_contract
                    .get(&row.contract_id)
                    .is_none_or(|blocks| blocks.len() != 1)
            {
                issues.push(json!({
                    "reason":"each GAP or BLOCKED implementation Contract ID requires exactly one atomic batch finding",
                    "unit":row.file,"contract_id":row.contract_id,
                }));
            }
            if let Some(blocks) = findings_by_contract.get(&row.contract_id) {
                for block in blocks {
                    let files = block
                        .fields
                        .get("files")
                        .map(|value| backticks(value))
                        .unwrap_or_default();
                    if !files.contains(&unit.rel_path)
                        || ((unit.start_line.is_some() || unit.start_byte.is_some())
                            && !block.text.contains(&unit.unit_id))
                    {
                        issues.push(json!({
                            "reason":"GAP/BLOCKED finding must cite its manifest source file and exact range unit",
                            "unit":row.file,"contract_id":row.contract_id,"heading":block.heading,
                        }));
                    }
                }
            }
            if BATCH_CONTRACT_RE.is_match(&row.contract_id) {
                valid_anchors.retain(|anchor| {
                    anchor_refs.contains(anchor)
                        && trace_refs.contains(anchor)
                        && verification_refs.contains(anchor)
                });
                contracts.insert(
                    row.contract_id.clone(),
                    BatchContract {
                        source_file: unit.rel_path.clone(),
                        result: row.result.clone(),
                        anchors: valid_anchors,
                    },
                );
            }
        }
        for unit_id in &batch.units {
            let Some(unit) = units.get(unit_id.as_str()).copied() else {
                continue;
            };
            let text = unit_text(context, unit);
            let occurrences = definition_anchors(unit, &text);
            let missing_occurrences = occurrences
                .into_iter()
                .filter(|anchor| {
                    !report.implementation.iter().any(|row| {
                        row.file == *unit_id
                            && backticks(&row.anchors).contains(anchor)
                            && backticks(&row.implementation_trace).contains(anchor)
                            && backticks(&row.verification).contains(anchor)
                    })
                })
                .collect::<Vec<_>>();
            if !missing_occurrences.is_empty() {
                issues.push(json!({
                    "reason":"implementation inventory omitted occurrence-aware named source responsibilities",
                    "unit":unit_id,"missing_source_anchors":missing_occurrences,
                }));
            }
            let markers = placeholder_markers(&text);
            let missing_markers = markers
                .iter()
                .filter(|marker| {
                    !report.findings.iter().any(|finding| {
                        finding
                            .fields
                            .get("files")
                            .is_some_and(|files| backticks(files).contains(&unit.rel_path))
                            && finding.text.contains(marker.as_str())
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing_markers.is_empty() {
                placeholder_omissions.push(json!({
                    "batch":batch_id,"report":report.path,"file":unit_id,
                    "source_file":unit.rel_path,"markers":markers,"missing_markers":missing_markers,
                    "reason":"source file contains placeholder markers but report Findings do not cover the marker details",
                }));
            }
            if context
                .sources
                .iter()
                .find(|source| source.rel_path == unit.rel_path)
                .is_some_and(|source| source.interface_relevant)
                && !is_test_source_path(&unit.rel_path)
            {
                let controls = control_markers(&text);
                if !controls.is_empty()
                    && !report.findings.iter().any(|finding| {
                        finding
                            .fields
                            .get("files")
                            .is_some_and(|files| backticks(files).contains(&unit.rel_path))
                    })
                {
                    control_omissions.push(json!({
                        "batch":batch_id,"report":report.path,"file":unit_id,
                        "source_file":unit.rel_path,"markers":controls,
                        "reason":"interface file contains dead, no-op, or unlabeled controls but report Findings do not cover them",
                    }));
                }
            }
        }
        for issue in findings_schema_issues(&report.findings_body) {
            finding_schema.push(json!({"batch":batch_id,"report":report.path,"detail":issue}));
        }
        if !issues.is_empty() {
            grouped.push(json!({"batch":batch_id,"report":report.path,"issues":issues}));
        }
    }
    ImplementationValidation {
        issues: grouped,
        contracts,
        finding_schema,
        placeholder_omissions,
        control_omissions,
    }
}

fn boilerplate(value: &str) -> bool {
    matches!(
        normalized(value).as_str(),
        "" | "n/a"
            | "na"
            | "none"
            | "unknown"
            | "not sure"
            | "looks good"
            | "implemented"
            | "checked"
            | "reviewed"
            | "works"
            | "ok"
            | "pass"
    )
}

fn source_contains_visible_text(source: &str, visible: &str) -> bool {
    let visible = plain(visible);
    if visible.len() < 2 {
        return false;
    }
    source.contains(&visible)
        || normalized(source).contains(&normalized(&visible))
        || backticks(visible.as_str())
            .iter()
            .any(|value| source.contains(value))
}

fn visible_hints(rel_path: &str, source: &str) -> BTreeSet<String> {
    let mut hints = BTreeSet::new();
    for captures in VISIBLE_ATTR_RE.captures_iter(source) {
        if let Some(value) = captures.get(1).or_else(|| captures.get(2)) {
            let value = plain(value.as_str());
            if value.len() >= 2 && !value.contains(['{', '}']) {
                hints.insert(value);
            }
        }
    }
    for captures in VISIBLE_TEXT_RE.captures_iter(source) {
        let value = plain(&TAG_RE.replace_all(&captures[1], ""));
        if value.len() >= 2 && value.chars().any(char::is_alphabetic) {
            hints.insert(value);
        }
    }
    let extension = Path::new(rel_path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    if extension == "md" {
        hints.extend(
            MARKDOWN_H1_RE
                .captures_iter(source)
                .map(|captures| plain(&captures[1])),
        );
    }
    if matches!(
        extension.as_str(),
        "json" | "jsonc" | "yaml" | "yml" | "ftl" | "properties" | "strings"
    ) {
        hints.extend(
            KEY_VALUE_RE
                .captures_iter(source)
                .map(|captures| plain(&captures[1]))
                .filter(|value| value.len() >= 2 && value.chars().any(char::is_alphabetic)),
        );
    }
    if extension == "json"
        && let Ok(value) = serde_json::from_str::<Value>(source)
    {
        fn walk(value: &Value, key: Option<&str>, hints: &mut BTreeSet<String>) {
            match value {
                Value::Object(values) => {
                    for (key, value) in values {
                        walk(value, Some(key), hints);
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        walk(value, key, hints);
                    }
                }
                Value::String(value) => {
                    let key = key.unwrap_or("").to_lowercase();
                    if [
                        "label",
                        "title",
                        "text",
                        "message",
                        "description",
                        "placeholder",
                        "tooltip",
                        "error",
                        "success",
                        "warning",
                        "save",
                        "delete",
                        "button",
                    ]
                    .iter()
                    .any(|token| key.contains(token))
                        && value.len() >= 2
                        && value.len() <= 100
                    {
                        hints.insert(value.clone());
                    }
                }
                _ => {}
            }
        }
        walk(&value, None, &mut hints);
    }
    hints
}

fn interface_issues(context: &ManifestContext, reports: &[ParsedReport]) -> Vec<Value> {
    let source_map = context
        .sources
        .iter()
        .map(|source| (source.rel_path.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    let mut grouped = Vec::new();
    for report in reports {
        let Some(batch_id) = report.batch_id.as_deref() else {
            continue;
        };
        let Some(batch) = context.batches.iter().find(|batch| batch.id == batch_id) else {
            continue;
        };
        let expected = batch
            .files
            .iter()
            .filter(|file| {
                source_map
                    .get(file.as_str())
                    .is_some_and(|source| source.interface_relevant)
            })
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let observed = report
            .interface
            .iter()
            .map(|row| row.file.as_str())
            .collect::<BTreeSet<_>>();
        let mut issues = Vec::new();
        if !report.malformed_interface.is_empty() {
            issues.push(json!({"reason":"malformed interface inventory rows","rows":report.malformed_interface}));
        }
        if expected.is_empty() {
            if !report.interface.is_empty() {
                issues.push(json!({"reason":"non-interface batch should not include interface inventory rows"}));
            }
            if report.interface_body.trim() != "No interface-relevant files in this batch." {
                issues.push(json!({
                    "reason":"non-interface batch must use exact no-interface sentinel",
                    "expected":"No interface-relevant files in this batch.","actual":report.interface_body.trim(),
                }));
            }
        } else {
            let missing = expected.difference(&observed).copied().collect::<Vec<_>>();
            let extra = observed.difference(&expected).copied().collect::<Vec<_>>();
            if !missing.is_empty() {
                issues.push(json!({"reason":"missing interface inventory rows","files":missing}));
            }
            if !extra.is_empty() {
                issues.push(json!({"reason":"interface inventory rows for non-interface files","files":extra}));
            }
            for row in &report.interface {
                if !expected.contains(row.file.as_str()) {
                    continue;
                }
                for (field, value) in [
                    ("visible_text", row.visible_text.as_str()),
                    ("expected_behavior_path", row.expected_path.as_str()),
                    (
                        "actual_implementation_notes",
                        row.implementation_notes.as_str(),
                    ),
                ] {
                    if boilerplate(value) {
                        issues.push(json!({
                            "reason":"interface inventory row contains boilerplate text",
                            "file":row.file,"field":field,"actual":value,
                        }));
                    }
                }
                if normalized(&row.visible_text) != "none found" {
                    let source = read_bytes_nofollow(
                        &context.repo_root.join(&row.file),
                        Some(&context.repo_root),
                    )
                    .ok()
                    .flatten()
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default();
                    if !source_contains_visible_text(&source, &row.visible_text) {
                        issues.push(json!({
                            "reason":"visible text/control/message is not present in the source file",
                            "file":row.file,"visible_text":row.visible_text,
                        }));
                    }
                }
            }
            for file in &expected {
                let source =
                    read_bytes_nofollow(&context.repo_root.join(file), Some(&context.repo_root))
                        .ok()
                        .flatten()
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .unwrap_or_default();
                let observed = report
                    .interface
                    .iter()
                    .filter(|row| row.file == **file)
                    .map(|row| plain(&row.visible_text))
                    .collect::<BTreeSet<_>>();
                let missing = visible_hints(file, &source)
                    .difference(&observed)
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    issues.push(json!({
                        "reason":"interface inventory is missing visible text/control/message hints",
                        "file":file,"missing_visible_text":missing,
                    }));
                }
            }
        }
        if !issues.is_empty() {
            grouped.push(json!({"batch":batch_id,"report":report.path,"issues":issues}));
        }
    }
    grouped
}

fn lead_issues(
    context: &ManifestContext,
    path: &Path,
    contracts: &BTreeMap<String, BatchContract>,
    valid_evidence: &BTreeSet<String>,
) -> Result<(Vec<Value>, usize), String> {
    let bytes = read_bytes_nofollow(path, Some(&context.reports_root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("lead reconciliation report is missing: {}", path.display()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| "lead reconciliation report is not UTF-8".to_owned())?;
    let bodies = section_bodies(&text);
    let mut issues = Vec::new();
    if section_order(&text) != LEAD_SECTIONS {
        issues.push(json!({
            "reason":"lead reconciliation sections must exactly match required order",
            "expected":LEAD_SECTIONS,"actual":section_order(&text),
        }));
    }
    let run_ids = section_values(&text, "run id");
    if run_ids != [context.run_id.clone()] {
        issues.push(json!({"reason":"lead reconciliation Run ID mismatch","actual":run_ids}));
    }
    if section_values(&text, "worker") != ["lead_reconciliation"] {
        issues.push(json!({"reason":"lead reconciliation Worker must be lead_reconciliation"}));
    }
    let trace_body = bodies
        .get("cross-file contract trace")
        .cloned()
        .unwrap_or_default();
    let mut rows = Vec::new();
    if context.sources.is_empty()
        && trace_body.trim() == "No source-backed implementation contracts were queued."
    {
        if !contracts.is_empty() {
            issues.push(json!({"reason":"empty lead sentinel contradicts batch contracts"}));
        }
    } else {
        let table = table_lines(&text, "Cross-File Contract Trace");
        if table.first().map(|(_, columns)| {
            columns
                .iter()
                .map(|column| column.to_lowercase())
                .collect::<Vec<_>>()
        }) != Some(LEAD_HEADERS.map(str::to_owned).to_vec())
        {
            issues.push(
                json!({"reason":"lead reconciliation requires the exact 13-column table header"}),
            );
        }
        if table.get(1).is_none_or(|(_, columns)| {
            columns.len() != LEAD_HEADERS.len() || !is_separator_row(columns)
        }) {
            issues.push(json!({"reason":"lead reconciliation requires the exact 13-column table separator immediately after the header"}));
        }
        for (raw, columns) in table.into_iter().skip(2) {
            if columns.len() != LEAD_HEADERS.len()
                || columns.iter().any(|column| column.is_empty())
                || is_separator_row(&columns)
            {
                issues
                    .push(json!({"reason":"malformed lead reconciliation contract row","row":raw}));
            } else {
                rows.push(columns);
            }
        }
        if !context.sources.is_empty() && rows.is_empty() {
            issues.push(
                json!({"reason":"non-empty manifests require at least one lead contract row"}),
            );
        }
    }
    let source_names = source_set(context);
    let mut lead_ids = BTreeSet::new();
    let mut mapped = Vec::new();
    let finding_blocks =
        parse_finding_blocks(bodies.get("findings").map(String::as_str).unwrap_or(""));
    let mut gap_ids = BTreeSet::new();
    for columns in &rows {
        let contract_id = plain(&columns[0]);
        if !LEAD_CONTRACT_RE.is_match(&contract_id) || !lead_ids.insert(contract_id.clone()) {
            issues.push(json!({"reason":"lead reconciliation Contract ID must be unique lead:C###","contract_id":contract_id}));
        }
        let batch_refs = backticks(&columns[1])
            .into_iter()
            .filter(|reference| BATCH_CONTRACT_RE.is_match(reference))
            .collect::<Vec<_>>();
        if batch_refs.is_empty() || batch_refs.len() != backticks(&columns[1]).len() {
            issues.push(json!({"reason":"lead reconciliation Batch Contract IDs must contain exact unique backticked batch IDs","contract_id":contract_id}));
        }
        mapped.extend(batch_refs.iter().cloned());
        let unknown = batch_refs
            .iter()
            .filter(|reference| !contracts.contains_key(*reference))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            issues.push(json!({"reason":"lead reconciliation maps unknown batch Contract IDs","contract_id":contract_id,"batch_contract_ids":unknown}));
        }
        let result = columns[12].trim().to_uppercase();
        if !RESULT_VALUES.contains(&result.as_str()) {
            issues.push(json!({"reason":"lead reconciliation Result must be PASS, GAP, or BLOCKED","contract_id":contract_id}));
        }
        let mut statuses = Vec::new();
        for (offset, field) in LEAD_TRACE_FIELDS.iter().enumerate() {
            let value = &columns[offset + 3];
            if let Some((status, detail)) = trace_status(value) {
                statuses.push((field, status.to_lowercase()));
                if plain(detail).len() < 12 {
                    issues.push(json!({"reason":"lead trace requires concrete evidence","contract_id":contract_id,"field":field}));
                }
            } else {
                issues.push(json!({"reason":"lead trace cells must begin with a status plus evidence","contract_id":contract_id,"field":field}));
            }
        }
        let derived = if statuses.iter().any(|(_, status)| status == "gap") {
            "GAP"
        } else if statuses.iter().any(|(_, status)| status == "blocked") {
            "BLOCKED"
        } else {
            "PASS"
        };
        if result != derived {
            issues.push(json!({"reason":"lead reconciliation Result contradicts trace statuses","contract_id":contract_id,"expected":derived,"actual":result}));
        }
        if result == "PASS"
            && (!statuses
                .iter()
                .any(|(field, status)| **field == "observable-outcome" && status == "pass")
                || !statuses
                    .iter()
                    .any(|(field, status)| **field == "verification" && status == "pass"))
        {
            issues.push(json!({"reason":"lead PASS requires pass observable-outcome and verification","contract_id":contract_id}));
        }
        let mapped_results = batch_refs
            .iter()
            .filter_map(|id| contracts.get(id))
            .map(|contract| contract.result.as_str())
            .collect::<BTreeSet<_>>();
        if mapped_results.contains("GAP") && result != "GAP"
            || mapped_results.contains("BLOCKED") && result == "PASS"
        {
            issues.push(json!({"reason":"lead reconciliation cannot hide a mapped unresolved batch result","contract_id":contract_id}));
        }
        let anchor_refs = backticks(&columns[2]);
        let source_refs = anchor_refs
            .intersection(&source_names)
            .cloned()
            .collect::<BTreeSet<_>>();
        for batch_id in &batch_refs {
            if let Some(batch) = contracts.get(batch_id)
                && (!source_refs.contains(&batch.source_file)
                    || batch.anchors.is_empty()
                    || !batch
                        .anchors
                        .iter()
                        .any(|anchor| anchor_refs.contains(anchor))
                    || !batch
                        .anchors
                        .iter()
                        .any(|anchor| backticks(&columns[9]).contains(anchor))
                    || !batch
                        .anchors
                        .iter()
                        .any(|anchor| backticks(&columns[11]).contains(anchor)))
            {
                issues.push(json!({"reason":"lead must carry every mapped batch source/anchor through anchors, outcome, and verification","contract_id":contract_id,"batch_contract_id":batch_id}));
            }
        }
        for issue in typed_evidence_issues(
            &columns[11],
            &result,
            &source_names,
            valid_evidence,
            "lead reconciliation verification",
        ) {
            issues.push(json!({"contract_id":contract_id,"detail":issue}));
        }
        if result == "PASS"
            && EVIDENCE_TYPE_RE
                .captures(&columns[11])
                .and_then(|capture| capture.get(1))
                .is_some_and(|value| value.as_str().eq_ignore_ascii_case("source-only"))
            && STRONG_CLAIM_RE.is_match(&without_backticks(&columns[3..12].join(" ")))
        {
            issues.push(json!({"reason":"lead PASS effect claims require test or runtime evidence","contract_id":contract_id}));
        }
        if matches!(result.as_str(), "GAP" | "BLOCKED") {
            gap_ids.insert(contract_id);
        }
    }
    let missing = contracts
        .keys()
        .filter(|id| !mapped.contains(id))
        .cloned()
        .collect::<Vec<_>>();
    let duplicate = duplicates(mapped);
    if !missing.is_empty() {
        issues.push(json!({"reason":"lead reconciliation must map every batch Contract ID exactly once","missing_batch_contract_ids":missing}));
    }
    if !duplicate.is_empty() {
        issues.push(json!({"reason":"lead reconciliation must not map a batch Contract ID more than once","duplicate_batch_contract_ids":duplicate}));
    }
    issues.extend(findings_schema_issues(
        bodies.get("findings").map(String::as_str).unwrap_or(""),
    ));
    for gap in &gap_ids {
        let count = finding_blocks
            .iter()
            .filter(|block| finding_contract_ids(block, true) == [gap.clone()])
            .count();
        if count != 1 {
            issues.push(json!({"reason":"each lead GAP/BLOCKED Contract ID requires one atomic finding","contract_id":gap,"finding_count":count}));
        }
    }
    if bodies
        .get("open questions")
        .is_none_or(|body| body.trim().is_empty())
    {
        issues.push(json!({"reason":"lead reconciliation Open Questions must not be empty"}));
    }
    Ok((issues, rows.len()))
}

fn journey_report_issues(
    context: &ManifestContext,
    path: &Path,
    worker: &str,
    evidence_records: &EvidenceRecords,
) -> Result<Vec<Value>, String> {
    let bytes = read_bytes_nofollow(path, Some(&context.reports_root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("journey report is missing: {}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| "journey report is not UTF-8".to_owned())?;
    let bodies = section_bodies(&text);
    let (required_sections, worker_label, table_heading, expected_headers) = if worker == "source" {
        (
            vec![
                "run id",
                "worker",
                "journey sources",
                "proposed journeys",
                "ui source journey checks",
                "findings",
                "open questions",
            ],
            "journey_source",
            "ui source journey checks",
            [
                "journey",
                "step",
                "files",
                "primary navigation/decision elements",
                "relevance estimate",
                "required information",
                "interaction and metadata checklist",
                "mobile/desktop availability",
                "test mode evidence",
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        )
    } else {
        (
            vec![
                "run id",
                "worker",
                "visual tooling",
                "visual journey checks",
                "changed visual review",
                "findings",
                "open questions",
            ],
            "visual_journey",
            "visual journey checks",
            [
                "journey",
                "viewport",
                "route/screen",
                "evidence",
                "navigation visibility",
                "decision information",
                "interaction and metadata checklist",
                "visual quality",
                "result",
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        )
    };
    let mut issues = Vec::new();
    if section_order(&text) != required_sections {
        issues.push(json!({
            "path":path,"reason":"journey report sections must exactly match required order",
            "expected":required_sections,"actual":section_order(&text),
        }));
    }
    if section_values(&text, "run id") != [context.run_id.clone()] {
        issues.push(json!({"path":path,"reason":"journey report Run ID mismatch"}));
    }
    if section_values(&text, "worker") != [worker_label] {
        issues.push(json!({"path":path,"reason":"journey report Worker mismatch"}));
    }
    let rows =
        parse_markdown_table_dicts(bodies.get(table_heading).map(String::as_str).unwrap_or(""));
    if rows.is_empty() {
        issues.push(json!({"path":path,"section":table_heading,"reason":"journey report must include at least one table row"}));
    } else {
        let actual = rows[0].keys().map(String::as_str).collect::<BTreeSet<_>>();
        if actual != expected_headers {
            issues.push(json!({"path":path,"section":table_heading,"reason":"journey table headers must exactly match required columns"}));
        }
    }
    let interface_files = context
        .sources
        .iter()
        .filter(|source| source.interface_relevant)
        .map(|source| source.rel_path.as_str())
        .collect::<BTreeSet<_>>();
    let report_context = format!(
        "{}\n{}\n{}\n{}",
        bodies
            .get("journey sources")
            .map(String::as_str)
            .unwrap_or(""),
        bodies
            .get("ui source journey checks")
            .map(String::as_str)
            .unwrap_or(""),
        bodies
            .get("visual journey checks")
            .map(String::as_str)
            .unwrap_or(""),
        bodies
            .get("visual tooling")
            .map(String::as_str)
            .unwrap_or("")
    );
    let not_applicable_text = normalized(&report_context);
    let report_is_not_applicable = worker == "visual"
        && not_applicable_text.contains("not applicable")
        && [
            "no repo-owned",
            "host-owned",
            "no visual ui",
            "no rendered ui",
        ]
        .iter()
        .any(|term| not_applicable_text.contains(term));
    let table_text = rows
        .iter()
        .flat_map(BTreeMap::values)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let missing_files = interface_files
        .iter()
        .filter(|file| !table_text.contains(**file))
        .copied()
        .collect::<Vec<_>>();
    if worker == "source" && !missing_files.is_empty() {
        issues.push(json!({"path":path,"reason":"journey source table must cover each manifest interface file","files":missing_files}));
    }
    if !interface_files.is_empty() && !report_is_not_applicable {
        let checklist_text = format!(
            "{}\n{}",
            bodies.get(table_heading).map(String::as_str).unwrap_or(""),
            bodies.get("findings").map(String::as_str).unwrap_or("")
        );
        let missing = interaction_checklist_missing(&checklist_text);
        if !missing.is_empty() {
            issues.push(json!({"path":path,"section":table_heading,"reason":"journey report must mark every interaction checklist label","missing":missing}));
        }
    }
    if worker == "source" {
        let allowed = [
            "critical-always",
            "primary-frequent",
            "secondary-occasional",
            "rare-under-5-percent",
        ];
        for (index, row) in rows.iter().enumerate() {
            let relevance = row
                .get("relevance estimate")
                .map(|value| {
                    value
                        .split([',', ';', '/'])
                        .map(normalized)
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if relevance.is_empty()
                || relevance
                    .iter()
                    .any(|value| !allowed.contains(&value.as_str()))
            {
                issues.push(json!({"path":path,"row":index + 1,"field":"relevance estimate"}));
            }
        }
    } else {
        let tooling = normalized(
            bodies
                .get("visual tooling")
                .map(String::as_str)
                .unwrap_or(""),
        );
        if ![
            "test mode",
            "fixture",
            "playwright",
            "cypress",
            "storybook",
            "browser",
            "not applicable",
            "no visual",
        ]
        .iter()
        .any(|term| tooling.contains(term))
        {
            issues.push(json!({"path":path,"section":"visual tooling","reason":"visual report must identify tooling/test mode or explain non-applicability"}));
        }
        let rendered = if report_is_not_applicable {
            Vec::new()
        } else {
            rows.iter()
                .filter(|row| {
                    !matches!(
                        row.get("result").map(|value| normalized(value)).as_deref(),
                        Some("blocked" | "not applicable" | "not-applicable" | "n/a")
                    )
                })
                .collect::<Vec<_>>()
        };
        if !rendered.is_empty() {
            let mut required = ["screenshot".to_owned()]
                .into_iter()
                .collect::<BTreeSet<_>>();
            let web = interface_files.iter().any(|file| {
                matches!(
                    Path::new(file).extension().and_then(|value| value.to_str()),
                    Some("html" | "jsx" | "tsx" | "vue" | "svelte" | "astro")
                )
            });
            if web {
                required.extend(
                    ["formal-web-verifier", "review-queue", "manual-review"].map(str::to_owned),
                );
            }
            for issue in validate_references(&text, evidence_records, Some(&required)) {
                issues.push(json!({"path":path,"section":"visual evidence","detail":issue}));
            }
            for (index, row) in rendered.iter().enumerate() {
                let refs =
                    evidence_references(row.get("evidence").map(String::as_str).unwrap_or(""));
                if !refs.iter().any(|id| {
                    evidence_records.get(id).is_some_and(|record| {
                        matches!(
                            record.get("kind").and_then(Value::as_str),
                            Some("screenshot" | "native-snapshot")
                        )
                    })
                }) {
                    issues.push(json!({"path":path,"row":index + 1,"reason":"each rendered viewport row must bind screenshot evidence"}));
                }
            }
            let mut by_journey: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in &rows {
                by_journey
                    .entry(
                        row.get("journey")
                            .map(|value| normalized(value))
                            .unwrap_or_default(),
                    )
                    .or_default()
                    .insert(
                        row.get("viewport")
                            .map(|value| normalized(value))
                            .unwrap_or_default(),
                    );
            }
            for (journey, viewports) in by_journey {
                let desktop = viewports.iter().any(|value| value.contains("desktop"));
                let mobile = viewports.iter().any(|value| {
                    ["mobile", "narrow", "small", "phone"]
                        .iter()
                        .any(|term| value.contains(term))
                });
                if !desktop || (web && !mobile) {
                    issues.push(json!({"path":path,"journey":journey,"reason":"visual journey requires desktop and applicable narrow-mobile rows"}));
                }
            }
        }
        let review_rows = parse_markdown_table_dicts(
            bodies
                .get("changed visual review")
                .map(String::as_str)
                .unwrap_or(""),
        );
        let web = interface_files.iter().any(|file| {
            matches!(
                Path::new(file).extension().and_then(|value| value.to_str()),
                Some("html" | "jsx" | "tsx" | "vue" | "svelte" | "astro")
            )
        });
        if web && !report_is_not_applicable && review_rows.is_empty() {
            issues.push(json!({"path":path,"section":"changed visual review","reason":"rendered web UI requires a changed visual-review table"}));
        }
        let review_text = bodies
            .get("changed visual review")
            .map(String::as_str)
            .unwrap_or("");
        if web && !report_is_not_applicable {
            let kinds = evidence_references(review_text)
                .iter()
                .filter_map(|id| evidence_records.get(id))
                .filter_map(|record| record.get("kind"))
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>();
            let missing = ["formal-web-verifier", "review-queue", "manual-review"]
                .into_iter()
                .filter(|kind| !kinds.contains(kind))
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                issues.push(json!({"path":path,"section":"changed visual review","reason":"web UI review must cite the complete formal review chain","missing_kinds":missing}));
            }
        }
    }
    issues.extend(findings_schema_issues(
        bodies.get("findings").map(String::as_str).unwrap_or(""),
    ));
    Ok(issues)
}

fn prepare_receipt(context: &ManifestContext, requested: &Path) -> Result<PathBuf, String> {
    let target = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve receipt path: {error}"))?
            .join(requested)
    };
    if target != context.root.join("verification_receipt.json") {
        return Err(
            "Verification receipt output must be exactly <manifest-dir>/verification_receipt.json."
                .to_owned(),
        );
    }
    if let Ok(metadata) = target.symlink_metadata() {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(
                "Verification receipt output must be a regular non-symlink file".to_owned(),
            );
        }
        std::fs::remove_file(&target)
            .map_err(|error| format!("cannot invalidate prior receipt: {error}"))?;
    }
    Ok(target)
}

fn snapshot_reports(context: &ManifestContext) -> Result<BTreeMap<String, String>, String> {
    context
        .authorized_reports
        .iter()
        .map(|name| {
            let bytes = read_bytes_nofollow(
                &context.reports_root.join(name),
                Some(&context.reports_root),
            )
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("authorized report is missing: {name}"))?;
            Ok((name.clone(), sha256_hex(&bytes)))
        })
        .collect()
}

fn report_paths(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut files = BTreeSet::new();
    for input in inputs {
        let metadata = input
            .symlink_metadata()
            .map_err(|_| format!("Report path does not exist: {}", input.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Report path must not be a symlink: {}",
                input.display()
            ));
        }
        if metadata.is_dir() {
            let directory =
                validate_directory_nofollow(input).map_err(|error| error.to_string())?;
            for entry in std::fs::read_dir(&directory)
                .map_err(|error| format!("cannot list report directory: {error}"))?
                .filter_map(Result::ok)
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                let valid_batch = name.starts_with("batch_")
                    && name.ends_with(".md")
                    && BATCH_ID_RE
                        .find(&name)
                        .is_some_and(|value| value.start() == 0 && value.end() + 3 == name.len());
                if entry.file_type().ok().is_some_and(|kind| kind.is_file())
                    && (valid_batch || name == "lead_reconciliation.md")
                {
                    files.insert(entry.path());
                }
            }
        } else if metadata.is_file() {
            let name = input
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let valid_batch = name.starts_with("batch_")
                && name.ends_with(".md")
                && BATCH_ID_RE
                    .find(name)
                    .is_some_and(|value| value.start() == 0 && value.end() + 3 == name.len());
            if !valid_batch && name != "lead_reconciliation.md" {
                return Err(format!(
                    "Report file must use exact batch_###.md or lead_reconciliation.md filename: {}",
                    input.display()
                ));
            }
            files.insert(input.clone());
        } else {
            return Err(format!("Report path does not exist: {}", input.display()));
        }
    }
    let parents = files
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with("batch_"))
        })
        .filter_map(|path| path.parent().map(Path::to_owned))
        .collect::<BTreeSet<_>>();
    for parent in parents {
        let lead = parent.join("lead_reconciliation.md");
        if lead
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            files.insert(lead);
        }
    }
    Ok(files.into_iter().collect())
}

fn current_hash_issues(
    context: &ManifestContext,
    selected_files: &BTreeSet<String>,
    skip: bool,
) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    if skip {
        return (
            Vec::new(),
            Vec::new(),
            vec![json!({
                "flag":"--skip-current-hash-check",
                "reason":"current source fingerprint freshness check was skipped; verification is degraded",
            })],
        );
    }
    let mut mismatches = Vec::new();
    let mut errors = Vec::new();
    for source in context
        .sources
        .iter()
        .filter(|source| selected_files.contains(&source.rel_path))
    {
        let Some(expected) = source.sha256.as_deref() else {
            continue;
        };
        match read_bytes_nofollow(
            &context.repo_root.join(&source.rel_path),
            Some(&context.repo_root),
        ) {
            Ok(Some(bytes)) => {
                let current = sha256_hex(&bytes);
                if current != expected {
                    mismatches.push(json!({
                        "file":source.rel_path,"expected":expected,"current":current,"reason":"changed",
                    }));
                }
            }
            Ok(None) => mismatches.push(json!({
                "file":source.rel_path,"expected":expected,"current":Value::Null,"reason":"missing",
            })),
            Err(error) => errors.push(json!({
                "file":source.rel_path,"expected":expected,"current":Value::Null,
                "reason":format!("current SHA-256 check could not read file: {error}"),
            })),
        }
    }
    (mismatches, errors, Vec::new())
}

fn verify_result(
    context: &ManifestContext,
    paths: &[PathBuf],
    options: &VerifyOptions,
) -> Result<Value, String> {
    let selected_batches = if let Some(batch_id) = options.batch_id.as_deref() {
        let batch = context
            .batches
            .iter()
            .find(|batch| batch.id == batch_id)
            .ok_or_else(|| format!("--batch-id is not present in the manifest: {batch_id}"))?;
        vec![batch]
    } else {
        context.batches.iter().collect::<Vec<_>>()
    };
    let known_batch_ids = selected_batches
        .iter()
        .map(|batch| batch.id.clone())
        .collect::<BTreeSet<_>>();
    let expected_units = selected_batches
        .iter()
        .flat_map(|batch| batch.units.iter().cloned())
        .collect::<BTreeSet<_>>();
    let expected_files = selected_batches
        .iter()
        .flat_map(|batch| batch.files.iter().cloned())
        .collect::<BTreeSet<_>>();
    let batch_paths = paths
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| BATCH_ID_RE.is_match(name))
        })
        .cloned()
        .collect::<Vec<_>>();
    let reports = batch_paths
        .iter()
        .map(|path| parse_report(path, &known_batch_ids))
        .collect::<Result<Vec<_>, _>>()?;
    let (evidence_records, valid_evidence, evidence_manifest_issues) = valid_evidence_ids(context);

    let mut observed: BTreeMap<String, Vec<&CoverageRow>> = BTreeMap::new();
    for report in &reports {
        for row in &report.coverage {
            observed.entry(row.file.clone()).or_default().push(row);
        }
    }
    let observed_units = observed.keys().cloned().collect::<BTreeSet<_>>();
    let missing = expected_units
        .difference(&observed_units)
        .cloned()
        .collect::<Vec<_>>();
    let extra = observed_units
        .difference(&expected_units)
        .cloned()
        .collect::<Vec<_>>();
    let duplicate = observed
        .iter()
        .filter_map(|(unit, rows)| (rows.len() > 1).then_some(unit.clone()))
        .collect::<Vec<_>>();
    let unchecked = observed
        .iter()
        .filter(|(unit, rows)| {
            expected_units.contains(*unit) && rows.iter().any(|row| row.status != "CHECKED")
        })
        .map(|(unit, _)| unit.clone())
        .collect::<Vec<_>>();
    let unit_map = context
        .units
        .iter()
        .map(|unit| (unit.unit_id.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut report_hash_mismatches = Vec::new();
    for (unit_id, rows) in &observed {
        let expected = unit_map
            .get(unit_id.as_str())
            .and_then(|unit| unit.sha256.as_deref());
        if let Some(expected) = expected {
            for row in rows {
                if row.sha256 != expected {
                    report_hash_mismatches.push(json!({
                        "file":unit_id,"expected":expected,"reported":row.sha256,
                    }));
                }
            }
        }
    }
    let mut batch_counts = BTreeMap::new();
    for report in &reports {
        if let Some(batch_id) = &report.batch_id {
            *batch_counts.entry(batch_id.clone()).or_insert(0usize) += 1;
        }
    }
    let missing_batch_reports = known_batch_ids
        .iter()
        .filter(|batch| !batch_counts.contains_key(*batch))
        .cloned()
        .collect::<Vec<_>>();
    let duplicate_batch_reports = batch_counts
        .iter()
        .filter_map(|(batch, count)| (*count > 1).then_some(batch.clone()))
        .collect::<Vec<_>>();
    let unassigned_reports = reports
        .iter()
        .filter(|report| report.batch_id.is_none())
        .map(|report| report.path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut location_mismatches = Vec::new();
    let mut batch_id_mismatches = Vec::new();
    let mut run_id_mismatches = Vec::new();
    for report in &reports {
        if let Some(batch_id) = report.batch_id.as_deref() {
            let expected = context.reports_root.join(format!("{batch_id}.md"));
            if report.path != expected {
                location_mismatches.push(json!({
                    "report":report.path,"expected":expected,"reason":"batch report must use manifest-owned exact path",
                }));
            }
        }
        let mut reasons = Vec::new();
        if report.declared_batch_ids.len() != 1 {
            reasons.push(format!(
                "expected exactly one declared Batch ID, found {}",
                report.declared_batch_ids.len()
            ));
        }
        if report.filename_batch_id != report.declared_batch_ids.first().cloned() {
            reasons.push("filename Batch ID does not match declaration".to_owned());
        }
        if !reasons.is_empty() {
            batch_id_mismatches.push(json!({"report":report.path,"reasons":reasons}));
        }
        if report.run_ids != [context.run_id.clone()] {
            run_id_mismatches.push(json!({
                "report":report.path,"run_ids":report.run_ids,
                "reasons":["report must declare exactly the manifest Run ID"],
            }));
        }
    }
    let malformed_rows = reports
        .iter()
        .filter(|report| !report.malformed_coverage.is_empty())
        .map(|report| json!({"report":report.path,"rows":report.malformed_coverage}))
        .collect::<Vec<_>>();
    let missing_sections = reports
        .iter()
        .filter(|report| !report.missing_sections.is_empty())
        .map(|report| json!({"report":report.path,"sections":report.missing_sections}))
        .collect::<Vec<_>>();
    let section_shape_mismatches = reports
        .iter()
        .filter(|report| report.section_order != REQUIRED_SECTIONS)
        .map(|report| {
            json!({
                "report":report.path,"expected":REQUIRED_SECTIONS,"actual":report.section_order,
            })
        })
        .collect::<Vec<_>>();
    let semantic_report_issues = reports
        .iter()
        .filter(|report| !report.narrative_issues.is_empty())
        .map(|report| json!({"report":report.path,"issues":report.narrative_issues}))
        .collect::<Vec<_>>();
    let mut batch_mismatches = Vec::new();
    for report in &reports {
        let Some(batch_id) = report.batch_id.as_deref() else {
            continue;
        };
        let expected = selected_batches
            .iter()
            .find(|batch| batch.id == batch_id)
            .map(|batch| batch.units.iter().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let actual = report
            .coverage
            .iter()
            .map(|row| row.file.clone())
            .collect::<BTreeSet<_>>();
        let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        let extra = actual.difference(&expected).cloned().collect::<Vec<_>>();
        let unchecked = report
            .coverage
            .iter()
            .filter(|row| expected.contains(&row.file) && row.status != "CHECKED")
            .map(|row| row.file.clone())
            .collect::<Vec<_>>();
        if !missing.is_empty() || !extra.is_empty() || !unchecked.is_empty() {
            batch_mismatches.push(json!({
                "batch":batch_id,"report":report.path,"missing":missing,"extra":extra,"unchecked":unchecked,
            }));
        }
    }
    let ImplementationValidation {
        issues: implementation_inventory_issues,
        contracts,
        finding_schema: finding_schema_issues,
        placeholder_omissions,
        control_omissions: interface_control_omissions,
    } = implementation_issues(context, &reports, &valid_evidence);
    let interface_inventory_issues = interface_issues(context, &reports);
    let (current_hash_mismatches, current_hash_errors, verification_warnings) =
        current_hash_issues(context, &expected_files, options.skip_current_hash_check);
    let completion_marker_mismatches = completion_marker_issues(context);
    let (unresolved_scope_warnings, excluded_file_mismatches) = if options.batch_id.is_none() {
        excluded_issues(context)
    } else {
        (Vec::new(), Vec::new())
    };
    let effort_ledger_mismatches = if options.batch_id.is_none() {
        effort_ledger_issues(context)
    } else {
        Vec::new()
    };
    let mut lead_reconciliation_issues = Vec::new();
    let mut lead_count = 0usize;
    if options.batch_id.is_none() {
        let lead_paths = paths
            .iter()
            .filter(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some("lead_reconciliation.md")
            })
            .collect::<Vec<_>>();
        if lead_paths.len() != 1 {
            lead_reconciliation_issues.push(json!({
                "reason":"exactly one reports/lead_reconciliation.md artifact is required",
                "reports":lead_paths,
            }));
        } else if *lead_paths[0] != context.reports_root.join("lead_reconciliation.md") {
            lead_reconciliation_issues.push(
                json!({"reason":"lead reconciliation report must use exact manifest-owned path"}),
            );
        } else {
            let (mut issues, count) =
                lead_issues(context, lead_paths[0], &contracts, &valid_evidence)?;
            lead_reconciliation_issues.append(&mut issues);
            lead_count = count;
        }
    }
    let mut journey_issues = Vec::new();
    if options.batch_id.is_none() && context.journey_required {
        for (name, worker) in [
            ("journey_audit.md", "source"),
            ("visual_journey_audit.md", "visual"),
        ] {
            let journey_path = context.reports_root.join(name);
            if !journey_path.is_file() || journey_path.is_symlink() {
                journey_issues.push(json!({"reason":"required journey report is missing or misplaced","report":name}));
            } else {
                journey_issues.extend(journey_report_issues(
                    context,
                    &journey_path,
                    worker,
                    &evidence_records,
                )?);
            }
        }
        if !evidence_manifest_issues.is_empty() {
            journey_issues.push(json!({"reason":"visual evidence manifest is invalid","issues":evidence_manifest_issues}));
        }
    }
    let known_names = selected_batches
        .iter()
        .map(|batch| format!("{}.md", batch.id))
        .chain(std::iter::once("lead_reconciliation.md".to_owned()))
        .collect::<BTreeSet<_>>();
    let extra_report_paths = paths
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !known_names.contains(name))
        })
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut source_text_errors = Vec::new();
    if !journey_issues.is_empty() {
        source_text_errors
            .push(json!({"reason":"journey report validation failed","issues":journey_issues}));
    }
    let mut result = Map::new();
    result.insert("expected_count".to_owned(), json!(expected_units.len()));
    result.insert("reported_count".to_owned(), json!(observed_units.len()));
    result.insert(
        "expected_batch_count".to_owned(),
        json!(known_batch_ids.len()),
    );
    result.insert(
        "verification_scope".to_owned(),
        json!(
            options
                .batch_id
                .as_ref()
                .map(|batch| format!("batch:{batch}"))
                .unwrap_or_else(|| "complete".to_owned())
        ),
    );
    result.insert(
        "report_files".to_owned(),
        json!(
            paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        ),
    );
    result.insert(
        "effort_ledger_provenance_note".to_owned(),
        json!("Ledger-recorded effort consistency only; scheduler effort settings are not independently verified by this tool."),
    );
    result.insert(
        "effort_verification_scope".to_owned(),
        json!("ledger-recorded"),
    );
    for (name, value) in [
        ("missing", json!(missing)),
        ("extra", json!(extra)),
        ("duplicate", json!(duplicate)),
        ("unchecked", json!(unchecked)),
        ("missing_batch_reports", json!(missing_batch_reports)),
        ("duplicate_batch_reports", json!(duplicate_batch_reports)),
        (
            "unassigned_reports",
            json!([unassigned_reports, extra_report_paths].concat()),
        ),
        ("report_location_mismatches", json!(location_mismatches)),
        ("run_id_mismatches", json!(run_id_mismatches)),
        ("report_hash_mismatches", json!(report_hash_mismatches)),
        ("current_hash_mismatches", json!(current_hash_mismatches)),
        ("current_hash_errors", json!(current_hash_errors)),
        ("source_text_errors", json!(source_text_errors)),
        ("verification_warnings", json!(verification_warnings)),
        (
            "unresolved_scope_warnings",
            json!(unresolved_scope_warnings),
        ),
        ("excluded_file_mismatches", json!(excluded_file_mismatches)),
        (
            "completion_marker_mismatches",
            json!(completion_marker_mismatches),
        ),
        ("effort_ledger_mismatches", json!(effort_ledger_mismatches)),
        ("batch_id_mismatches", json!(batch_id_mismatches)),
        ("malformed_rows", json!(malformed_rows)),
        (
            "lead_reconciliation_issues",
            json!(lead_reconciliation_issues),
        ),
        (
            "implementation_inventory_issues",
            json!(implementation_inventory_issues),
        ),
        (
            "interface_inventory_issues",
            json!(interface_inventory_issues),
        ),
        ("finding_schema_issues", json!(finding_schema_issues)),
        ("placeholder_omissions", json!(placeholder_omissions)),
        (
            "interface_control_omissions",
            json!(interface_control_omissions),
        ),
        ("missing_sections", json!(missing_sections)),
        ("section_shape_mismatches", json!(section_shape_mismatches)),
        ("semantic_report_issues", json!(semantic_report_issues)),
        ("batch_mismatches", json!(batch_mismatches)),
    ] {
        result.insert(name.to_owned(), value);
    }
    result.insert(
        "current_hash_check_skipped".to_owned(),
        json!(options.skip_current_hash_check),
    );
    result.insert(
        "lead_reconciliation_contract_count".to_owned(),
        json!(lead_count),
    );
    let ok = RESULT_ARRAY_KEYS
        .iter()
        .chain(std::iter::once(&"batch_mismatches"))
        .all(|key| {
            result
                .get(*key)
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        });
    result.insert("ok".to_owned(), json!(ok));
    Ok(Value::Object(result))
}

fn write_receipt_atomic(path: &Path, receipt: &Value) -> Result<(), String> {
    let mut random = [0u8; 8];
    getrandom::fill(&mut random)
        .map_err(|error| format!("cannot generate receipt nonce: {error}"))?;
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("receipt"),
        sha256_hex(&random)[..16].to_owned()
    ));
    let mut bytes = serde_json::to_vec_pretty(receipt).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    write_bytes_nofollow(&temporary, &bytes, 0o600).map_err(|error| error.to_string())?;
    if path.symlink_metadata().is_ok() {
        let _ = std::fs::remove_file(&temporary);
        return Err("Verification receipt output became occupied before publication".to_owned());
    }
    std::fs::rename(&temporary, path).map_err(|error| format!("cannot publish receipt: {error}"))
}

pub fn verify(options: &VerifyOptions) -> Result<VerificationRun, String> {
    let (context, manifest_before) = load_manifest(&options.manifest)?;
    let receipt_path = options
        .receipt_out
        .as_ref()
        .map(|path| prepare_receipt(&context, path))
        .transpose()?;
    if options.batch_id.is_some() && receipt_path.is_some() {
        return Err("--receipt-out is available only for a complete verification".to_owned());
    }
    let paths = report_paths(&options.reports)?;
    let report_snapshot_before = if receipt_path.is_some() {
        Some(snapshot_reports(&context)?)
    } else {
        None
    };
    let result = verify_result(&context, &paths, options)?;
    if result.get("ok") != Some(&json!(true)) {
        return Ok(VerificationRun {
            result,
            receipt: None,
        });
    }
    let Some(receipt_path) = receipt_path else {
        return Ok(VerificationRun {
            result,
            receipt: None,
        });
    };
    let manifest_after = read_bytes_nofollow(&context.path, None)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Audit manifest disappeared during verification".to_owned())?;
    if manifest_after != manifest_before {
        return Err(
            "Audit manifest changed during verification; refusing verification receipt.".to_owned(),
        );
    }
    let report_snapshot_after = snapshot_reports(&context)?;
    if report_snapshot_before.as_ref() != Some(&report_snapshot_after) {
        return Err(
            "Manifest-authorized reports changed during verification; refusing verification receipt."
                .to_owned(),
        );
    }
    let receipt = json!({
        "schema_version":1,"audit_kind":"full-repo-audit","run_id":context.run_id,
        "repo_root":context.repo_root.to_string_lossy(),
        "manifest_sha256":sha256_hex(&manifest_before),
        "reports_dir":context.reports_root.to_string_lossy(),
        "report_sha256":report_snapshot_after,
        "verifier_result_sha256":canonical_json_sha256(&result)?,
    });
    write_receipt_atomic(&receipt_path, &receipt)?;
    Ok(VerificationRun {
        result,
        receipt: Some(receipt),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_queue::{
        ArtifactOwnership, CollectOptions, FullRepoOutputOptions, audit_units_for, batch_files,
        collect_files, write_full_repo_outputs,
    };
    use std::process::Command;

    struct Fixture {
        _directory: tempfile::TempDir,
        repo: PathBuf,
        output: PathBuf,
        unit_id: String,
        digest: String,
    }

    fn git(root: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        let output = directory.path().join("audit");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::create_dir(repo.join("src")).unwrap();
        let source = b"pub fn alpha(value: u32) -> u32 { value + 1 }\n";
        std::fs::write(repo.join("src/lib.rs"), source).unwrap();
        git(&repo, &["add", "src/lib.rs"]);
        let collection = collect_files(
            &repo,
            &CollectOptions {
                include_config: true,
                ..Default::default()
            },
        );
        let units = audit_units_for(&repo, &collection.entries, 60_000);
        let unit_id = units[0].unit_id.clone();
        let digest = units[0].sha256.clone();
        let batches = batch_files(&units, 8, 60_000).unwrap();
        write_full_repo_outputs(
            &repo,
            &output,
            &collection,
            &units,
            &batches,
            "run-verify-1234",
            &FullRepoOutputOptions {
                generated_at: "2026-09-04T00:00:00Z".to_owned(),
                archive_stamp: "20260904T000000Z".to_owned(),
                verifier_program: PathBuf::from("/usr/local/bin/devcoordinator2-tooling"),
                ownership: ArtifactOwnership::default(),
            },
        )
        .unwrap();
        Fixture {
            _directory: directory,
            repo,
            output,
            unit_id,
            digest,
        }
    }

    fn batch_report(fixture: &Fixture, anchor: &str) -> String {
        format!(
            "## Run ID\nrun-verify-1234\n\n## Batch ID\nbatch_001\n\n## Batch Summary\nThe Rust library exposes one deterministic arithmetic responsibility.\n\n## File Coverage\n| File | Status | SHA-256 | Purpose |\n| --- | --- | --- | --- |\n| `{}` | CHECKED | `{}` | Implements the alpha increment calculation. |\n\n## Implementation Inventory\n| File/unit | Contract ID | Contract/responsibility | Entrypoints/source anchors | Implementation/data/side-effect trace | Failure/edge/permission/recovery trace | Verification evidence | Result |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| `{}` | `batch_001:C001` | Basis: source-inferred — `src/lib.rs#alpha` Discovery: parsed — `{anchor}` | `src/lib.rs` and `{anchor}` | pass — `{anchor}` returns the incremented numeric value to its caller | not applicable — unsigned fixture arithmetic has no external permission or recovery boundary | pass — evidence-type: source-only; `{anchor}`; invariance: the same input always returns the same incremented value; source inspection verifies the return expression | PASS |\n\n## Interface Inventory\nNo interface-relevant files in this batch.\n\n## Findings\nNo findings.\n\n## No Finding Notes\n`{}` was checked and its alpha responsibility is implemented.\n\n## Open Questions\nNone.\n",
            fixture.unit_id, fixture.digest, fixture.unit_id, fixture.unit_id
        )
    }

    fn lead_report(anchor: &str) -> String {
        format!(
            "## Run ID\nrun-verify-1234\n\n## Worker\nlead_reconciliation\n\n## Cross-File Contract Trace\n| Contract ID | Batch Contract IDs | Contract/source anchors | entry-registration | core-logic | data-lifecycle | integration-boundary | authorization-trust | failure-recovery | observable-outcome | operational-lifecycle | verification | Result |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n| `lead:C001` | `batch_001:C001` | `src/lib.rs` and `{anchor}` | pass — `{anchor}` is the public fixture entry point | pass — `{anchor}` calculates the increment from the supplied value | not applicable — the pure arithmetic function owns no mutable lifecycle | not applicable — the fixture has no dependency boundary | not applicable — the pure function has no authorization boundary | not applicable — unsigned arithmetic has no recoverable dependency failure | pass — `{anchor}` returns the calculated increment to the direct caller | not applicable — the pure function has no operational lifecycle | pass — evidence-type: source-only; `{anchor}` verifies the return expression; invariance: identical input always yields the same calculated increment | PASS |\n\n## Findings\nNo findings.\n\n## Open Questions\nNone.\n"
        )
    }

    fn complete_effort(fixture: &Fixture) {
        let path = fixture.output.join("effort_ledger.json");
        let mut ledger: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        ledger["subagent_capability_check"] = json!({
            "status":"completed","spawn_tool":"fixture","can_set_reasoning_effort":true,
            "claim_basis":"self-reported","claim_label":"ledger-recorded-unverified",
            "evidence":"fixture runner capability was inspected","notes":"fixture",
        });
        ledger["lead"] = json!({
            "status":"completed","required_reasoning_effort":"xhigh","actual_reasoning_effort":"xhigh",
            "agent_id":"lead-fixture","effort_claim_basis":"self-reported",
            "effort_claim_label":"ledger-recorded-unverified","runtime_provenance":"fixture runner",
            "notes":Value::Null,
        });
        ledger["lead_reconciliation"]["status"] = json!("completed");
        ledger["batches"][0]["status"] = json!("completed");
        ledger["batches"][0]["agent_id"] = json!("agent-1");
        ledger["batches"][0]["actual_reasoning_effort"] = json!("low");
        ledger["batches"][0]["runtime_provenance"] = json!("fixture worker");
        ledger["batches"][0]["effort_claim_basis"] = json!("self-reported");
        ledger["batches"][0]["effort_claim_label"] = json!("ledger-recorded-unverified");
        crate::audit_queue::write_json(&path, &ledger).unwrap();
    }

    fn install_reports(fixture: &Fixture, anchor: &str) {
        std::fs::write(
            fixture.output.join("reports/batch_001.md"),
            batch_report(fixture, anchor),
        )
        .unwrap();
        std::fs::write(
            fixture.output.join("reports/lead_reconciliation.md"),
            lead_report(anchor),
        )
        .unwrap();
    }

    #[test]
    fn complete_verification_publishes_hash_bound_receipt() {
        let fixture = fixture();
        install_reports(&fixture, "alpha@L1:C8");
        complete_effort(&fixture);
        let receipt = fixture.output.join("verification_receipt.json");
        let run = verify(&VerifyOptions {
            manifest: fixture.output.join("manifest.json"),
            reports: vec![fixture.output.join("reports")],
            batch_id: None,
            skip_current_hash_check: false,
            receipt_out: Some(receipt.clone()),
        })
        .unwrap();
        assert_eq!(run.result["ok"], true, "{:#}", run.result);
        assert!(run.receipt.is_some());
        assert!(receipt.is_file());
        let receipt_value: Value =
            serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
        assert_eq!(receipt_value["audit_kind"], "full-repo-audit");
        assert_eq!(receipt_value["report_sha256"].as_object().unwrap().len(), 2);
    }

    #[test]
    fn batch_scope_does_not_require_lead_or_completed_effort() {
        let fixture = fixture();
        std::fs::write(
            fixture.output.join("reports/batch_001.md"),
            batch_report(&fixture, "alpha@L1:C8"),
        )
        .unwrap();
        let run = verify(&VerifyOptions {
            manifest: fixture.output.join("manifest.json"),
            reports: vec![fixture.output.join("reports/batch_001.md")],
            batch_id: Some("batch_001".to_owned()),
            skip_current_hash_check: false,
            receipt_out: None,
        })
        .unwrap();
        assert_eq!(run.result["ok"], true, "{:#}", run.result);
        assert_eq!(run.result["verification_scope"], "batch:batch_001");
    }

    #[test]
    fn wrong_anchor_and_source_drift_fail_closed_and_remove_prior_receipt() {
        let fixture = fixture();
        install_reports(&fixture, "alpha@L1:C99");
        complete_effort(&fixture);
        let receipt = fixture.output.join("verification_receipt.json");
        std::fs::write(&receipt, "stale").unwrap();
        let run = verify(&VerifyOptions {
            manifest: fixture.output.join("manifest.json"),
            reports: vec![fixture.output.join("reports")],
            batch_id: None,
            skip_current_hash_check: false,
            receipt_out: Some(receipt.clone()),
        })
        .unwrap();
        assert_eq!(run.result["ok"], false);
        assert!(!receipt.exists());
        assert!(
            !run.result["implementation_inventory_issues"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        install_reports(&fixture, "alpha@L1:C8");
        std::fs::write(
            fixture.repo.join("src/lib.rs"),
            "pub fn alpha(value: u32) -> u32 { value + 2 }\n",
        )
        .unwrap();
        let run = verify(&VerifyOptions {
            manifest: fixture.output.join("manifest.json"),
            reports: vec![fixture.output.join("reports")],
            batch_id: None,
            skip_current_hash_check: false,
            receipt_out: None,
        })
        .unwrap();
        assert_eq!(run.result["ok"], false);
        assert!(
            !run.result["current_hash_mismatches"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    fn verify_batch_text(fixture: &Fixture, text: &str) -> Value {
        let report = fixture.output.join("reports/batch_001.md");
        std::fs::write(&report, text).unwrap();
        verify(&VerifyOptions {
            manifest: fixture.output.join("manifest.json"),
            reports: vec![report],
            batch_id: Some("batch_001".to_owned()),
            skip_current_hash_check: false,
            receipt_out: None,
        })
        .unwrap()
        .result
    }

    #[test]
    fn semantic_contract_mutations_fail_while_one_atomic_gap_is_accepted() {
        let fixture = fixture();
        let valid = batch_report(&fixture, "alpha@L1:C8");
        assert_eq!(verify_batch_text(&fixture, &valid)["ok"], true);

        for mutated in [
            valid.replace("Basis: source-inferred", "Basis: guess"),
            valid.replace("Discovery: parsed", "Discovery: manual"),
            valid.replace("evidence-type: source-only; ", ""),
            valid.replace(
                "returns the incremented numeric value to its caller",
                "persists the incremented numeric value to durable storage",
            ),
            valid.replace("`alpha@L1:C8`", "`missing@L1:C99`"),
        ] {
            let result = verify_batch_text(&fixture, &mutated);
            assert_eq!(result["ok"], false, "mutation escaped: {mutated}");
            assert!(
                !result["implementation_inventory_issues"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }

        let gap = valid
            .replace(
                "pass — `alpha@L1:C8` returns the incremented numeric value to its caller",
                "gap — `alpha@L1:C8` returns a fixed substitute instead of the requested calculation",
            )
            .replace(
                "| PASS |\n\n## Interface Inventory",
                "| GAP |\n\n## Interface Inventory",
            )
            .replace(
                "## Findings\nNo findings.",
                "## Findings\n### P1 - Alpha calculation is incomplete\n- Files: `src/lib.rs`\n- Evidence: `batch_001:C001` and `alpha@L1:C8` return a fixed substitute for every input.\n- Interface evidence: Not applicable\n- Expected behavior/standard: Calculate the increment from the supplied numeric value.\n- Gap: The function does not calculate the promised varying result.\n- Suggested direction: Implement the calculation and verify varied inputs end to end.",
            );
        assert_eq!(verify_batch_text(&fixture, &gap)["ok"], true);
    }

    #[test]
    fn placeholder_source_requires_marker_specific_finding_even_with_fresh_hashes() {
        let fixture = fixture();
        let source = b"pub fn alpha(value: u32) -> u32 { value + 1 } // TODO finish alpha\n";
        std::fs::write(fixture.repo.join("src/lib.rs"), source).unwrap();
        let digest = sha256_hex(source);
        let manifest_path = fixture.output.join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["source_files"][0]["sha256"] = json!(digest);
        manifest["coverage_units"][0]["sha256"] = json!(digest);
        crate::audit_queue::write_json(&manifest_path, &manifest).unwrap();
        let mut report = batch_report(&fixture, "alpha@L1:C8");
        report = report.replace(&fixture.digest, &digest);
        let result = verify_batch_text(&fixture, &report);
        assert_eq!(result["ok"], false);
        assert!(
            !result["placeholder_omissions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
