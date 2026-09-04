//! Deterministic, receipt-bound consolidation of audit findings.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::audit_ledger::{
    LedgerRow, read_text_nofollow, validate_directory_nofollow, write_bytes_nofollow,
};

static SECTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+(.+?)\s*$").expect("constant section regex"));
static FINDINGS_SECTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(finding|gap)s?\b").expect("constant findings-section regex")
});
static HEADING_FINDING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^###\s+(P[0-3])\s*[-–—:]\s*(.+?)\s*$").expect("constant heading-finding regex")
});
static PRIORITY_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^-\s*Priority\s*:\s*(P[0-3])\b").expect("constant priority-field regex")
});
static FIELD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-\s*([^:]+?)\s*:\s*(.*)$").expect("constant field regex"));
static PATH_IN_BACKTICKS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`]+)`").expect("constant path regex"));
static SHA256_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").expect("constant SHA-256 regex"));

const NO_FINDINGS_SENTINELS: [&str; 4] = ["no findings.", "no findings", "none.", "none"];
const RECEIPT_KEYS: [&str; 8] = [
    "schema_version",
    "audit_kind",
    "run_id",
    "repo_root",
    "manifest_sha256",
    "reports_dir",
    "report_sha256",
    "verifier_result_sha256",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Finding {
    pub priority: String,
    pub summary: String,
    pub files: Vec<String>,
    pub evidence: String,
    pub expected_behavior: String,
    pub gap: String,
    pub suggested_direction: String,
    pub sources: Vec<String>,
    pub candidate_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergeResult {
    pub reports_scanned: usize,
    pub report_sha256: BTreeMap<String, String>,
    pub ignored_unverified_reports: Vec<String>,
    pub raw_findings: usize,
    pub unique_findings: usize,
    pub priority_counts: BTreeMap<String, usize>,
    pub findings: Vec<Finding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectionCandidate {
    pub candidate_id: String,
    pub priority: String,
    pub summary: String,
    pub files: Vec<String>,
    pub evidence: String,
    pub expected_behavior: String,
    pub gap: String,
    pub suggested_direction: String,
    pub source_reports: Vec<String>,
    pub disposition: String,
    pub disposition_reason: String,
    pub ledger_row: LedgerRow,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionLedgerProjection {
    pub schema_version: u32,
    pub run_id: String,
    pub repo_root: String,
    pub manifest_sha256: String,
    pub consolidated_findings_sha256: String,
    pub review_status: String,
    pub review_instructions: String,
    pub candidates: Vec<ProjectionCandidate>,
}

#[derive(Clone, Debug, Default)]
struct ParsedFinding {
    priority: String,
    summary: String,
    fields: BTreeMap<String, String>,
    files: Vec<String>,
    evidence: String,
    expected_behavior: String,
    gap: String,
    suggested_direction: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FindingIdentity {
    files: Vec<String>,
    summary: String,
    evidence: String,
    expected_behavior: String,
    gap: String,
    suggested_direction: String,
}

impl From<&ParsedFinding> for FindingIdentity {
    fn from(finding: &ParsedFinding) -> Self {
        Self {
            files: finding.files.clone(),
            summary: normalize(&finding.summary),
            evidence: normalize(&finding.evidence),
            expected_behavior: normalize(&finding.expected_behavior),
            gap: normalize(&finding.gap),
            suggested_direction: normalize(&finding.suggested_direction),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MergeCommandOptions {
    pub reports: PathBuf,
    pub json_out: Option<PathBuf>,
    pub markdown_out: Option<PathBuf>,
    pub manifest: Option<PathBuf>,
    pub verification_receipt: Option<PathBuf>,
    pub ledger_projection_out: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct MergeCommandResult {
    pub result: MergeResult,
    pub projection: Option<CompletionLedgerProjection>,
}

fn priority_rank(priority: &str) -> u8 {
    match priority {
        "P0" => 0,
        "P1" => 1,
        "P2" => 2,
        "P3" => 3,
        _ => 9,
    }
}

fn section_blocks(text: &str) -> Vec<(String, String)> {
    let mut blocks: Vec<(String, Vec<String>)> = Vec::new();
    let mut current = None;
    for line in text.lines() {
        if let Some(captures) = SECTION_RE.captures(line.trim()) {
            let title = captures[1].trim().to_owned();
            current = blocks.iter().position(|(existing, _)| existing == &title);
            if current.is_none() {
                blocks.push((title, Vec::new()));
                current = Some(blocks.len() - 1);
            }
        } else if let Some(index) = current {
            blocks[index].1.push(line.to_owned());
        }
    }
    blocks
        .into_iter()
        .map(|(title, lines)| (title, lines.join("\n").trim().to_owned()))
        .collect()
}

fn is_findings_section(title: &str) -> bool {
    let lowered = title.to_lowercase();
    !lowered.contains("no gap")
        && !lowered.contains("no finding")
        && FINDINGS_SECTION_RE.is_match(&lowered)
}

fn split_files(value: &str) -> Vec<String> {
    let captured = PATH_IN_BACKTICKS_RE
        .captures_iter(value)
        .map(|capture| capture[1].to_owned())
        .collect::<Vec<_>>();
    let candidates = if captured.is_empty() {
        value
            .split([',', ';'])
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        captured
    };
    candidates
        .into_iter()
        .filter_map(|candidate| {
            let cleaned = candidate.trim().trim_matches('`').trim();
            (!cleaned.is_empty()
                && !matches!(
                    cleaned.to_lowercase().as_str(),
                    "none" | "n/a" | "not applicable"
                ))
            .then(|| cleaned.to_owned())
        })
        .collect()
}

fn finish_parsed(mut finding: ParsedFinding) -> ParsedFinding {
    if finding.summary.is_empty() {
        finding.summary = finding
            .fields
            .get("summary")
            .or_else(|| finding.fields.get("gap"))
            .or_else(|| finding.fields.get("evidence"))
            .or_else(|| finding.fields.get("missing scenarios/boundaries"))
            .cloned()
            .unwrap_or_else(|| "(no summary)".to_owned());
    }
    for key in ["files", "docs/files", "file"] {
        if let Some(value) = finding.fields.get(key) {
            finding.files = split_files(value);
            break;
        }
    }
    finding.evidence = finding
        .fields
        .get("evidence")
        .or_else(|| finding.fields.get("interface evidence"))
        .cloned()
        .unwrap_or_default();
    finding.expected_behavior = finding
        .fields
        .get("expected behavior/standard")
        .cloned()
        .unwrap_or_default();
    finding.gap = finding.fields.get("gap").cloned().unwrap_or_default();
    finding.suggested_direction = finding
        .fields
        .get("suggested direction")
        .or_else(|| finding.fields.get("suggested implementation direction"))
        .cloned()
        .unwrap_or_default();
    finding
}

fn parse_findings_from_section(body: &str) -> Vec<ParsedFinding> {
    let mut findings = Vec::new();
    let mut current: Option<ParsedFinding> = None;
    for raw in body.lines() {
        let line = raw.trim_end();
        if let Some(captures) = HEADING_FINDING_RE.captures(line.trim()) {
            if let Some(finding) = current.take() {
                findings.push(finish_parsed(finding));
            }
            current = Some(ParsedFinding {
                priority: captures[1].to_uppercase(),
                summary: captures[2].trim().to_owned(),
                ..Default::default()
            });
            continue;
        }
        if let Some(captures) = PRIORITY_FIELD_RE.captures(line.trim()) {
            if let Some(finding) = current.take() {
                findings.push(finish_parsed(finding));
            }
            current = Some(ParsedFinding {
                priority: captures[1].to_uppercase(),
                ..Default::default()
            });
            continue;
        }
        if let (Some(captures), Some(finding)) = (FIELD_RE.captures(line.trim()), current.as_mut())
        {
            finding.fields.insert(
                captures[1].trim().to_lowercase(),
                captures[2].trim().to_owned(),
            );
        }
    }
    if let Some(finding) = current {
        findings.push(finish_parsed(finding));
    }
    findings
}

fn normalize(value: &str) -> String {
    value.nfc().collect()
}

fn candidate_id_from_identity(identity: &FindingIdentity) -> String {
    let joined_files = identity.files.join("\0");
    let identity = [
        joined_files.as_str(),
        identity.summary.as_str(),
        identity.evidence.as_str(),
        identity.expected_behavior.as_str(),
        identity.gap.as_str(),
        identity.suggested_direction.as_str(),
    ]
    .join("\0");
    let digest = sha256_hex(identity.as_bytes());
    format!("FRA-C-{}", digest[..12].to_uppercase())
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

pub fn candidate_id(finding: &Finding) -> String {
    candidate_id_from_identity(&FindingIdentity {
        files: finding.files.clone(),
        summary: normalize(&finding.summary),
        evidence: normalize(&finding.evidence),
        expected_behavior: normalize(&finding.expected_behavior),
        gap: normalize(&finding.gap),
        suggested_direction: normalize(&finding.suggested_direction),
    })
}

fn direct_report_name(value: &str, field: &str) -> Result<String, String> {
    let Some(name) = value.strip_prefix("reports/") else {
        return Err(format!("manifest {field} must be a direct reports/ child"));
    };
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || Path::new(name).file_name().and_then(|part| part.to_str()) != Some(name)
    {
        return Err(format!("manifest {field} must be a direct reports/ child"));
    }
    Ok(name.to_owned())
}

pub fn manifest_report_names(manifest: &Map<String, Value>) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let batches = manifest
        .get("batches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for batch in batches {
        let id = batch
            .as_object()
            .and_then(|batch| batch.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| "manifest batches must contain string ids".to_owned())?;
        names.push(format!("{id}.md"));
    }
    if manifest
        .get("journey_audit")
        .and_then(Value::as_object)
        .and_then(|journey| journey.get("required"))
        == Some(&Value::Bool(true))
    {
        let journey = manifest["journey_audit"]
            .as_object()
            .expect("checked object");
        for field in ["source_report", "visual_report"] {
            let report = journey
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("manifest journey_audit.{field} must be a string"))?;
            names.push(direct_report_name(
                report,
                &format!("journey_audit.{field}"),
            )?);
        }
    }
    let lead = manifest
        .get("lead_reconciliation")
        .and_then(Value::as_object)
        .filter(|lead| lead.get("required") == Some(&Value::Bool(true)))
        .ok_or_else(|| {
            "manifest mode is full-repo-audit-only and requires lead_reconciliation; other audit skills must omit --manifest".to_owned()
        })?;
    let lead_report = lead
        .get("report")
        .and_then(Value::as_str)
        .ok_or_else(|| "manifest lead_reconciliation.report must be a string".to_owned())?;
    names.push(direct_report_name(
        lead_report,
        "lead_reconciliation.report",
    )?);
    let unique = names.iter().collect::<BTreeSet<_>>();
    if unique.len() != names.len() {
        return Err("manifest report allowlist contains duplicates".to_owned());
    }
    Ok(names)
}

fn read_report(path: &Path) -> Result<Vec<u8>, String> {
    read_text_nofollow(path, None)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("required verified report is missing: {}", path.display()))
        .map(String::into_bytes)
}

pub fn merge_findings(
    reports_dir: &Path,
    report_names: Option<&[String]>,
) -> Result<MergeResult, String> {
    let reports_dir =
        validate_directory_nofollow(reports_dir).map_err(|error| error.to_string())?;
    let mut all_markdown = std::fs::read_dir(&reports_dir)
        .map_err(|error| format!("could not list reports directory: {error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|value| value.to_str()) == Some("md")
                && entry.file_type().ok().is_some_and(|kind| kind.is_file()))
            .then_some(path)
        })
        .collect::<Vec<_>>();
    all_markdown.sort();
    let (report_paths, ignored_unverified_reports) = if let Some(names) = report_names {
        if names.iter().collect::<BTreeSet<_>>().len() != names.len() {
            return Err("report allowlist contains duplicates".to_owned());
        }
        let invalid = names
            .iter()
            .filter(|name| {
                !name.ends_with(".md")
                    || Path::new(name).file_name().and_then(|value| value.to_str())
                        != Some(name.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        if !invalid.is_empty() {
            return Err(format!(
                "report allowlist must contain direct Markdown basenames: {invalid:?}"
            ));
        }
        let paths = names
            .iter()
            .map(|name| reports_dir.join(name))
            .collect::<Vec<_>>();
        let missing = paths
            .iter()
            .filter_map(|path| match path.symlink_metadata() {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => None,
                _ => path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
            })
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "required verified reports are missing or symlinked: {missing:?}"
            ));
        }
        let allowed = names.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let ignored = all_markdown
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .filter(|name| !allowed.contains(name))
            .map(str::to_owned)
            .collect();
        (paths, ignored)
    } else {
        (all_markdown, Vec::new())
    };

    let mut merged: BTreeMap<FindingIdentity, Finding> = BTreeMap::new();
    let mut raw_findings = 0;
    let mut report_sha256 = BTreeMap::new();
    for report_path in &report_paths {
        let bytes = read_report(report_path).map_err(|error| {
            format!(
                "could not read report {}: {error}",
                report_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            )
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            format!(
                "could not read report {}: {error}",
                report_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            )
        })?;
        let report_name = report_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "report name is not valid UTF-8".to_owned())?
            .to_owned();
        report_sha256.insert(report_name.clone(), sha256_hex(&bytes));
        for (title, body) in section_blocks(text) {
            if !is_findings_section(&title)
                || NO_FINDINGS_SENTINELS.contains(&body.trim().to_lowercase().as_str())
            {
                continue;
            }
            for finding in parse_findings_from_section(&body) {
                raw_findings += 1;
                let identity = FindingIdentity::from(&finding);
                if let Some(existing) = merged.get_mut(&identity) {
                    existing.sources.push(report_name.clone());
                    if priority_rank(&finding.priority) < priority_rank(&existing.priority) {
                        existing.priority = finding.priority;
                    }
                } else {
                    merged.insert(
                        identity,
                        Finding {
                            priority: finding.priority,
                            summary: finding.summary,
                            files: finding.files,
                            evidence: finding.evidence,
                            expected_behavior: finding.expected_behavior,
                            gap: finding.gap,
                            suggested_direction: finding.suggested_direction,
                            sources: vec![report_name.clone()],
                            candidate_id: String::new(),
                        },
                    );
                }
            }
        }
    }
    let mut findings = merged.into_values().collect::<Vec<_>>();
    findings.sort_by(|left, right| {
        (
            priority_rank(&left.priority),
            left.files.first().map(String::as_str).unwrap_or(""),
            left.summary.as_str(),
        )
            .cmp(&(
                priority_rank(&right.priority),
                right.files.first().map(String::as_str).unwrap_or(""),
                right.summary.as_str(),
            ))
    });
    let mut priority_counts = ["P0", "P1", "P2", "P3"]
        .into_iter()
        .map(|priority| (priority.to_owned(), 0usize))
        .collect::<BTreeMap<_, _>>();
    for finding in &mut findings {
        finding.candidate_id = candidate_id(finding);
        *priority_counts.entry(finding.priority.clone()).or_default() += 1;
    }
    Ok(MergeResult {
        reports_scanned: report_paths.len(),
        report_sha256,
        ignored_unverified_reports,
        raw_findings,
        unique_findings: findings.len(),
        priority_counts,
        findings,
    })
}

fn canonical_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_value).collect()),
        Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_value(value)))
                    .collect(),
            )
        }
        other => other,
    }
}

pub fn canonical_json_sha256<T: Serialize>(value: &T) -> Result<String, String> {
    let value = serde_json::to_value(value).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&canonical_value(value)).map_err(|error| error.to_string())?;
    Ok(sha256_hex(&bytes))
}

pub fn render_completion_ledger_projection(
    result: &MergeResult,
    run_id: &str,
    repo_root: &str,
    manifest_sha256: &str,
) -> Result<CompletionLedgerProjection, String> {
    let candidates = result
        .findings
        .iter()
        .map(|finding| {
            let suffix = finding
                .candidate_id
                .strip_prefix("FRA-C-")
                .unwrap_or(&finding.candidate_id);
            let mut source_reports = finding.sources.clone();
            source_reports.sort();
            source_reports.dedup();
            ProjectionCandidate {
                candidate_id: finding.candidate_id.clone(),
                priority: finding.priority.clone(),
                summary: finding.summary.clone(),
                files: finding.files.clone(),
                evidence: finding.evidence.clone(),
                expected_behavior: finding.expected_behavior.clone(),
                gap: finding.gap.clone(),
                suggested_direction: finding.suggested_direction.clone(),
                source_reports,
                disposition: "pending".to_owned(),
                disposition_reason: String::new(),
                ledger_row: LedgerRow {
                    id: format!("FRA-{suffix}"),
                    remaining_work: String::new(),
                    why_it_matters: String::new(),
                    status: "Open".to_owned(),
                    verification: String::new(),
                },
            }
        })
        .collect();
    Ok(CompletionLedgerProjection {
        schema_version: 1,
        run_id: run_id.to_owned(),
        repo_root: repo_root.to_owned(),
        manifest_sha256: manifest_sha256.to_owned(),
        consolidated_findings_sha256: canonical_json_sha256(result)?,
        review_status: "pending".to_owned(),
        review_instructions: "Review every candidate. Set disposition to confirmed, duplicate, hypothesis, invalid, or out_of_scope; complete active ledger fields only for confirmed candidates and explain every excluded or duplicate disposition. Candidates must already be atomic; if one is compound, reissue atomic findings in an authorized report, rerun verification and consolidation, and review the regenerated projection.".to_owned(),
        candidates,
    })
}

pub fn render_markdown(result: &MergeResult) -> String {
    let count = |priority: &str| result.priority_counts.get(priority).copied().unwrap_or(0);
    let mut lines = vec![
        "# Consolidated Audit Findings".to_owned(),
        String::new(),
        format!(
            "{} unique findings from {} raw findings across {} reports (P0 {}, P1 {}, P2 {}, P3 {}).",
            result.unique_findings,
            result.raw_findings,
            result.reports_scanned,
            count("P0"),
            count("P1"),
            count("P2"),
            count("P3")
        ),
        String::new(),
    ];
    let mut current_priority: Option<&str> = None;
    for finding in &result.findings {
        if current_priority != Some(&finding.priority) {
            current_priority = Some(&finding.priority);
            lines.push(format!("## {}", finding.priority));
            lines.push(String::new());
        }
        let files = if finding.files.is_empty() {
            "_no file cited_".to_owned()
        } else {
            finding
                .files
                .iter()
                .map(|path| format!("`{path}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut sources = finding.sources.clone();
        sources.sort();
        sources.dedup();
        lines.push(format!("- **{}**", finding.summary));
        lines.push(format!("  - Files: {files}"));
        if !finding.evidence.is_empty() {
            lines.push(format!("  - Evidence: {}", finding.evidence));
        }
        lines.push(format!("  - Reported by: {}", sources.join(", ")));
        lines.push(String::new());
    }
    if result.findings.is_empty() {
        lines.push("_No findings reported across the scanned reports._".to_owned());
        lines.push(String::new());
    }
    lines.join("\n")
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON value without duplicate object keys")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(value)))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(Number::from(value))))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(Number::from(value))))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Number::from_f64(value)
                    .map(Value::Number)
                    .map(StrictValue)
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value)))
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }

            fn visit_some<D: Deserializer<'de>>(self, value: D) -> Result<Self::Value, D::Error> {
                StrictValue::deserialize(value)
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(StrictValue(value)) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(StrictValue(Value::Array(values)))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut value = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if value.contains_key(&key) {
                        return Err(de::Error::custom(format!(
                            "contains duplicate JSON key {key:?}"
                        )));
                    }
                    let StrictValue(item) = map.next_value()?;
                    value.insert(key, item);
                }
                Ok(StrictValue(Value::Object(value)))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

pub fn strict_json_object(data: &[u8], label: &str) -> Result<Map<String, Value>, String> {
    let text = std::str::from_utf8(data)
        .map_err(|error| format!("{label} is not valid UTF-8 JSON: {error}"))?;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let StrictValue(parsed) = StrictValue::deserialize(&mut deserializer)
        .map_err(|error| format!("{label} is not valid UTF-8 JSON: {error}"))?;
    deserializer
        .end()
        .map_err(|error| format!("{label} is not valid UTF-8 JSON: {error}"))?;
    parsed
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{label} must be a JSON object"))
}

fn read_stable_regular_file(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    read_text_nofollow(path, None)
        .map_err(|error| format!("could not read {label} {}: {error}", path.display()))?
        .ok_or_else(|| format!("could not read {label} {}: file not found", path.display()))
        .map(String::into_bytes)
}

fn validate_verification_receipt(
    receipt: &Map<String, Value>,
    manifest: &Map<String, Value>,
    manifest_sha256: &str,
    reports_dir: &Path,
    report_names: &[String],
) -> Result<BTreeMap<String, String>, String> {
    let actual_keys = receipt.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected_keys = RECEIPT_KEYS.into_iter().collect::<BTreeSet<_>>();
    if actual_keys != expected_keys {
        let missing = expected_keys
            .difference(&actual_keys)
            .copied()
            .collect::<Vec<_>>();
        let extra = actual_keys
            .difference(&expected_keys)
            .copied()
            .collect::<Vec<_>>();
        return Err(format!(
            "verification receipt keys must be exact; missing={missing:?}, extra={extra:?}"
        ));
    }
    let expected = [
        ("schema_version", Value::Number(Number::from(1))),
        ("audit_kind", Value::String("full-repo-audit".to_owned())),
        (
            "run_id",
            manifest.get("run_id").cloned().unwrap_or(Value::Null),
        ),
        (
            "repo_root",
            manifest.get("repo_root").cloned().unwrap_or(Value::Null),
        ),
        ("manifest_sha256", Value::String(manifest_sha256.to_owned())),
        (
            "reports_dir",
            Value::String(reports_dir.to_string_lossy().into_owned()),
        ),
    ];
    for (field, expected) in expected {
        if receipt.get(field) != Some(&expected) {
            return Err(format!(
                "verification receipt {field} does not match the audit manifest/report root"
            ));
        }
    }
    let verifier_hash = receipt
        .get("verifier_result_sha256")
        .and_then(Value::as_str)
        .filter(|value| SHA256_RE.is_match(value))
        .ok_or_else(|| {
            "verification receipt verifier_result_sha256 must be a lowercase SHA-256 digest"
                .to_owned()
        })?;
    let _ = verifier_hash;
    let hashes = receipt
        .get("report_sha256")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            "verification receipt report_sha256 must bind every authorized report exactly once"
                .to_owned()
        })?;
    let expected_names = report_names
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let hash_names = hashes.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if expected_names != hash_names {
        return Err(
            "verification receipt report_sha256 must bind every authorized report exactly once"
                .to_owned(),
        );
    }
    hashes
        .iter()
        .map(|(name, value)| {
            value
                .as_str()
                .filter(|value| SHA256_RE.is_match(value))
                .map(|value| (name.clone(), value.to_owned()))
                .ok_or_else(|| {
                    "verification receipt report hashes must be lowercase SHA-256 digests"
                        .to_owned()
                })
        })
        .collect()
}

fn write_output(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_bytes_nofollow(path, bytes, 0o600).map_err(|error| error.to_string())
}

fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(value).map_err(|error| error.to_string())?;
    let mut bytes =
        serde_json::to_vec_pretty(&canonical_value(value)).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn run_merge_command(options: &MergeCommandOptions) -> Result<MergeCommandResult, String> {
    let reports_dir =
        validate_directory_nofollow(&options.reports).map_err(|error| error.to_string())?;
    let mut manifest_path = None;
    let mut manifest_bytes = None;
    let mut receipt_path = None;
    let mut receipt_bytes = None;
    let mut manifest = None;
    let mut manifest_sha256 = None;
    let mut report_names = None;
    let mut receipt_report_hashes = None;

    if let Some(path) = &options.manifest {
        let bytes = read_stable_regular_file(path, "audit manifest")?;
        let parsed = strict_json_object(&bytes, "audit manifest")?;
        let digest = sha256_hex(&bytes);
        let names = manifest_report_names(&parsed)?;
        let declared_reports = parsed
            .get("reports_dir")
            .and_then(Value::as_str)
            .ok_or_else(|| "audit manifest reports_dir must be a string".to_owned())?;
        let declared_reports = validate_directory_nofollow(Path::new(declared_reports))
            .map_err(|error| error.to_string())?;
        let parent = path
            .parent()
            .ok_or_else(|| "audit manifest has no parent directory".to_owned())?;
        let expected_reports = validate_directory_nofollow(&parent.join("reports"))
            .map_err(|error| error.to_string())?;
        if declared_reports != reports_dir || expected_reports != reports_dir {
            return Err(
                "--reports must be the exact non-symlinked reports directory owned by the manifest"
                    .to_owned(),
            );
        }
        let selected_receipt = options
            .verification_receipt
            .clone()
            .unwrap_or_else(|| parent.join("verification_receipt.json"));
        let selected_receipt_bytes =
            read_stable_regular_file(&selected_receipt, "verification receipt")?;
        let receipt = strict_json_object(&selected_receipt_bytes, "verification receipt")?;
        receipt_report_hashes = Some(validate_verification_receipt(
            &receipt,
            &parsed,
            &digest,
            &reports_dir,
            &names,
        )?);
        manifest_path = Some(path.clone());
        manifest_bytes = Some(bytes);
        receipt_path = Some(selected_receipt);
        receipt_bytes = Some(selected_receipt_bytes);
        manifest = Some(parsed);
        manifest_sha256 = Some(digest);
        report_names = Some(names);
    } else if options.verification_receipt.is_some() {
        return Err("--verification-receipt requires --manifest".to_owned());
    }

    let result = merge_findings(&reports_dir, report_names.as_deref())?;
    if receipt_report_hashes
        .as_ref()
        .is_some_and(|hashes| hashes != &result.report_sha256)
    {
        return Err(
            "verified audit reports changed after receipt creation; rerun the verifier".to_owned(),
        );
    }
    let projection = if options.ledger_projection_out.is_some() {
        let manifest = manifest
            .as_ref()
            .ok_or_else(|| "--ledger-projection-out requires --manifest".to_owned())?;
        let run_id = manifest
            .get("run_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "audit manifest must contain non-empty run_id and repo_root".to_owned()
            })?;
        let repo_root = manifest
            .get("repo_root")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "audit manifest must contain non-empty run_id and repo_root".to_owned()
            })?;
        Some(render_completion_ledger_projection(
            &result,
            run_id,
            repo_root,
            manifest_sha256.as_deref().expect("manifest digest"),
        )?)
    } else {
        None
    };

    if let (Some(path), Some(before)) = (&manifest_path, &manifest_bytes)
        && read_stable_regular_file(path, "audit manifest")? != *before
    {
        return Err("audit manifest or verification receipt changed before publication".to_owned());
    }
    if let (Some(path), Some(before)) = (&receipt_path, &receipt_bytes)
        && read_stable_regular_file(path, "verification receipt")? != *before
    {
        return Err("audit manifest or verification receipt changed before publication".to_owned());
    }
    if let Some(path) = &options.json_out {
        write_output(path, &pretty_json(&result)?)?;
    }
    if let Some(path) = &options.markdown_out {
        let mut markdown = render_markdown(&result).into_bytes();
        markdown.push(b'\n');
        write_output(path, &markdown)?;
    }
    if let (Some(path), Some(projection)) = (&options.ledger_projection_out, &projection) {
        write_output(path, &pretty_json(projection)?)?;
    }
    Ok(MergeCommandResult { result, projection })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BATCH_ONE: &str = r#"## File-Level Findings
### P1 - Save button uses placeholder console-only behavior
- Files: `src/SaveButton.tsx`
- Evidence: onClick only calls console.log
- Expected behavior/standard: save changes durably
- Gap: no persistence.
- Suggested direction: wire the handler.

### P2 - Minor copy inconsistency
- Files: `src/Header.tsx`
- Evidence: title casing differs.
- Gap: cosmetic.
"#;
    const BATCH_TWO: &str = r#"## Implementation Gap Findings
- Priority: P1
- Files: `src/SaveButton.tsx`
- Summary: Save button uses placeholder console-only behavior
- Evidence: onClick only calls console.log
- Expected behavior/standard: save changes durably
- Gap: no persistence.
- Suggested implementation direction: wire the handler.

- Priority: P0
- Files: `src/auth/login.ts`
- Evidence: password compared with ==
- Gap: auth bypass risk
"#;
    const BATCH_THREE: &str = "## Coverage Findings\nNo findings.\n";
    const BATCH_FOUR: &str = r#"## File-Level Findings
### P2 - This deliberately long implementation finding summary shares the same first eighty characters but ends in calculation path A
- Files: `src/calculate.py`
- Evidence: branch A returns a fixed value for all inputs.
- Gap: calculation A is not implemented.

### P2 - This deliberately long implementation finding summary shares the same first eighty characters but ends in persistence path B
- Files: `src/calculate.py`
- Evidence: branch B reports success without durable storage.
- Gap: persistence B is not implemented.

### P2 - Repeated summary and gap must retain distinct create evidence
- Files: `src/store.py`
- Evidence: create_item returns success without writing the record.
- Expected behavior/standard: persist the requested mutation.
- Gap: the mutation is not persisted.
- Suggested direction: implement and verify durable storage.

### P2 - Repeated summary and gap must retain distinct create evidence
- Files: `src/store.py`
- Evidence: delete_item returns success without deleting the record.
- Expected behavior/standard: persist the requested mutation.
- Gap: the mutation is not persisted.
- Suggested direction: implement and verify durable storage.
"#;
    const LEAD: &str = r#"## Findings
### P1 - Registration never reaches the scheduled worker
- Files: `src/jobs.py`, `src/worker.py`
- Evidence: register_job stores a name that worker dispatch never reads.
- Interface evidence: Not applicable
- Expected behavior/standard: registered jobs must be dispatched.
- Gap: the cross-file registration and dispatch contract is disconnected.
- Suggested direction: use one registry and exercise a real scheduled run.
"#;

    fn write(path: &Path, value: &str) {
        std::fs::write(path, value).unwrap();
    }

    #[test]
    fn merges_both_report_forms_conservatively() {
        let directory = tempfile::tempdir().unwrap();
        write(&directory.path().join("batch_001.md"), BATCH_ONE);
        write(&directory.path().join("batch_002.md"), BATCH_TWO);
        write(&directory.path().join("batch_003.md"), BATCH_THREE);
        write(&directory.path().join("batch_004.md"), BATCH_FOUR);
        write(&directory.path().join("lead_reconciliation.md"), LEAD);
        write(
            &directory.path().join("rogue.md"),
            "## Findings\n### P0 - injected\n- Files: `rogue.py`\n",
        );
        let names = [
            "batch_001.md",
            "batch_002.md",
            "batch_003.md",
            "batch_004.md",
            "lead_reconciliation.md",
        ]
        .map(str::to_owned)
        .to_vec();
        let result = merge_findings(directory.path(), Some(&names)).unwrap();
        assert_eq!(result.reports_scanned, 5);
        assert_eq!(result.raw_findings, 9);
        assert_eq!(result.unique_findings, 8);
        assert_eq!(result.ignored_unverified_reports, ["rogue.md"]);
        assert_eq!(result.findings[0].priority, "P0");
        assert_eq!(
            result.priority_counts,
            [("P0", 1), ("P1", 2), ("P2", 5), ("P3", 0)]
                .into_iter()
                .map(|(priority, count)| (priority.to_owned(), count))
                .collect()
        );
        assert_eq!(
            result
                .findings
                .iter()
                .map(|finding| finding.candidate_id.as_str())
                .collect::<Vec<_>>(),
            [
                "FRA-C-AE19AF00B78D",
                "FRA-C-B93BA3D964B4",
                "FRA-C-5A49B018CC77",
                "FRA-C-3727CE649251",
                "FRA-C-69E675F34BA3",
                "FRA-C-F3B430EF3AE3",
                "FRA-C-C9B8A4D885DF",
                "FRA-C-4228B5211E9E",
            ]
        );
        let save = result
            .findings
            .iter()
            .find(|finding| finding.files == ["src/SaveButton.tsx"])
            .unwrap();
        assert_eq!(save.sources, ["batch_001.md", "batch_002.md"]);
        assert!(save.candidate_id.starts_with("FRA-C-"));
        assert!(render_markdown(&result).contains("`src/SaveButton.tsx`"));
    }

    #[test]
    fn identity_keeps_unicode_and_operators_lossless_but_normalizes_nfc() {
        let base = Finding {
            priority: "P2".to_owned(),
            summary: "Café is unavailable".to_owned(),
            files: vec!["src/store.rs".to_owned()],
            evidence: "guard uses x == y".to_owned(),
            expected_behavior: String::new(),
            gap: String::new(),
            suggested_direction: String::new(),
            sources: Vec::new(),
            candidate_id: String::new(),
        };
        let mut decomposed = base.clone();
        decomposed.summary = "Cafe\u{301} is unavailable".to_owned();
        assert_eq!(candidate_id(&base), candidate_id(&decomposed));
        let mut unequal = base.clone();
        unequal.evidence = "guard uses x != y".to_owned();
        assert_ne!(candidate_id(&base), candidate_id(&unequal));
    }

    #[test]
    fn strict_json_rejects_duplicate_keys_at_any_depth() {
        assert!(strict_json_object(br#"{"a":{"b":1,"b":2}}"#, "fixture").is_err());
        assert!(strict_json_object(br#"[]"#, "fixture").is_err());
        assert!(strict_json_object(br#"{"a":1}"#, "fixture").is_ok());
    }

    #[test]
    fn manifest_requires_exact_direct_reports() {
        let manifest = serde_json::json!({
            "batches": [{"id":"batch_001"}],
            "journey_audit": {"required": false},
            "lead_reconciliation": {"required": true, "report":"reports/lead.md"}
        });
        assert_eq!(
            manifest_report_names(manifest.as_object().unwrap()).unwrap(),
            ["batch_001.md", "lead.md"]
        );
        let invalid = serde_json::json!({
            "batches": [],
            "lead_reconciliation": {"required": true, "report":"reports/nested/lead.md"}
        });
        assert!(manifest_report_names(invalid.as_object().unwrap()).is_err());
    }

    #[test]
    fn projection_is_review_only_and_hash_bound() {
        let directory = tempfile::tempdir().unwrap();
        write(&directory.path().join("batch.md"), BATCH_ONE);
        let result = merge_findings(directory.path(), None).unwrap();
        let projection =
            render_completion_ledger_projection(&result, "run-1", "/tmp/repo", &"a".repeat(64))
                .unwrap();
        assert_eq!(projection.review_status, "pending");
        assert_eq!(projection.candidates.len(), result.unique_findings);
        assert!(
            projection
                .candidates
                .iter()
                .all(|candidate| candidate.disposition == "pending"
                    && candidate.ledger_row.verification.is_empty())
        );
    }

    #[test]
    fn authorized_missing_and_symlinked_reports_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        write(&directory.path().join("real.md"), BATCH_ONE);
        assert!(merge_findings(directory.path(), Some(&["missing.md".to_owned()])).is_err());
        std::os::unix::fs::symlink(
            directory.path().join("real.md"),
            directory.path().join("linked.md"),
        )
        .unwrap();
        assert!(merge_findings(directory.path(), Some(&["linked.md".to_owned()])).is_err());
    }

    #[test]
    fn manifest_command_binds_receipt_reports_and_projection_before_publication() {
        let directory = tempfile::tempdir().unwrap();
        let reports = directory.path().join("reports");
        std::fs::create_dir(&reports).unwrap();
        write(&reports.join("batch_001.md"), BATCH_ONE);
        write(&reports.join("lead.md"), LEAD);
        let names = ["batch_001.md".to_owned(), "lead.md".to_owned()];
        let initial = merge_findings(&reports, Some(&names)).unwrap();
        let manifest = serde_json::json!({
            "run_id": "run-1",
            "repo_root": "/tmp/repo",
            "reports_dir": reports.to_string_lossy(),
            "batches": [{"id": "batch_001"}],
            "journey_audit": {"required": false},
            "lead_reconciliation": {"required": true, "report": "reports/lead.md"}
        });
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        manifest_bytes.push(b'\n');
        let manifest_path = directory.path().join("manifest.json");
        std::fs::write(&manifest_path, &manifest_bytes).unwrap();
        let receipt = serde_json::json!({
            "schema_version": 1,
            "audit_kind": "full-repo-audit",
            "run_id": "run-1",
            "repo_root": "/tmp/repo",
            "manifest_sha256": sha256_hex(&manifest_bytes),
            "reports_dir": reports.to_string_lossy(),
            "report_sha256": initial.report_sha256.clone(),
            "verifier_result_sha256": "b".repeat(64),
        });
        let receipt_path = directory.path().join("verification_receipt.json");
        std::fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let json_out = directory.path().join("findings.json");
        let markdown_out = directory.path().join("findings.md");
        let projection_out = directory.path().join("projection.json");
        let options = MergeCommandOptions {
            reports: reports.clone(),
            json_out: Some(json_out.clone()),
            markdown_out: Some(markdown_out.clone()),
            manifest: Some(manifest_path.clone()),
            verification_receipt: Some(receipt_path),
            ledger_projection_out: Some(projection_out.clone()),
        };
        let output = run_merge_command(&options).unwrap();
        assert_eq!(output.result, initial);
        assert_eq!(output.projection.unwrap().review_status, "pending");
        assert!(json_out.is_file());
        assert!(
            std::fs::read_to_string(markdown_out)
                .unwrap()
                .contains("Registration never reaches")
        );
        assert!(projection_out.is_file());

        write(&reports.join("batch_001.md"), LEAD);
        assert!(
            run_merge_command(&options)
                .unwrap_err()
                .contains("reports changed")
        );

        let linked = directory.path().join("linked-reports");
        std::os::unix::fs::symlink(&reports, &linked).unwrap();
        let linked_options = MergeCommandOptions {
            reports: linked,
            json_out: None,
            markdown_out: None,
            manifest: Some(manifest_path),
            verification_receipt: None,
            ledger_projection_out: None,
        };
        assert!(run_merge_command(&linked_options).is_err());
    }
}
