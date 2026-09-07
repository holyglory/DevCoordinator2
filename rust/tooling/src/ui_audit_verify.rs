//! Strict verifier for UI implementation audit artifacts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::audit_common::{
    duplicate_values, interaction_checklist_missing, iter_report_files, parse_markdown_table_dicts,
    section_bodies, section_order, sha256_file,
};
use crate::audit_ledger::{read_bytes_nofollow, validate_directory_nofollow};
use crate::audit_queue;
use crate::ui_gate::{self, ExplicitUiBasis};

const BATCH_SECTIONS: &[&str] = &[
    "run id",
    "batch id",
    "batch summary",
    "file coverage",
    "ui source inventory",
    "mockup and journey alignment",
    "implementation gap findings",
    "no gap notes",
    "open questions",
];
const MOCKUP_SECTIONS: &[&str] = &[
    "run id",
    "worker",
    "mockup/asset inputs",
    "journey requirement inputs",
    "expected screens and visual requirements",
    "findings",
    "open questions",
];
const TOOLING_SECTIONS: &[&str] = &[
    "run id",
    "worker",
    "tooling inventory",
    "safe run path",
    "desktop/mobile screenshot plan",
    "findings",
    "open questions",
];
const VISUAL_SECTIONS: &[&str] = &[
    "run id",
    "worker",
    "mockup and asset inventory",
    "visual tooling",
    "journey decision model",
    "journey flow review",
    "ux guideline review",
    "ui configuration contract",
    "interaction coverage",
    "rendered build review",
    "design decision review",
    "rendered journey usability",
    "visual comparison checks",
    "formal evidence",
    "findings",
    "open questions",
];
const FINAL_SECTIONS: &[&str] = &[
    "coverage",
    "mockup and requirement inputs",
    "journey decision model",
    "journey flow review",
    "ux guideline review",
    "ui configuration contract",
    "interaction coverage",
    "rendered build review",
    "design decision review",
    "rendered journey usability findings",
    "visual audit findings",
    "source implementation findings",
    "journey and responsive findings",
    "accessibility and interaction findings",
    "implementation plan",
    "verification plan",
];
const REQUIRED_FINDING_FIELDS: &[&str] = &[
    "Priority",
    "Files",
    "Mockup/requirement evidence",
    "Interface evidence",
    "Expected behavior/standard",
    "Gap",
    "Suggested implementation direction",
];
const JOURNEY_DECISION_COLUMNS: &[&str] = &[
    "journey id",
    "requirement evidence",
    "surface",
    "primary user goal",
    "primary decision",
    "required facts",
    "warning/flag conditions",
    "frequent actions",
    "secondary/rare actions",
    "unconfirmed assumptions",
];
const JOURNEY_FLOW_COLUMNS: &[&str] = &[
    "journey id",
    "starting situation",
    "intended outcome",
    "observed path",
    "surfaces",
    "outcome evidence",
    "unnecessary effort",
    "simpler alternative",
    "result",
    "reason",
    "evidence",
    "finding",
];
const UX_REVIEW_COLUMNS: &[&str] = &[
    "journey id",
    "criterion",
    "guideline source",
    "surface",
    "item",
    "user benefit",
    "observation",
    "result",
    "reason",
    "evidence",
    "finding",
];
const CONFIG_COLUMNS: &[&str] = &[
    "config id",
    "platform",
    "theme",
    "viewport",
    "input mode",
    "journey ids",
    "update journey",
    "requirement source",
];
const INTERACTION_COLUMNS: &[&str] = &[
    "journey id",
    "config id",
    "scenario",
    "target",
    "expected result",
    "observation",
    "result",
    "reason",
    "evidence",
    "finding",
];
const BUILD_COLUMNS: &[&str] = &[
    "config id",
    "target",
    "expected snapshot",
    "observed snapshot",
    "result",
    "reason",
    "evidence",
    "finding",
];
const DESIGN_COLUMNS: &[&str] = &[
    "review id",
    "journey ids",
    "applicability",
    "design target",
    "options",
    "selection",
    "authority",
    "distinctions",
    "result",
    "reason",
    "evidence",
    "finding",
];
const RENDERED_USABILITY_COLUMNS: &[&str] = &[
    "platform",
    "viewport",
    "decision supported",
    "visible decision-driving content",
    "visible secondary/detail content",
    "detail access pattern",
    "readability/contrast evidence",
    "layout quality result",
    "evidence",
];
const UI_SOURCE_COLUMNS: &[&str] = &[
    "unit",
    "file",
    "surface",
    "visible element",
    "source evidence",
    "expected behavior",
    "actual implementation",
    "handler reference",
    "backend/api reference",
    "permission reference",
    "persistence reference",
    "test reference",
    "responsive/state notes",
];
const VISUAL_COMPARISON_COLUMNS: &[&str] = &[
    "platform",
    "journey",
    "viewport",
    "route/screen",
    "mockup/requirement",
    "implementation screenshot/tool evidence",
    "differences",
    "result",
];

#[derive(Clone, Debug)]
struct VerifyContext {
    raw: Map<String, Value>,
    root: PathBuf,
    repo: PathBuf,
    source_hashes: BTreeMap<String, String>,
    asset_hashes: BTreeMap<String, String>,
    requirement_hashes: BTreeMap<String, String>,
    unit_to_file: BTreeMap<String, String>,
    unit_hashes: BTreeMap<String, String>,
    batches: Vec<Value>,
    expected_by_batch: BTreeMap<String, BTreeSet<String>>,
    files_by_batch: BTreeMap<String, BTreeSet<String>>,
    platform: String,
    formal_config: Option<Value>,
}

fn sha(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn string_set(value: Option<&Value>, label: &str) -> Result<BTreeSet<String>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label} must be a list of strings"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{label} must be a list of strings"))
        })
        .collect()
}

fn load_input_hashes(
    items: Option<&Value>,
    label: &str,
    require_interface: bool,
) -> Result<BTreeMap<String, String>, String> {
    let values = items
        .and_then(Value::as_array)
        .ok_or_else(|| format!("manifest {label} must be a list."))?;
    let mut result = BTreeMap::new();
    for (index, item) in values.iter().enumerate() {
        let object = item
            .as_object()
            .ok_or_else(|| format!("{label}[{index}] must be an object"))?;
        let rel = object
            .get("rel_path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{label}[{index}] must contain rel_path."))?;
        audit_queue::validate_repo_relative_path_token(rel, label)?;
        let digest = object
            .get("sha256")
            .and_then(sha)
            .ok_or_else(|| format!("{label}[{index}].sha256 must be a SHA-256 hex digest."))?;
        if require_interface {
            if object.get("interface_relevant") != Some(&json!(true)) {
                return Err(format!("{label}[{index}] must be interface_relevant."));
            }
            if object.get("kind") == Some(&json!("source/ui-asset")) {
                return Err(format!(
                    "{label}[{index}] must not be a visual asset batch entry."
                ));
            }
        }
        if result.insert(rel.to_owned(), digest.to_owned()).is_some() {
            return Err(format!("{label} rel_path values must be unique: {rel}"));
        }
    }
    Ok(result)
}

fn load_context(path: &Path) -> Result<VerifyContext, String> {
    let root = validate_directory_nofollow(
        path.parent()
            .ok_or_else(|| "manifest has no parent directory".to_owned())?,
    )
    .map_err(|error| error.to_string())?;
    let bytes = read_bytes_nofollow(path, Some(&root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("manifest is missing: {}", path.display()))?;
    let raw = crate::audit_findings::strict_json_object(&bytes, "manifest")?;
    if raw.get("audit_kind") != Some(&json!("ui-implementation")) {
        return Err("manifest audit_kind must be 'ui-implementation'.".to_owned());
    }
    let repo = raw
        .get("repo_root")
        .and_then(Value::as_str)
        .ok_or_else(|| "manifest repo_root must be a string".to_owned())?;
    let repo = validate_directory_nofollow(Path::new(repo)).map_err(|error| error.to_string())?;
    let source_hashes = load_input_hashes(raw.get("source_files"), "source_files", true)?;
    let batches = raw
        .get("batches")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "manifest batches must be a list.".to_owned())?;
    let unit_values = raw
        .get("coverage_units")
        .and_then(Value::as_array)
        .ok_or_else(|| "manifest coverage_units must be a list.".to_owned())?;
    if source_hashes.is_empty() || batches.is_empty() || unit_values.is_empty() {
        return Err(
            "UI implementation audit manifests must contain implemented interface source, batches, and coverage units."
                .to_owned(),
        );
    }
    if raw.get("source_file_count").and_then(Value::as_u64) != Some(source_hashes.len() as u64)
        || raw.get("interface_file_count").and_then(Value::as_u64)
            != Some(source_hashes.len() as u64)
    {
        return Err(
            "manifest source/interface file counts must match the non-empty source_files list."
                .to_owned(),
        );
    }
    let mut unit_to_file = BTreeMap::new();
    let mut unit_hashes = BTreeMap::new();
    for (index, unit) in unit_values.iter().enumerate() {
        let unit = unit
            .as_object()
            .ok_or_else(|| format!("coverage_units[{index}] must be an object"))?;
        let id = unit
            .get("unit_id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("coverage_units[{index}] must contain unit_id"))?;
        let rel = unit
            .get("rel_path")
            .and_then(Value::as_str)
            .filter(|rel| source_hashes.contains_key(*rel))
            .ok_or_else(|| {
                format!("coverage_units[{index}].rel_path is absent from source_files")
            })?;
        if unit_to_file.insert(id.to_owned(), rel.to_owned()).is_some() {
            return Err(format!(
                "coverage_units unit_id values must be unique: {id}"
            ));
        }
        let digest = unit
            .get("sha256")
            .and_then(sha)
            .unwrap_or(&source_hashes[rel]);
        unit_hashes.insert(id.to_owned(), digest.to_owned());
    }
    let mut expected_by_batch = BTreeMap::new();
    let mut files_by_batch = BTreeMap::new();
    let mut assigned = Vec::new();
    for (index, batch) in batches.iter().enumerate() {
        let batch = batch
            .as_object()
            .ok_or_else(|| format!("batches[{index}] must be an object"))?;
        let id = batch
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("batches[{index}] must contain id."))?;
        let units = string_set(batch.get("coverage_units"), "batch coverage_units")?;
        if units.iter().any(|unit| !unit_to_file.contains_key(unit)) {
            return Err(format!("batch {id} references unknown coverage units"));
        }
        let files = string_set(batch.get("files"), "batch files")?;
        assigned.extend(units.iter().cloned());
        if expected_by_batch.insert(id.to_owned(), units).is_some() {
            return Err(format!("duplicate batch id: {id}"));
        }
        files_by_batch.insert(id.to_owned(), files);
    }
    let all_units = unit_to_file.keys().cloned().collect::<BTreeSet<_>>();
    let assigned_set = assigned.iter().cloned().collect::<BTreeSet<_>>();
    if all_units != assigned_set || !duplicate_values(&assigned).is_empty() {
        return Err("coverage unit assignment mismatch".to_owned());
    }
    let audit = raw
        .get("ui_implementation_audit")
        .and_then(Value::as_object)
        .ok_or_else(|| "manifest ui_implementation_audit must be an object.".to_owned())?;
    if audit.get("visual_required") != Some(&json!(true)) {
        return Err("implemented UI audits must require visual review.".to_owned());
    }
    let platform = audit
        .get("ui_platform")
        .and_then(Value::as_str)
        .filter(|value| ["web", "native", "hybrid"].contains(value))
        .ok_or_else(|| {
            "manifest ui_implementation_audit.ui_platform must be web, native, or hybrid."
                .to_owned()
        })?
        .to_owned();
    let formal_config = audit
        .get("formal_config")
        .filter(|value| !value.is_null())
        .cloned();
    if platform == "native" && formal_config.is_some() {
        return Err("native UI audit manifest must not declare formal web config.".to_owned());
    }
    if let Some(config) = formal_config.as_ref() {
        let config = config.as_object().ok_or_else(|| {
            "formal_config must be null or a hash-bound repository file record.".to_owned()
        })?;
        let rel = config
            .get("rel_path")
            .and_then(Value::as_str)
            .ok_or_else(|| "formal_config must contain rel_path.".to_owned())?;
        audit_queue::validate_repo_relative_path_token(rel, "formal_config")?;
        let config_path = repo.join(rel);
        let config_bytes = read_bytes_nofollow(&config_path, Some(&repo))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "formal_config must resolve to a regular repository file.".to_owned())?;
        if config.get("sha256") != Some(&json!(sha256_file(&config_path)?)) {
            return Err(
                "formal_config hash does not match the current repository file.".to_owned(),
            );
        }
        if config.get("size_bytes").and_then(Value::as_u64) != Some(config_bytes.len() as u64) {
            return Err(
                "formal_config size does not match the current repository file.".to_owned(),
            );
        }
    }
    let gate = audit
        .get("implementation_gate")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            "manifest implementation_gate must be a passed schema-version-1-or-2 gate.".to_owned()
        })?;
    let gate_schema = gate.get("schema_version").and_then(Value::as_u64);
    if !matches!(gate_schema, Some(1 | 2)) || gate.get("status") != Some(&json!("passed")) {
        return Err(
            "manifest implementation_gate must be a passed schema-version-1-or-2 gate.".to_owned(),
        );
    }
    if gate.get("rejected_files").is_some_and(|value| {
        !value.is_null() && value.as_array().is_none_or(|items| !items.is_empty())
    }) {
        return Err(
            "manifest implementation_gate must not retain rejected evidence files in a passed gate."
                .to_owned(),
        );
    }
    let evidence_files = gate
        .get("evidence_files")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            "manifest implementation_gate must name at least one evidence file.".to_owned()
        })?;
    let mut evidence_paths = BTreeSet::new();
    for (index, item) in evidence_files.iter().enumerate() {
        let item = item.as_object().ok_or_else(|| {
            format!("implementation_gate.evidence_files[{index}] must contain rel_path.")
        })?;
        let rel = item
            .get("rel_path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("implementation_gate.evidence_files[{index}] must contain rel_path.")
            })?;
        if item.get("sha256").and_then(Value::as_str) != source_hashes.get(rel).map(String::as_str)
        {
            return Err(format!(
                "implementation gate evidence hash does not match source_files: {rel}"
            ));
        }
        let qualification = item.get("qualification");
        let basis = if gate_schema == Some(1) {
            None
        } else {
            let qualification = qualification.and_then(Value::as_object).ok_or_else(|| {
                format!("implementation gate evidence lacks qualification metadata: {rel}")
            })?;
            match qualification.get("method").and_then(Value::as_str) {
                Some("recognized-ui-signal") => None,
                Some("explicit-source-anchor") => Some(ExplicitUiBasis {
                    ui_kind: qualification
                        .get("ui_kind")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    source_anchor: qualification
                        .get("source_anchor")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                }),
                _ => {
                    return Err(format!(
                        "implementation gate evidence uses an unknown qualification method: {rel}"
                    ));
                }
            }
        };
        let current = ui_gate::qualify_implementation_source(&repo, rel, basis.as_ref()).map_err(
            |issue| {
                format!(
                    "implementation gate evidence is not a qualifying product UI source: {rel}: {issue}"
                )
            },
        )?;
        if gate_schema == Some(2)
            && serde_json::to_value(current).map_err(|error| error.to_string())?
                != qualification.cloned().unwrap_or(Value::Null)
        {
            return Err(format!(
                "implementation gate qualification no longer matches executable source: {rel}"
            ));
        }
        if !evidence_paths.insert(rel.to_owned()) {
            return Err("manifest implementation_gate evidence paths must be unique.".to_owned());
        }
    }
    let empty = Value::Array(Vec::new());
    let asset_hashes = load_input_hashes(
        Some(audit.get("visual_assets").unwrap_or(&empty)),
        "visual_assets",
        false,
    )?;
    let requirement_hashes = load_input_hashes(
        Some(audit.get("requirement_sources").unwrap_or(&empty)),
        "requirement_sources",
        false,
    )?;
    Ok(VerifyContext {
        raw,
        root,
        repo,
        source_hashes,
        asset_hashes,
        requirement_hashes,
        unit_to_file,
        unit_hashes,
        batches,
        expected_by_batch,
        files_by_batch,
        platform,
        formal_config,
    })
}

fn first_declared_value(body: &str) -> &str {
    body.lines()
        .map(|line| line.trim().trim_matches('`'))
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

fn finding_blocks(text: &str) -> Vec<BTreeMap<String, String>> {
    if text.trim() == "No findings." {
        return Vec::new();
    }
    let line = Regex::new(r"^-\s+([^:]+):\s*(.*)$").expect("finding line regex");
    let mut blocks = Vec::new();
    let mut current = BTreeMap::new();
    for raw in text.lines() {
        let Some(capture) = line.captures(raw.trim()) else {
            continue;
        };
        let key = capture.get(1).unwrap().as_str().trim();
        if key == "Priority" && !current.is_empty() {
            blocks.push(std::mem::take(&mut current));
        }
        current.insert(
            key.to_owned(),
            capture.get(2).unwrap().as_str().trim().to_owned(),
        );
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

fn split_files(value: &str) -> Vec<String> {
    value
        .replace('`', "")
        .split([',', ';'])
        .map(str::trim)
        .filter(|item| !item.is_empty() && !item.eq_ignore_ascii_case("none"))
        .map(str::to_owned)
        .collect()
}

fn issue_path(path: &Path) -> Value {
    json!(path.to_string_lossy())
}

fn validate_findings(
    text: &str,
    allowed_files: &BTreeSet<String>,
    path: &Path,
    section: &str,
    allow_not_applicable: bool,
) -> Vec<Value> {
    if text.trim().is_empty() {
        return vec![
            json!({"path":issue_path(path),"section":section,"reason":"findings section is empty"}),
        ];
    }
    if text.trim() == "No findings." {
        return Vec::new();
    }
    let blocks = finding_blocks(text);
    if blocks.is_empty() {
        return vec![
            json!({"path":issue_path(path),"section":section,"reason":"findings must use required field blocks or exact sentinel"}),
        ];
    }
    let mut issues = Vec::new();
    for (offset, block) in blocks.iter().enumerate() {
        let index = offset + 1;
        let missing = REQUIRED_FINDING_FIELDS
            .iter()
            .filter(|field| block.get(**field).is_none_or(|value| value.is_empty()))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            issues.push(json!({"path":issue_path(path),"section":section,"finding":index,"missing_fields":missing}));
        }
        if let Some(priority) = block.get("Priority")
            && !["P0", "P1", "P2", "P3"].contains(&priority.as_str())
        {
            issues.push(json!({"path":issue_path(path),"section":section,"finding":index,"field":"Priority","actual":priority}));
        }
        let files = split_files(block.get("Files").map_or("", String::as_str));
        if files.is_empty() {
            issues.push(json!({"path":issue_path(path),"section":section,"finding":index,"field":"Files","reason":"no files listed"}));
            continue;
        }
        let normalized = files
            .into_iter()
            .filter(|item| {
                !["not-applicable", "not applicable", "n/a"]
                    .contains(&item.to_ascii_lowercase().as_str())
            })
            .collect::<BTreeSet<_>>();
        if normalized.is_empty() && !allow_not_applicable {
            issues.push(json!({"path":issue_path(path),"section":section,"finding":index,"field":"Files","reason":"not-applicable not allowed here"}));
        }
        let unknown = normalized
            .difference(allowed_files)
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            issues.push(json!({"path":issue_path(path),"section":section,"finding":index,"field":"Files","out_of_scope":unknown}));
        }
    }
    issues
}

fn visual_danger(text: &str) -> bool {
    let danger = Regex::new(
        r"(?i)\b(?:overloaded?|crowded|cramped|unreadable|invisible|low[- ]contrast|clipped|cropped|truncated|overflow|unscannable|ambiguous hierarchy|oversized|excessive detail|debug detail|raw status|overexposed|over-prescribed|overprescribed|duplicate summaries?|duplicate severity|vague labels?|unclear labels?|source-model leakage|data-model leakage|dominates|dominating|buried|below the fold|nested cards?|nested blocks?|nested containers?|nested frames?|card[- ]in[- ]card|border stacks?|background stacks?|visual noise|noisy surfaces?|misaligned|misalignment|random placement|weak grid|poor grid|grid drift|inconsistent gutters?|unstable expander|unstable expansion|unstable disclosure|jumps? horizontally|width changes?|different widths?|meaningless icons?|unclear icons?|unintuitive icons?|decorative clutter|avatar clutter|unnecessary avatars?|tiny icon[- ]only target|instruction noise|(?:redundant|unnecessary|excessive|misleading|verbose|duplicated) (?:helper text|instructions?|explanations?)|low[- ]importance)\b|(?:row [^.\n;]*not clickable|navigation [^.\n;]*no pointer|popover [^.\n;]*never closes|badge [^.\n;]*no detail|hover [^.\n;]*missing|scrollbar [^.\n;]*overlaps|copy button [^.\n;]*always visible|status [^.\n;]*twice|decision-critical [^.\n;]*hidden)",
    )
    .expect("visual danger regex");
    let negation = Regex::new(r"(?i)\b(?:no(?: signs? of)?|not|without(?: any)?)\s+$")
        .expect("visual danger negation regex");
    danger
        .find_iter(text)
        .any(|matched| !negation.is_match(&text[..matched.start()]))
}

fn evidence_text(value: &str, first_viewport: bool) -> bool {
    let common = r"(?i)\b(?:screenshot|screen shot|png|jpg|jpeg|webp|trace|video|playwright|cypress|storybook|browser|preview|simulator|native|snapshot|dom|viewport|measurement|measured|px|%|fold|scroll|source|css|blocked|unavailable|not applicable|no safe|no runnable)\b";
    if Regex::new(common).expect("evidence regex").is_match(value) {
        return true;
    }
    !first_viewport && value.to_ascii_lowercase().contains("xcode")
}

fn has_visual_danger_finding(findings: &str) -> bool {
    finding_blocks(findings).iter().any(|block| {
        let combined = block.values().cloned().collect::<Vec<_>>().join(" ");
        visual_danger(&combined)
            || [
                "journey usability",
                "decision path",
                "rendered journey",
                "readability",
                "contrast",
                "scannable",
            ]
            .iter()
            .any(|token| combined.to_ascii_lowercase().contains(token))
    })
}

fn wiring_reference(repo: &Path, value: &str, field: &str) -> (String, Option<Value>) {
    let normalized = value.trim().trim_matches('`');
    let lowered = normalized.to_ascii_lowercase();
    if lowered == "missing" {
        return ("missing".to_owned(), None);
    }
    if lowered.starts_with("not-applicable:") || lowered.starts_with("not applicable:") {
        let rationale = normalized
            .split_once(':')
            .map_or("", |(_, value)| value.trim());
        if rationale.len() < 12 {
            return (
                "invalid".to_owned(),
                Some(
                    json!({"field":field,"reason":"not-applicable wiring references require a concrete rationale","actual":value}),
                ),
            );
        }
        return ("not-applicable".to_owned(), None);
    }
    let Some((raw_path, symbol)) = normalized.split_once('#') else {
        return (
            "invalid".to_owned(),
            Some(
                json!({"field":field,"reason":"wiring reference must be path#symbol, missing, or not-applicable: rationale","actual":value}),
            ),
        );
    };
    if audit_queue::validate_repo_relative_path_token(raw_path, "wiring reference").is_err()
        || symbol.trim().is_empty()
    {
        return (
            "invalid".to_owned(),
            Some(
                json!({"field":field,"reason":"wiring reference path#symbol is invalid","actual":value}),
            ),
        );
    }
    let path = repo.join(raw_path);
    let source = match read_bytes_nofollow(&path, Some(repo)) {
        Ok(Some(bytes)) => match String::from_utf8(bytes) {
            Ok(source) => source,
            Err(_) => {
                return (
                    "invalid".to_owned(),
                    Some(
                        json!({"field":field,"reason":"wiring reference source is not UTF-8 text","actual":value}),
                    ),
                );
            }
        },
        _ => {
            return (
                "invalid".to_owned(),
                Some(
                    json!({"field":field,"reason":"wiring reference path does not resolve inside the audited repo","actual":value}),
                ),
            );
        }
    };
    if !source.contains(symbol.trim()) {
        return (
            "invalid".to_owned(),
            Some(
                json!({"field":field,"reason":"wiring reference symbol/text is absent from the referenced file","actual":value}),
            ),
        );
    }
    if field == "test reference"
        && !Regex::new(r"(?i)(?:^|/)(?:tests?|__tests__)(?:/|$)|\.(?:test|spec)\.")
            .expect("test path regex")
            .is_match(raw_path)
    {
        return (
            "invalid".to_owned(),
            Some(
                json!({"field":field,"reason":"test reference must use a test/spec path","actual":value}),
            ),
        );
    }
    ("bound".to_owned(), None)
}

fn missing_columns(row: &BTreeMap<String, String>, required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter(|column| !row.contains_key(**column))
        .map(|column| (*column).to_owned())
        .collect()
}

fn verify_journey_model(path: &Path, body: &str) -> Vec<Value> {
    let rows = parse_markdown_table_dicts(body);
    if rows.is_empty() {
        return vec![
            json!({"path":issue_path(path),"section":"Journey Decision Model","reason":"missing journey decision table"}),
        ];
    }
    let mut issues = Vec::new();
    let mut journeys = BTreeSet::new();
    for (offset, row) in rows.iter().enumerate() {
        let missing = missing_columns(row, JOURNEY_DECISION_COLUMNS);
        if !missing.is_empty() {
            issues.push(json!({"path":issue_path(path),"section":"Journey Decision Model","row":offset+1,"missing_columns":missing}));
            continue;
        }
        for field in JOURNEY_DECISION_COLUMNS {
            if row.get(*field).is_none_or(|value| value.trim().is_empty()) {
                issues.push(json!({"path":issue_path(path),"section":"Journey Decision Model","row":offset+1,"field":field,"reason":"empty"}));
            }
        }
        if let Some(journey) = row.get("journey id")
            && !journeys.insert(journey.trim().to_owned())
        {
            issues.push(json!({"path":issue_path(path),"section":"Journey Decision Model","journey":journey,"reason":"duplicate Journey ID"}));
        }
    }
    issues
}

fn ux_cell<'a>(row: &'a BTreeMap<String, String>, column: &str) -> &'a str {
    row.get(column).map_or("", |value| value.trim())
}

fn ux_rows(
    path: &Path,
    bodies: &BTreeMap<String, String>,
    section: &str,
    columns: &[&str],
    issues: &mut Vec<Value>,
) -> Vec<BTreeMap<String, String>> {
    let rows = parse_markdown_table_dicts(bodies.get(section).map_or("", String::as_str));
    if rows.is_empty() {
        issues.push(json!({"path":issue_path(path),"section":section,"reason":"missing required UX review table"}));
    }
    rows.into_iter()
        .enumerate()
        .filter_map(|(index, row)| {
            let missing = columns
                .iter()
                .filter(|column| ux_cell(&row, column).is_empty())
                .copied()
                .collect::<Vec<_>>();
            if missing.is_empty() {
                Some(row)
            } else {
                issues.push(json!({"path":issue_path(path),"section":section,"row":index+1,"missing_or_empty_columns":missing}));
                None
            }
        })
        .collect()
}

fn verify_ux_row(
    path: &Path,
    section: &str,
    row: &BTreeMap<String, String>,
    records: &crate::audit_evidence::EvidenceRecords,
    finding_ids: &BTreeSet<String>,
    allow_not_applicable: bool,
) -> Vec<Value> {
    let mut issues = Vec::new();
    let result = ux_cell(row, "result");
    let journey = ux_cell(row, "journey id");
    if ["none", "n/a", "not applicable", "-", "todo"]
        .contains(&ux_cell(row, "reason").to_ascii_lowercase().as_str())
    {
        issues.push(json!({"path":issue_path(path),"section":section,"journey":journey,"reason":"a concrete assessment rationale is required"}));
    }
    if !["PASS", "GAP", "BLOCKED", "NOT_APPLICABLE"].contains(&result)
        || (!allow_not_applicable && result == "NOT_APPLICABLE")
    {
        issues.push(json!({"path":issue_path(path),"section":section,"journey":journey,"reason":"invalid UX review result","actual":result}));
    }
    let references = split_files(ux_cell(row, "finding"));
    if ["GAP", "BLOCKED"].contains(&result) {
        if references.is_empty()
            || references
                .iter()
                .any(|reference| !finding_ids.contains(reference))
        {
            issues.push(json!({"path":issue_path(path),"section":section,"journey":journey,"reason":"GAP/BLOCKED requires an existing complete Finding ID","actual":references}));
        }
    } else if !references.is_empty() {
        issues.push(json!({"path":issue_path(path),"section":section,"journey":journey,"reason":"PASS/NOT_APPLICABLE requires Finding=none"}));
    }
    let evidence = ux_cell(row, "evidence");
    for detail in crate::audit_evidence::validate_references(evidence, records, None) {
        issues.push(
            json!({"path":issue_path(path),"section":section,"journey":journey,"detail":detail}),
        );
    }
    let has_observation = crate::audit_evidence::evidence_references(evidence)
        .iter()
        .filter_map(|reference| records.get(reference))
        .filter_map(|record| record.get("kind").and_then(Value::as_str))
        .any(|kind| !["review-queue", "manual-review"].contains(&kind));
    if result != "BLOCKED" && !has_observation {
        issues.push(json!({"path":issue_path(path),"section":section,"journey":journey,"reason":"UX judgment requires registered observation evidence, not only a review decision"}));
    }
    issues
}

fn verify_ux_reviews(
    path: &Path,
    bodies: &BTreeMap<String, String>,
    findings: &str,
    records: &crate::audit_evidence::EvidenceRecords,
    platform_scope: &str,
) -> Vec<Value> {
    let mut issues = Vec::new();
    let journeys = parse_markdown_table_dicts(
        bodies
            .get("journey decision model")
            .map_or("", String::as_str),
    )
    .iter()
    .map(|row| ux_cell(row, "journey id").to_owned())
    .filter(|journey| !journey.is_empty())
    .collect::<BTreeSet<_>>();
    let mut finding_ids = BTreeSet::new();
    for finding in finding_blocks(findings) {
        let Some(identifier) = finding.get("Finding ID") else {
            continue;
        };
        if identifier.trim().is_empty()
            || !finding_ids.insert(identifier.trim().to_owned())
            || REQUIRED_FINDING_FIELDS
                .iter()
                .any(|field| ux_cell(&finding, field).is_empty())
        {
            issues.push(json!({"path":issue_path(path),"section":"UX findings","reason":"referenced findings require unique nonempty IDs and complete finding fields","finding_id":identifier}));
        }
    }
    let flows = ux_rows(
        path,
        bodies,
        "journey flow review",
        JOURNEY_FLOW_COLUMNS,
        &mut issues,
    );
    let reviews = ux_rows(
        path,
        bodies,
        "ux guideline review",
        UX_REVIEW_COLUMNS,
        &mut issues,
    );
    let mut flow_by_journey = BTreeMap::new();
    let mut surfaces = BTreeMap::new();
    for row in &flows {
        let journey = ux_cell(row, "journey id");
        if flow_by_journey.insert(journey.to_owned(), row).is_some() {
            issues.push(json!({"path":issue_path(path),"section":"journey flow review","journey":journey,"reason":"duplicate journey flow"}));
        }
        let names = ux_cell(row, "surfaces")
            .split(';')
            .map(str::trim)
            .filter(|surface| !surface.is_empty())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        if names.is_empty() || names.contains("all") {
            issues.push(json!({"path":issue_path(path),"section":"journey flow review","journey":journey,"reason":"name the actual surfaces, not an empty list or all"}));
        }
        surfaces.insert(journey.to_owned(), names);
        issues.extend(verify_ux_row(
            path,
            "journey flow review",
            row,
            records,
            &finding_ids,
            false,
        ));
        let outcome = ux_cell(row, "outcome evidence");
        for detail in crate::audit_evidence::validate_references(outcome, records, None) {
            issues.push(json!({"path":issue_path(path),"section":"journey flow review","journey":journey,"detail":detail}));
        }
        let outcome_proof = crate::audit_evidence::evidence_references(outcome)
            .iter()
            .filter_map(|reference| records.get(reference))
            .filter_map(|record| record.get("kind").and_then(Value::as_str))
            .any(|kind| {
                ["trace", "video", "formal-web-verifier", "journey-evidence"].contains(&kind)
            });
        if ux_cell(row, "result") == "PASS" && !outcome_proof {
            issues.push(json!({"path":issue_path(path),"section":"journey flow review","journey":journey,"reason":"PASS requires runtime outcome evidence; screenshots and source anchors alone are insufficient"}));
        }
    }
    if flow_by_journey.keys().cloned().collect::<BTreeSet<_>>() != journeys {
        issues.push(json!({"path":issue_path(path),"section":"journey flow review","reason":"flow coverage must exactly match the journey decision model"}));
    }
    let mut coverage = BTreeSet::new();
    let mut item_keys = BTreeSet::new();
    let mut surface_coverage = BTreeSet::new();
    for row in &reviews {
        let journey = ux_cell(row, "journey id");
        let criterion = ux_cell(row, "criterion");
        let surface = ux_cell(row, "surface");
        let item = ux_cell(row, "item");
        if !journeys.contains(journey) || !crate::ui_audit::UX_CRITERIA.contains(&criterion) {
            issues.push(json!({"path":issue_path(path),"section":"ux guideline review","journey":journey,"criterion":criterion,"reason":"unknown journey or criterion"}));
        }
        if !item_keys.insert((journey, criterion, surface, item)) {
            issues.push(json!({"path":issue_path(path),"section":"ux guideline review","journey":journey,"criterion":criterion,"item":item,"reason":"duplicate assessment"}));
        }
        let contextual = crate::ui_audit::CONTEXT_CRITERIA.contains(&criterion);
        let needs_surface = ["surface-purpose", "copy-purpose"].contains(&criterion) || contextual;
        if (needs_surface || surface != "all")
            && !surfaces
                .get(journey)
                .is_some_and(|names| names.contains(surface))
        {
            issues.push(json!({"path":issue_path(path),"section":"ux guideline review","journey":journey,"surface":surface,"reason":"assessment must name a declared journey surface"}));
        }
        coverage.insert((journey, criterion));
        surface_coverage.insert((journey, criterion, surface));
        let allow_not_applicable =
            !["journey-first", "step-necessity", "surface-purpose"].contains(&criterion);
        issues.extend(verify_ux_row(
            path,
            "ux guideline review",
            row,
            records,
            &finding_ids,
            allow_not_applicable,
        ));
        if contextual
            && ux_cell(row, "result") == "PASS"
            && (!concrete_ux_value(item) || !runtime_observation(ux_cell(row, "evidence"), records))
        {
            issues.push(json!({"path":issue_path(path),"section":"ux guideline review","journey":journey,"criterion":criterion,"reason":"passing contextual controls require a named item and runtime observation"}));
        }
        if ["GAP", "BLOCKED"].contains(&ux_cell(row, "result"))
            && flow_by_journey
                .get(journey)
                .is_some_and(|flow| ux_cell(flow, "result") == "PASS")
        {
            issues.push(json!({"path":issue_path(path),"section":"journey flow review","journey":journey,"reason":"flow cannot PASS with a GAP/BLOCKED guideline assessment"}));
        }
    }
    for journey in &journeys {
        for criterion in crate::ui_audit::UX_CRITERIA {
            if !coverage.contains(&(journey.as_str(), *criterion)) {
                issues.push(json!({"path":issue_path(path),"section":"ux guideline review","journey":journey,"criterion":criterion,"reason":"missing required guideline assessment"}));
            }
        }
        if let Some(names) = surfaces.get(journey) {
            for surface in names {
                for criterion in ["surface-purpose", "copy-purpose"]
                    .into_iter()
                    .chain(crate::ui_audit::CONTEXT_CRITERIA.iter().copied())
                {
                    if !surface_coverage.contains(&(journey.as_str(), criterion, surface.as_str()))
                    {
                        issues.push(json!({"path":issue_path(path),"section":"ux guideline review","journey":journey,"surface":surface,"criterion":criterion,"reason":"each surface requires purpose, supporting-copy, and contextual-control review"}));
                    }
                }
            }
        }
    }
    issues.extend(verify_configuration_reviews(
        path,
        bodies,
        records,
        &finding_ids,
        &journeys,
        platform_scope,
    ));
    issues
}

fn concrete_ux_value(value: &str) -> bool {
    ![
        "",
        "none",
        "all",
        "unknown",
        "n/a",
        "not applicable",
        "-",
        "todo",
    ]
    .contains(&value.trim().to_ascii_lowercase().as_str())
}

fn ux_names(value: &str) -> BTreeSet<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn runtime_observation(evidence: &str, records: &crate::audit_evidence::EvidenceRecords) -> bool {
    crate::audit_evidence::evidence_references(evidence)
        .iter()
        .filter_map(|reference| records.get(reference))
        .filter_map(|record| record.get("kind").and_then(Value::as_str))
        .any(|kind| ["trace", "video", "journey-evidence"].contains(&kind))
}

fn platform_observation(
    evidence: &str,
    records: &crate::audit_evidence::EvidenceRecords,
    platform: &str,
) -> bool {
    if platform == "web" {
        return runtime_observation(evidence, records);
    }
    crate::audit_evidence::evidence_references(evidence)
        .iter()
        .filter_map(|reference| records.get(reference))
        .any(|record| {
            record
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| ["trace", "video"].contains(&kind))
                && record.get("platform").and_then(Value::as_str) == Some(platform)
        })
}

fn verify_configuration_reviews(
    path: &Path,
    bodies: &BTreeMap<String, String>,
    records: &crate::audit_evidence::EvidenceRecords,
    finding_ids: &BTreeSet<String>,
    journeys: &BTreeSet<String>,
    platform_scope: &str,
) -> Vec<Value> {
    let mut issues = Vec::new();
    let configurations = ux_rows(
        path,
        bodies,
        "ui configuration contract",
        CONFIG_COLUMNS,
        &mut issues,
    );
    let interactions = ux_rows(
        path,
        bodies,
        "interaction coverage",
        INTERACTION_COLUMNS,
        &mut issues,
    );
    let builds = ux_rows(
        path,
        bodies,
        "rendered build review",
        BUILD_COLUMNS,
        &mut issues,
    );
    let designs = ux_rows(
        path,
        bodies,
        "design decision review",
        DESIGN_COLUMNS,
        &mut issues,
    );
    let mut configuration_ids = BTreeSet::new();
    let mut configuration_keys = BTreeSet::new();
    let mut unknown_configurations = BTreeSet::new();
    let mut declared_journeys = BTreeSet::new();
    let mut expected_interactions = BTreeSet::new();
    for row in &configurations {
        let identifier = ux_cell(row, "config id");
        let platform = ux_cell(row, "platform");
        let members = ux_names(ux_cell(row, "journey ids"));
        let update = ux_cell(row, "update journey");
        let platform_known = platform == "web"
            || ["desktop:", "native:"]
                .iter()
                .any(|prefix| platform.strip_prefix(prefix).is_some_and(concrete_ux_value));
        let key =
            ["platform", "theme", "viewport", "input mode"].map(|column| ux_cell(row, column));
        if !configuration_ids.insert(identifier)
            || !configuration_keys.insert(key)
            || !concrete_ux_value(identifier)
            || members.is_empty()
            || !members.is_subset(journeys)
            || (!platform_known && platform != "unknown")
            || !["pointer", "keyboard", "touch", "unknown"].contains(&ux_cell(row, "input mode"))
            || (platform.starts_with("desktop:") && !members.contains(update))
            || (!platform.starts_with("desktop:") && update != "none")
            || (platform_scope == "web" && !["web", "unknown"].contains(&platform))
            || (platform_scope == "native" && platform == "web")
        {
            issues.push(json!({"path":issue_path(path),"section":"ui configuration contract","config":identifier,"reason":"invalid or duplicate configuration, journey membership, input mode, or desktop update journey"}));
        }
        if key.iter().any(|value| !concrete_ux_value(value)) {
            unknown_configurations.insert(identifier);
        }
        declared_journeys.extend(members.iter().cloned());
        for journey in members {
            for scenario in crate::ui_audit::INTERACTION_SCENARIOS
                .iter()
                .copied()
                .chain(
                    crate::ui_audit::UPDATE_SCENARIOS
                        .iter()
                        .copied()
                        .filter(|_| journey == update),
                )
            {
                expected_interactions.insert((
                    identifier.to_owned(),
                    journey.clone(),
                    scenario.to_owned(),
                ));
            }
        }
    }
    if &declared_journeys != journeys {
        issues.push(json!({"path":issue_path(path),"section":"ui configuration contract","reason":"every journey requires an explicit supported configuration"}));
    }
    let mut actual_interactions = BTreeSet::new();
    let mut unavailable_journeys = BTreeSet::new();
    for row in &interactions {
        let configuration = ux_cell(row, "config id");
        let journey = ux_cell(row, "journey id");
        let scenario = ux_cell(row, "scenario");
        let platform = configurations
            .iter()
            .find(|config| ux_cell(config, "config id") == configuration)
            .map_or("unknown", |config| ux_cell(config, "platform"));
        let key = (
            configuration.to_owned(),
            journey.to_owned(),
            scenario.to_owned(),
        );
        if !expected_interactions.contains(&key) || !actual_interactions.insert(key) {
            issues.push(json!({"path":issue_path(path),"section":"interaction coverage","config":configuration,"journey":journey,"scenario":scenario,"reason":"unexpected or duplicate interaction cell"}));
        }
        let allow_not_applicable =
            scenario != "completion" && !crate::ui_audit::UPDATE_SCENARIOS.contains(&scenario);
        issues.extend(verify_ux_row(
            path,
            "interaction coverage",
            row,
            records,
            finding_ids,
            allow_not_applicable,
        ));
        if ux_cell(row, "result") == "PASS"
            && (!platform_observation(ux_cell(row, "evidence"), records, platform)
                || !concrete_ux_value(ux_cell(row, "target")))
        {
            issues.push(json!({"path":issue_path(path),"section":"interaction coverage","config":configuration,"journey":journey,"scenario":scenario,"reason":"passing interactions require a concrete target and platform-bound runtime observation, not screenshots, browser-only native proof, or aggregate reports"}));
        }
        if unknown_configurations.contains(configuration) && ux_cell(row, "result") != "BLOCKED" {
            issues.push(json!({"path":issue_path(path),"section":"interaction coverage","config":configuration,"reason":"unknown supported configurations must remain BLOCKED"}));
        }
        if ["GAP", "BLOCKED"].contains(&ux_cell(row, "result")) {
            unavailable_journeys.insert(journey.to_owned());
        }
    }
    for (configuration, journey, scenario) in expected_interactions.difference(&actual_interactions)
    {
        issues.push(json!({"path":issue_path(path),"section":"interaction coverage","config":configuration,"journey":journey,"scenario":scenario,"reason":"missing required interaction cell"}));
    }
    let mut build_ids = BTreeSet::new();
    for row in &builds {
        let identifier = ux_cell(row, "config id");
        let platform = configurations
            .iter()
            .find(|config| ux_cell(config, "config id") == identifier)
            .map_or("unknown", |config| ux_cell(config, "platform"));
        if !configuration_ids.contains(identifier) || !build_ids.insert(identifier) {
            issues.push(json!({"path":issue_path(path),"section":"rendered build review","config":identifier,"reason":"unexpected or duplicate build review"}));
        }
        issues.extend(verify_ux_row(
            path,
            "rendered build review",
            row,
            records,
            finding_ids,
            false,
        ));
        if ux_cell(row, "result") == "PASS"
            && (!concrete_ux_value(ux_cell(row, "target"))
                || !concrete_ux_value(ux_cell(row, "expected snapshot"))
                || ux_cell(row, "expected snapshot") != ux_cell(row, "observed snapshot")
                || !platform_observation(ux_cell(row, "evidence"), records, platform))
        {
            issues.push(json!({"path":issue_path(path),"section":"rendered build review","config":identifier,"reason":"passing build review requires an accessible target, matching snapshots, and runtime observation"}));
        }
        if unknown_configurations.contains(identifier) && ux_cell(row, "result") != "BLOCKED" {
            issues.push(json!({"path":issue_path(path),"section":"rendered build review","config":identifier,"reason":"unknown build configuration must remain BLOCKED"}));
        }
        if ["GAP", "BLOCKED"].contains(&ux_cell(row, "result")) {
            for configuration in configurations
                .iter()
                .filter(|configuration| ux_cell(configuration, "config id") == identifier)
            {
                unavailable_journeys.extend(ux_names(ux_cell(configuration, "journey ids")));
            }
        }
    }
    if build_ids != configuration_ids {
        issues.push(json!({"path":issue_path(path),"section":"rendered build review","reason":"every configuration requires a build identity review"}));
    }
    let mut design_ids = BTreeSet::new();
    let mut design_journeys = BTreeSet::new();
    for row in &designs {
        let identifier = ux_cell(row, "review id");
        let applicability = ux_cell(row, "applicability");
        let members = ux_names(ux_cell(row, "journey ids"));
        if !design_ids.insert(identifier)
            || !concrete_ux_value(identifier)
            || members.is_empty()
            || !members.is_subset(journeys)
            || ![
                "alternatives",
                "approved-design",
                "routine-fix",
                "unavailable",
            ]
            .contains(&applicability)
        {
            issues.push(json!({"path":issue_path(path),"section":"design decision review","review":identifier,"reason":"invalid design applicability, identity, or journey references"}));
        }
        design_journeys.extend(members.iter().cloned());
        let unavailable = applicability == "unavailable";
        issues.extend(verify_ux_row(
            path,
            "design decision review",
            row,
            records,
            finding_ids,
            unavailable,
        ));
        if unavailable && ux_cell(row, "result") != "NOT_APPLICABLE" {
            issues.push(json!({"path":issue_path(path),"section":"design decision review","review":identifier,"reason":"unavailable mockups do not prove fidelity or historical noncompliance; use reasoned NOT_APPLICABLE"}));
        }
        if !unavailable && ux_cell(row, "result") == "PASS" {
            let options = ux_names(ux_cell(row, "options"));
            if !concrete_ux_value(ux_cell(row, "design target"))
                || !concrete_ux_value(ux_cell(row, "authority"))
                || (applicability == "alternatives"
                    && (options.len() != 3
                        || options.iter().any(|option| !concrete_ux_value(option))
                        || !options.contains(ux_cell(row, "selection"))
                        || !concrete_ux_value(ux_cell(row, "distinctions"))))
            {
                issues.push(json!({"path":issue_path(path),"section":"design decision review","review":identifier,"reason":"passing design review requires a current target and authority; supplied alternatives require three named options, material distinctions, and a selected option"}));
            }
        }
        if ["GAP", "BLOCKED"].contains(&ux_cell(row, "result")) {
            unavailable_journeys.extend(members);
        }
    }
    if &design_journeys != journeys {
        issues.push(json!({"path":issue_path(path),"section":"design decision review","reason":"every journey requires a current design applicability review"}));
    }
    for row in
        parse_markdown_table_dicts(bodies.get("journey flow review").map_or("", String::as_str))
    {
        if ux_cell(&row, "result") == "PASS"
            && unavailable_journeys.contains(ux_cell(&row, "journey id"))
        {
            issues.push(json!({"path":issue_path(path),"section":"journey flow review","journey":ux_cell(&row,"journey id"),"reason":"flow cannot PASS with GAP/BLOCKED interaction, build, or design coverage"}));
        }
    }
    issues
}

fn verify_ux_synthesis(
    path: &Path,
    final_bodies: &BTreeMap<String, String>,
    worker_bodies: &BTreeMap<String, String>,
) -> Vec<Value> {
    let mut issues = Vec::new();
    for (section, columns) in [
        ("journey decision model", &["journey id"][..]),
        ("journey flow review", &["journey id"][..]),
        (
            "ux guideline review",
            &["journey id", "criterion", "surface", "item"][..],
        ),
        ("ui configuration contract", CONFIG_COLUMNS),
        (
            "interaction coverage",
            &[
                "journey id",
                "config id",
                "scenario",
                "target",
                "expected result",
                "observation",
                "evidence",
                "finding",
            ][..],
        ),
        (
            "rendered build review",
            &[
                "config id",
                "target",
                "expected snapshot",
                "observed snapshot",
                "evidence",
                "finding",
            ][..],
        ),
        (
            "design decision review",
            &[
                "review id",
                "journey ids",
                "applicability",
                "design target",
                "options",
                "selection",
                "authority",
                "distinctions",
                "evidence",
                "finding",
            ][..],
        ),
    ] {
        let indexed = |bodies: &BTreeMap<String, String>| {
            parse_markdown_table_dicts(bodies.get(section).map_or("", String::as_str))
                .iter()
                .map(|row| {
                    (
                        columns
                            .iter()
                            .map(|column| ux_cell(row, column).to_owned())
                            .collect::<Vec<_>>(),
                        ux_cell(row, "result").to_owned(),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        };
        let expected = indexed(worker_bodies);
        let actual = indexed(final_bodies);
        if expected.keys().collect::<BTreeSet<_>>() != actual.keys().collect::<BTreeSet<_>>() {
            issues.push(json!({"path":issue_path(path),"section":section,"reason":"final report must preserve the visual worker's journey and assessment coverage"}));
        }
        for (key, result) in expected {
            if ["GAP", "BLOCKED"].contains(&result.as_str())
                && actual
                    .get(&key)
                    .is_some_and(|value| ["PASS", "NOT_APPLICABLE"].contains(&value.as_str()))
            {
                issues.push(json!({"path":issue_path(path),"section":section,"assessment":key,"reason":"final synthesis cannot silently clear a GAP/BLOCKED UX assessment"}));
            }
        }
    }
    issues
}

fn mobile(value: &str) -> bool {
    ["mobile", "narrow", "phone", "390", "375", "360", "320"]
        .iter()
        .any(|token| value.to_ascii_lowercase().contains(token))
}

fn verify_rendered_usability(
    path: &Path,
    body: &str,
    findings: &str,
    platform_scope: &str,
) -> Vec<Value> {
    let rows = parse_markdown_table_dicts(body);
    if rows.is_empty() {
        return vec![
            json!({"path":issue_path(path),"section":"Rendered Journey Usability","reason":"missing rendered journey usability table"}),
        ];
    }
    let mut issues = Vec::new();
    let mut web_desktop = false;
    let mut web_mobile = false;
    let mut native = false;
    let mut gap = false;
    let mut danger = false;
    for (offset, row) in rows.iter().enumerate() {
        let index = offset + 1;
        let missing = missing_columns(row, RENDERED_USABILITY_COLUMNS);
        if !missing.is_empty() {
            issues.push(json!({"path":issue_path(path),"section":"Rendered Journey Usability","row":index,"missing_columns":missing}));
            continue;
        }
        for field in RENDERED_USABILITY_COLUMNS {
            if row.get(*field).is_none_or(|value| value.trim().is_empty()) {
                issues.push(json!({"path":issue_path(path),"section":"Rendered Journey Usability","row":index,"field":field,"reason":"empty"}));
            }
        }
        let result = row
            .get("layout quality result")
            .map_or("", |value| value.trim());
        if !["PASS", "GAP", "BLOCKED", "NOT_APPLICABLE"].contains(&result) {
            issues.push(json!({"path":issue_path(path),"section":"Rendered Journey Usability","row":index,"field":"Layout quality result","expected":["BLOCKED","GAP","NOT_APPLICABLE","PASS"],"actual":result}));
        }
        gap |= ["GAP", "BLOCKED"].contains(&result);
        let platform = row.get("platform").map_or("", |value| value.trim());
        let platform_folded = platform.to_ascii_lowercase();
        if !["web", "native"].contains(&platform_folded.as_str()) {
            issues.push(json!({"path":issue_path(path),"section":"Rendered Journey Usability","row":index,"field":"Platform","expected":["native","web"],"actual":platform}));
        }
        let viewport = row.get("viewport").map_or("", String::as_str);
        web_desktop |=
            platform_folded == "web" && viewport.to_ascii_lowercase().contains("desktop");
        web_mobile |= platform_folded == "web" && mobile(viewport);
        native |= platform_folded == "native";
        let evidence = row.get("evidence").map_or("", String::as_str);
        if !evidence.trim().is_empty() && !evidence_text(evidence, true) {
            issues.push(json!({"path":issue_path(path),"section":"Rendered Journey Usability","row":index,"field":"Evidence","reason":"must name screenshot, DOM/viewport measurement, source evidence, or a concrete blocker"}));
        }
        let combined = row.values().cloned().collect::<Vec<_>>().join(" ");
        if visual_danger(&combined) && result == "PASS" {
            danger = true;
            issues.push(json!({"path":issue_path(path),"section":"Rendered Journey Usability","row":index,"field":"Layout quality result","reason":"danger terms such as overload, unreadable text, clipping, overflow, or low contrast cannot be marked PASS without a finding"}));
        }
        danger |= ["GAP", "BLOCKED"].contains(&result);
    }
    if ["web", "hybrid"].contains(&platform_scope) && (!web_desktop || !web_mobile) {
        issues.push(json!({"path":issue_path(path),"section":"Rendered Journey Usability","reason":"web platform requires desktop and mobile/narrow rendered rows"}));
    }
    if ["native", "hybrid"].contains(&platform_scope) && !native {
        issues.push(json!({"path":issue_path(path),"section":"Rendered Journey Usability","reason":"native platform requires at least one native rendered row"}));
    }
    if gap && findings.trim() == "No findings." {
        issues.push(json!({"path":issue_path(path),"section":"Findings","reason":"GAP or BLOCKED rendered journey usability rows require a finding"}));
    }
    if danger && !has_visual_danger_finding(findings) {
        issues.push(json!({"path":issue_path(path),"section":"Findings","reason":"rendered journey usability danger terms require a visual/usability finding"}));
    }
    issues
}

fn verify_visual_table(
    path: &Path,
    body: &str,
    findings: &str,
    platform_scope: &str,
) -> Vec<Value> {
    let rows = parse_markdown_table_dicts(body);
    if rows.is_empty() {
        return vec![
            json!({"path":issue_path(path),"section":"Visual Comparison Checks","reason":"missing visual comparison table"}),
        ];
    }
    let mut issues = Vec::new();
    let mut web_desktop = false;
    let mut web_mobile = false;
    let mut native = false;
    let mut relevant = false;
    let mut gap = false;
    let mut danger = false;
    for (offset, row) in rows.iter().enumerate() {
        let index = offset + 1;
        let missing = missing_columns(row, VISUAL_COMPARISON_COLUMNS);
        if !missing.is_empty() {
            issues.push(json!({"path":issue_path(path),"section":"Visual Comparison Checks","row":index,"missing_columns":missing}));
            continue;
        }
        for field in VISUAL_COMPARISON_COLUMNS {
            if row.get(*field).is_none_or(|value| value.trim().is_empty()) {
                issues.push(json!({"path":issue_path(path),"section":"Visual Comparison Checks","row":index,"field":field,"reason":"empty"}));
            }
        }
        let result = row.get("result").map_or("", |value| value.trim());
        if !["MATCHED", "GAP", "BLOCKED", "NOT_APPLICABLE"].contains(&result) {
            issues.push(json!({"path":issue_path(path),"section":"Visual Comparison Checks","row":index,"field":"Result","expected":["BLOCKED","GAP","MATCHED","NOT_APPLICABLE"],"actual":result}));
        }
        relevant |= result != "NOT_APPLICABLE";
        gap |= ["GAP", "BLOCKED"].contains(&result);
        let combined = row.values().cloned().collect::<Vec<_>>().join(" ");
        if result == "MATCHED" && visual_danger(&combined) {
            danger = true;
            issues.push(json!({"path":issue_path(path),"section":"Visual Comparison Checks","row":index,"field":"Result","reason":"danger terms cannot be marked MATCHED without a finding"}));
        }
        let platform = row
            .get("platform")
            .map_or("", |value| value.trim())
            .to_ascii_lowercase();
        if !["web", "native"].contains(&platform.as_str()) {
            issues.push(json!({"path":issue_path(path),"section":"Visual Comparison Checks","row":index,"field":"Platform","expected":["native","web"],"actual":platform}));
        }
        let viewport = row.get("viewport").map_or("", String::as_str);
        web_desktop |= platform == "web" && viewport.to_ascii_lowercase().contains("desktop");
        web_mobile |= platform == "web" && mobile(viewport);
        native |= platform == "native";
        let evidence = row
            .get("implementation screenshot/tool evidence")
            .map_or("", String::as_str);
        if !evidence.trim().is_empty() && !evidence_text(evidence, false) {
            issues.push(json!({"path":issue_path(path),"section":"Visual Comparison Checks","row":index,"field":"Implementation Screenshot/Tool Evidence","reason":"must name screenshot/trace/tool evidence or a concrete blocker"}));
        }
    }
    if relevant && ["web", "hybrid"].contains(&platform_scope) && (!web_desktop || !web_mobile) {
        issues.push(json!({"path":issue_path(path),"section":"Visual Comparison Checks","reason":"web platform requires desktop and mobile/narrow comparison rows"}));
    }
    if relevant && ["native", "hybrid"].contains(&platform_scope) && !native {
        issues.push(json!({"path":issue_path(path),"section":"Visual Comparison Checks","reason":"native platform requires at least one native comparison row"}));
    }
    if gap && findings.trim() == "No findings." {
        issues.push(json!({"path":issue_path(path),"section":"Findings","reason":"GAP or BLOCKED visual rows require a finding"}));
    }
    if danger && !has_visual_danger_finding(findings) {
        issues.push(json!({"path":issue_path(path),"section":"Findings","reason":"visual danger terms require a visual/usability finding"}));
    }
    issues
}

fn read_text(path: &Path, anchor: &Path, label: &str) -> Result<String, String> {
    let bytes = read_bytes_nofollow(path, Some(anchor))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{label} is missing: {}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{label} must be UTF-8: {}", path.display()))
}

fn exact_sections(actual: &[String], expected: &[&str]) -> bool {
    actual
        == expected
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
}

fn merge_issue(path: &Path, section: &str, unit: &str, detail: Value) -> Value {
    let mut object = detail.as_object().cloned().unwrap_or_default();
    object.insert("path".to_owned(), issue_path(path));
    object.insert("section".to_owned(), json!(section));
    object.insert("unit".to_owned(), json!(unit));
    Value::Object(object)
}

fn verify_batch_report(path: &Path, context: &VerifyContext, batch_id: &str) -> Vec<Value> {
    let text = match read_text(path, &context.root, "batch report") {
        Ok(text) => text,
        Err(error) => return vec![json!({"path":issue_path(path),"reason":error})],
    };
    let order = section_order(&text);
    let bodies = section_bodies(&text);
    let mut issues = Vec::new();
    if !exact_sections(&order, BATCH_SECTIONS) {
        issues.push(json!({"path":issue_path(path),"reason":"batch report sections must match required order","expected":BATCH_SECTIONS,"actual":order}));
    }
    let run_id = context.raw["run_id"].as_str().unwrap_or_default();
    if first_declared_value(bodies.get("run id").map_or("", String::as_str)) != run_id {
        issues.push(json!({"path":issue_path(path),"field":"Run ID","expected":run_id,"actual":first_declared_value(bodies.get("run id").map_or("",String::as_str))}));
    }
    if first_declared_value(bodies.get("batch id").map_or("", String::as_str)) != batch_id {
        issues.push(json!({"path":issue_path(path),"field":"Batch ID","expected":batch_id,"actual":first_declared_value(bodies.get("batch id").map_or("",String::as_str))}));
    }
    let expected_units = context
        .expected_by_batch
        .get(batch_id)
        .cloned()
        .unwrap_or_default();
    let expected_files = context
        .files_by_batch
        .get(batch_id)
        .cloned()
        .unwrap_or_default();
    let coverage =
        parse_markdown_table_dicts(bodies.get("file coverage").map_or("", String::as_str));
    if coverage.is_empty() {
        issues.push(json!({"path":issue_path(path),"section":"File Coverage","reason":"missing coverage table"}));
    }
    let covered = coverage
        .iter()
        .filter_map(|row| row.get("unit"))
        .cloned()
        .collect::<BTreeSet<_>>();
    let missing = expected_units
        .difference(&covered)
        .cloned()
        .collect::<Vec<_>>();
    let extra = covered
        .difference(&expected_units)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() || !extra.is_empty() {
        issues.push(json!({"path":issue_path(path),"section":"File Coverage","missing_units":missing,"extra_units":extra}));
    }
    for row in &coverage {
        let unit = row.get("unit").map_or("", String::as_str);
        if row.get("status").map(String::as_str) != Some("CHECKED") {
            issues.push(json!({"path":issue_path(path),"section":"File Coverage","unit":unit,"field":"Status","actual":row.get("status")}));
        }
        if let Some(expected) = context.unit_hashes.get(unit)
            && row.get("sha-256") != Some(expected)
        {
            issues.push(json!({"path":issue_path(path),"section":"File Coverage","unit":unit,"field":"SHA-256","expected":expected,"actual":row.get("sha-256")}));
        }
        if row
            .get("purpose")
            .is_none_or(|value| value.trim().is_empty())
        {
            issues.push(json!({"path":issue_path(path),"section":"File Coverage","unit":unit,"field":"Purpose","reason":"empty"}));
        }
    }
    let inventory =
        parse_markdown_table_dicts(bodies.get("ui source inventory").map_or("", String::as_str));
    if inventory.is_empty() {
        issues.push(json!({"path":issue_path(path),"section":"UI Source Inventory","reason":"missing UI source inventory table"}));
    } else {
        let actual = inventory[0].keys().cloned().collect::<BTreeSet<_>>();
        let expected = UI_SOURCE_COLUMNS
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<BTreeSet<_>>();
        if actual != expected {
            issues.push(json!({"path":issue_path(path),"section":"UI Source Inventory","reason":"UI source inventory headers must include exact handler/backend/permission/persistence/test reference columns","expected":expected,"actual":actual}));
        }
    }
    let mut missing_traces = Vec::new();
    for row in &inventory {
        let unit = row.get("unit").map_or("", String::as_str);
        let rel = row.get("file").map_or("", String::as_str);
        if !expected_units.contains(unit) {
            issues.push(json!({"path":issue_path(path),"section":"UI Source Inventory","unit":unit,"reason":"unit is outside this batch"}));
        }
        if !expected_files.contains(rel)
            || context
                .unit_to_file
                .get(unit)
                .is_some_and(|expected| expected != rel)
        {
            issues.push(json!({"path":issue_path(path),"section":"UI Source Inventory","file":rel,"reason":"file is outside this batch or mismatched to unit"}));
        }
        for field in [
            "surface",
            "visible element",
            "source evidence",
            "expected behavior",
            "actual implementation",
            "responsive/state notes",
        ] {
            if row.get(field).is_none_or(|value| value.trim().is_empty()) {
                issues.push(json!({"path":issue_path(path),"section":"UI Source Inventory","unit":unit,"field":field,"reason":"empty"}));
            }
        }
        for field in [
            "handler reference",
            "backend/api reference",
            "permission reference",
            "persistence reference",
            "test reference",
        ] {
            let (status, detail) = wiring_reference(
                &context.repo,
                row.get(field).map_or("", String::as_str),
                field,
            );
            if let Some(detail) = detail {
                issues.push(merge_issue(path, "UI Source Inventory", unit, detail));
            }
            if status == "missing" {
                missing_traces.push(json!({
                    "unit":unit,"visible_element":row.get("visible element"),"field":field,
                }));
            }
        }
    }
    let findings = bodies
        .get("implementation gap findings")
        .map_or("", String::as_str);
    if !missing_traces.is_empty() && findings.trim() == "No findings." {
        issues.push(json!({"path":issue_path(path),"section":"Implementation Gap Findings","reason":"missing handler/backend/permission/persistence/test wiring references require a finding","missing_traces":missing_traces}));
    }
    issues.extend(validate_findings(
        findings,
        &expected_files,
        path,
        "Implementation Gap Findings",
        false,
    ));
    for section in [
        "batch summary",
        "mockup and journey alignment",
        "no gap notes",
        "open questions",
    ] {
        if bodies
            .get(section)
            .is_none_or(|value| value.trim().is_empty())
        {
            issues.push(json!({"path":issue_path(path),"section":section,"reason":"empty"}));
        }
    }
    issues
}

fn extend_evidence_issue(issues: &mut Vec<Value>, path: &Path, detail: Value) {
    let mut object = detail.as_object().cloned().unwrap_or_default();
    object.insert("path".to_owned(), issue_path(path));
    object.insert("section".to_owned(), json!("Visual Evidence"));
    issues.push(Value::Object(object));
}

fn verify_aux_report(
    path: &Path,
    context: &VerifyContext,
    expected_sections: &[&str],
    worker: &str,
) -> Vec<Value> {
    let text = match read_text(path, &context.root, "worker report") {
        Ok(text) => text,
        Err(error) => return vec![json!({"path":issue_path(path),"reason":error})],
    };
    let order = section_order(&text);
    let bodies = section_bodies(&text);
    let mut issues = Vec::new();
    if !exact_sections(&order, expected_sections) {
        issues.push(json!({"path":issue_path(path),"reason":"worker report sections must match required order","expected":expected_sections,"actual":order}));
    }
    let run_id = context.raw["run_id"].as_str().unwrap_or_default();
    if first_declared_value(bodies.get("run id").map_or("", String::as_str)) != run_id {
        issues.push(json!({"path":issue_path(path),"field":"Run ID","expected":run_id,"actual":first_declared_value(bodies.get("run id").map_or("",String::as_str))}));
    }
    if first_declared_value(bodies.get("worker").map_or("", String::as_str)) != worker {
        issues.push(json!({"path":issue_path(path),"field":"Worker","expected":worker,"actual":first_declared_value(bodies.get("worker").map_or("",String::as_str))}));
    }
    let all_files = context
        .source_hashes
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let findings = bodies.get("findings").map_or("", String::as_str);
    issues.extend(validate_findings(
        findings, &all_files, path, "Findings", true,
    ));
    for section in expected_sections {
        if bodies
            .get(*section)
            .is_none_or(|value| value.trim().is_empty())
        {
            issues.push(json!({"path":issue_path(path),"section":section,"reason":"empty"}));
        }
    }
    if worker != "visual_comparison_audit" {
        return issues;
    }
    issues.extend(verify_journey_model(
        path,
        bodies
            .get("journey decision model")
            .map_or("", String::as_str),
    ));
    issues.extend(verify_rendered_usability(
        path,
        bodies
            .get("rendered journey usability")
            .map_or("", String::as_str),
        findings,
        &context.platform,
    ));
    issues.extend(verify_visual_table(
        path,
        bodies
            .get("visual comparison checks")
            .map_or("", String::as_str),
        findings,
        &context.platform,
    ));
    let checklist_text = [
        bodies.get("rendered journey usability"),
        bodies.get("visual comparison checks"),
        bodies.get("findings"),
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect::<Vec<_>>()
    .join("\n");
    let missing = interaction_checklist_missing(&checklist_text);
    if !missing.is_empty() {
        issues.push(json!({"path":issue_path(path),"section":"Interaction Checklist","reason":"visual comparison report must mark every interaction checklist label pass/gap/blocked/not-applicable","missing":missing}));
    }
    let visual_rows = parse_markdown_table_dicts(
        bodies
            .get("visual comparison checks")
            .map_or("", String::as_str),
    );
    let rendered_rows = visual_rows
        .iter()
        .filter(|row| {
            row.get("result")
                .is_some_and(|result| ["MATCHED", "GAP"].contains(&result.trim()))
        })
        .collect::<Vec<_>>();
    let (records, evidence_issues) = crate::audit_evidence::validate_visual_evidence_manifest(
        &context.root,
        run_id,
        !rendered_rows.is_empty(),
    );
    for issue in evidence_issues {
        extend_evidence_issue(&mut issues, path, issue);
    }
    issues.extend(verify_ux_reviews(
        path,
        &bodies,
        findings,
        &records,
        &context.platform,
    ));
    let formal_body = bodies.get("formal evidence").map_or("", String::as_str);
    let formal_refs = crate::audit_evidence::evidence_references(formal_body);
    let formal_kinds = formal_refs
        .iter()
        .filter_map(|reference| records.get(reference))
        .filter_map(|record| record.get("kind"))
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let web_required = ["web", "hybrid"].contains(&context.platform.as_str());
    let native_required = ["native", "hybrid"].contains(&context.platform.as_str());
    if web_required {
        if context.formal_config.is_none() {
            if !formal_body.to_ascii_lowercase().contains("blocked")
                || findings.trim() == "No findings."
            {
                issues.push(json!({"path":issue_path(path),"section":"Formal Evidence","reason":"missing manifest-bound formal config must be BLOCKED with a finding"}));
            }
        } else {
            let missing = [
                "formal-web-verifier",
                "journey-evidence",
                "review-queue",
                "manual-review",
            ]
            .into_iter()
            .filter(|kind| !formal_kinds.contains(kind))
            .collect::<Vec<_>>();
            if !missing.is_empty() {
                issues.push(json!({"path":issue_path(path),"section":"Formal Evidence","reason":"web/hybrid audit must cite imported formal evidence","missing_kinds":missing}));
            }
        }
    } else if formal_body.trim()
        != "Formal Web UI verification not applicable to declared native platform."
    {
        issues.push(json!({"path":issue_path(path),"section":"Formal Evidence","reason":"native audit must use the exact formal-web not-applicable statement"}));
    }
    if rendered_rows.is_empty() {
        return issues;
    }
    let mut required_kinds = BTreeSet::new();
    if web_required {
        required_kinds.insert("screenshot".to_owned());
    }
    if native_required {
        required_kinds.insert("native-snapshot".to_owned());
    }
    if web_required && context.formal_config.is_some() {
        required_kinds.extend(
            [
                "formal-web-verifier",
                "journey-evidence",
                "review-queue",
                "manual-review",
            ]
            .map(str::to_owned),
        );
    }
    for issue in crate::audit_evidence::validate_references(&text, &records, Some(&required_kinds))
    {
        extend_evidence_issue(&mut issues, path, issue);
    }
    for (offset, row) in rendered_rows.iter().enumerate() {
        let references = crate::audit_evidence::evidence_references(
            row.get("implementation screenshot/tool evidence")
                .map_or("", String::as_str),
        );
        let row_platform = row
            .get("platform")
            .map_or("", String::as_str)
            .trim()
            .to_ascii_lowercase();
        let screenshots = references
            .iter()
            .filter_map(|reference| records.get(reference))
            .filter(|record| {
                let kind = record.get("kind").and_then(Value::as_str).unwrap_or("");
                match row_platform.as_str() {
                    "web" => kind == "screenshot",
                    "native" => kind == "native-snapshot",
                    _ => ["screenshot", "native-snapshot"].contains(&kind),
                }
            })
            .collect::<Vec<_>>();
        if screenshots.is_empty() {
            issues.push(json!({"path":issue_path(path),"section":"Visual Evidence","row":offset+1,"reason":"each rendered comparison row must bind a real screenshot/native snapshot with evidence:<id>"}));
            continue;
        }
        let route = row
            .get("route/screen")
            .map_or("", String::as_str)
            .trim()
            .to_ascii_lowercase();
        if !screenshots.iter().any(|record| {
            record
                .get("route")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case(&route)
        }) {
            issues.push(json!({"path":issue_path(path),"section":"Visual Evidence","row":offset+1,"reason":"screenshot route metadata does not match the comparison row"}));
        }
        let viewport = row
            .get("viewport")
            .map_or("", String::as_str)
            .trim()
            .to_ascii_lowercase();
        if !screenshots.iter().any(|record| {
            let label = record
                .get("viewport")
                .and_then(Value::as_object)
                .and_then(|viewport| viewport.get("label"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            viewport.contains(&label) || label.contains(&viewport)
        }) {
            issues.push(json!({"path":issue_path(path),"section":"Visual Evidence","row":offset+1,"reason":"screenshot viewport metadata does not match the comparison row"}));
        }
    }
    issues
}

fn verify_final_report(context: &VerifyContext, visual_report: Option<&PathBuf>) -> Vec<Value> {
    let path = context.root.join("final-report.md");
    let text = match read_text(&path, &context.root, "final-report.md") {
        Ok(text) => text,
        Err(_) => {
            return vec![
                json!({"path":issue_path(&path),"reason":"final-report.md is missing or not a regular file"}),
            ];
        }
    };
    let order = section_order(&text);
    let bodies = section_bodies(&text);
    let mut issues = Vec::new();
    if !exact_sections(&order, FINAL_SECTIONS) {
        issues.push(json!({"path":issue_path(&path),"reason":"final report sections must match required order","expected":FINAL_SECTIONS,"actual":order}));
    }
    for section in FINAL_SECTIONS {
        if bodies
            .get(*section)
            .is_none_or(|body| body.trim().is_empty())
        {
            issues.push(json!({"path":issue_path(&path),"section":section,"reason":"empty"}));
        }
    }
    let coverage = bodies.get("coverage").map_or("", String::as_str);
    let run_id = context.raw["run_id"].as_str().unwrap_or_default();
    if !coverage.contains(run_id) {
        issues.push(json!({"path":issue_path(&path),"section":"Coverage","reason":"must name the audit run id"}));
    }
    if !coverage.to_ascii_lowercase().contains(&context.platform) {
        issues.push(json!({"path":issue_path(&path),"section":"Coverage","reason":"must state the declared UI platform"}));
    }
    let missing = interaction_checklist_missing(
        bodies
            .get("accessibility and interaction findings")
            .map_or("", String::as_str),
    );
    if !missing.is_empty() {
        issues.push(json!({"path":issue_path(&path),"section":"Accessibility And Interaction Findings","reason":"final report must mark every interaction checklist label","missing":missing}));
    }
    let web_required = ["web", "hybrid"].contains(&context.platform.as_str());
    let native_required = ["native", "hybrid"].contains(&context.platform.as_str());
    let evidence_required = native_required || context.formal_config.is_some();
    let (records, evidence_issues) = crate::audit_evidence::validate_visual_evidence_manifest(
        &context.root,
        run_id,
        evidence_required,
    );
    for issue in evidence_issues {
        extend_evidence_issue(&mut issues, &path, issue);
    }
    issues.extend(verify_journey_model(
        &path,
        bodies
            .get("journey decision model")
            .map_or("", String::as_str),
    ));
    issues.extend(verify_ux_reviews(
        &path,
        &bodies,
        &text,
        &records,
        &context.platform,
    ));
    if !finding_blocks(&text).is_empty() {
        let all_files = context
            .source_hashes
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        issues.extend(validate_findings(
            &text,
            &all_files,
            &path,
            "UX finding synthesis",
            true,
        ));
    }
    if let Some(visual_report) = visual_report
        && let Ok(worker_text) = read_text(visual_report, &context.root, "visual comparison report")
    {
        issues.extend(verify_ux_synthesis(
            &path,
            &bodies,
            &section_bodies(&worker_text),
        ));
    }
    let mut required = BTreeSet::new();
    if web_required {
        required.insert("screenshot".to_owned());
    }
    if native_required {
        required.insert("native-snapshot".to_owned());
    }
    if evidence_required {
        for issue in crate::audit_evidence::validate_references(&text, &records, Some(&required)) {
            extend_evidence_issue(&mut issues, &path, issue);
        }
    }
    let references = crate::audit_evidence::evidence_references(&text);
    let kinds = references
        .iter()
        .filter_map(|reference| records.get(reference))
        .filter_map(|record| record.get("kind"))
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    if web_required && context.formal_config.is_some() {
        let missing = [
            "formal-web-verifier",
            "journey-evidence",
            "review-queue",
            "manual-review",
        ]
        .into_iter()
        .filter(|kind| !kinds.contains(kind))
        .collect::<Vec<_>>();
        if !missing.is_empty() {
            issues.push(json!({"path":issue_path(&path),"section":"Visual Audit Findings","reason":"final report must cite the imported formal evidence chain","missing_kinds":missing}));
        }
    }
    if web_required && context.formal_config.is_none() {
        let formal = format!(
            "{}\n{}",
            coverage,
            bodies
                .get("visual audit findings")
                .map_or("", String::as_str)
        )
        .to_ascii_lowercase();
        if !formal.contains("formal") || !formal.contains("blocked") {
            issues.push(json!({"path":issue_path(&path),"section":"Visual Audit Findings","reason":"missing formal config must remain an explicit blocker"}));
        }
    }
    let verification = bodies
        .get("verification plan")
        .map_or("", String::as_str)
        .to_ascii_lowercase();
    if !["runtime", "test", "blocked", "not applicable"]
        .iter()
        .any(|token| verification.contains(token))
    {
        issues.push(json!({"path":issue_path(&path),"section":"Verification Plan","reason":"must name runtime/test proof or a concrete blocker/non-applicability"}));
    }
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("source-only proves") || lowered.contains("path#symbol proves") {
        issues.push(json!({"path":issue_path(&path),"reason":"source wiring references must not be represented as observable outcome proof"}));
    }
    issues
}

fn load_object(path: &Path, root: &Path, label: &str) -> Result<Map<String, Value>, String> {
    let bytes = read_bytes_nofollow(path, Some(root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{label} is missing"))?;
    crate::audit_findings::strict_json_object(&bytes, label)
}

fn verify_marker(context: &VerifyContext) -> Vec<Value> {
    let path = context.root.join("queue_complete.json");
    let marker = match load_object(&path, &context.root, "queue_complete.json") {
        Ok(marker) => marker,
        Err(_) => {
            return vec![
                json!({"path":issue_path(&path),"reason":"queue_complete.json is missing"}),
            ];
        }
    };
    let expected = BTreeMap::from([
        ("run_id", context.raw["run_id"].clone()),
        ("phase", json!("queue_generated")),
        ("audit_verified", json!(false)),
        ("audit_kind", json!("ui-implementation")),
        ("manifest", json!("manifest.json")),
        ("audit_index", json!("audit_index.md")),
        ("execution_ledger", json!("execution_ledger.json")),
        ("excluded_files", json!("excluded_files.json")),
        ("reports_dir", json!("reports")),
        (
            "ownership_marker",
            json!(".ui-implementation-audit-artifacts.json"),
        ),
        ("batch_count", context.raw["batch_count"].clone()),
        (
            "source_file_count",
            context.raw["source_file_count"].clone(),
        ),
    ]);
    expected
        .into_iter()
        .filter_map(|(field, expected)| {
            (marker.get(field) != Some(&expected)).then(|| {
                json!({"path":issue_path(&path),"field":field,"expected":expected,"actual":marker.get(field)})
            })
        })
        .collect()
}

fn verify_excluded(context: &VerifyContext) -> Result<Vec<Value>, String> {
    let path = context.root.join("excluded_files.json");
    let excluded = crate::audit_common::load_json_list(&path, "excluded_files.json")?;
    let mut issues = Vec::new();
    if context
        .raw
        .get("excluded_file_count")
        .and_then(Value::as_u64)
        != Some(excluded.len() as u64)
    {
        issues.push(json!({"path":issue_path(&path),"field":"excluded_file_count","expected":context.raw.get("excluded_file_count"),"actual":excluded.len()}));
    }
    let digest = audit_queue::canonical_json_sha256(&Value::Array(excluded.clone()))?;
    if context.raw.get("excluded_files_sha256") != Some(&json!(digest)) {
        issues.push(json!({"path":issue_path(&path),"field":"excluded_files_sha256","expected":context.raw.get("excluded_files_sha256"),"actual":digest}));
    }
    let warnings = excluded
        .into_iter()
        .filter(|item| item.get("scope_warning") == Some(&json!(true)))
        .collect::<Vec<_>>();
    if !warnings.is_empty() {
        issues.push(json!({"path":issue_path(&path),"reason":"unresolved scope warnings","scope_warnings":warnings}));
    }
    Ok(issues)
}

fn forbidden_effort_fields(value: &Value, prefix: &str, fields: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_prefix = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if [
                    "required_reasoning_effort",
                    "actual_reasoning_effort",
                    "can_set_reasoning_effort",
                ]
                .contains(&key.as_str())
                {
                    fields.push(child_prefix.clone());
                }
                forbidden_effort_fields(child, &child_prefix, fields);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                forbidden_effort_fields(child, &format!("{prefix}[{index}]"), fields);
            }
        }
        _ => {}
    }
}

fn nonempty(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn verify_execution_ledger(context: &VerifyContext) -> Vec<Value> {
    let path = context.root.join("execution_ledger.json");
    let ledger = match load_object(&path, &context.root, "execution_ledger.json") {
        Ok(ledger) => ledger,
        Err(_) => {
            return vec![
                json!({"path":issue_path(&path),"reason":"execution_ledger.json is missing"}),
            ];
        }
    };
    let mut issues = Vec::new();
    if ledger.get("run_id") != context.raw.get("run_id") {
        issues.push(json!({"path":issue_path(&path),"field":"run_id","expected":context.raw.get("run_id"),"actual":ledger.get("run_id")}));
    }
    let mut forbidden = Vec::new();
    forbidden_effort_fields(&Value::Object(ledger.clone()), "", &mut forbidden);
    if !forbidden.is_empty() {
        issues.push(json!({"path":issue_path(&path),"reason":"UI audit execution ledger must not prescribe or validate worker reasoning effort","fields":forbidden}));
    }
    let capability = ledger
        .get("worker_capability_check")
        .and_then(Value::as_object);
    if let Some(capability) = capability {
        if capability.get("status") != Some(&json!("completed")) {
            issues.push(json!({"path":issue_path(&path),"field":"worker_capability_check.status","expected":"completed","actual":capability.get("status")}));
        }
        if !nonempty(capability.get("spawn_tool")) {
            issues.push(json!({"path":issue_path(&path),"field":"worker_capability_check.spawn_tool","expected":"non-empty string","actual":capability.get("spawn_tool")}));
        }
    } else {
        issues.push(json!({"path":issue_path(&path),"field":"worker_capability_check","reason":"must be an object"}));
    }
    let lead = ledger.get("lead").and_then(Value::as_object);
    let lead_status = lead
        .and_then(|lead| lead.get("status"))
        .and_then(Value::as_str);
    if !lead_status.is_some_and(|status| {
        ["completed", "confirmed", "manual-fallback-completed"].contains(&status)
    }) {
        issues.push(json!({"path":issue_path(&path),"field":"lead.status","expected":"completed/confirmed","actual":lead_status}));
    }
    if !nonempty(lead.and_then(|lead| lead.get("runtime_provenance"))) {
        issues.push(json!({"path":issue_path(&path),"field":"lead.runtime_provenance","expected":"non-empty string","actual":lead.and_then(|lead|lead.get("runtime_provenance"))}));
    }
    let workers = match ledger.get("batch_workers").and_then(Value::as_array) {
        Some(workers) => workers,
        None => {
            issues.push(
                json!({"path":issue_path(&path),"field":"batch_workers","reason":"must be a list"}),
            );
            return issues;
        }
    };
    let by_id = workers
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|worker| {
            worker
                .get("batch_id")
                .and_then(Value::as_str)
                .map(|id| (id, worker))
        })
        .collect::<BTreeMap<_, _>>();
    for batch in &context.batches {
        let id = batch["id"].as_str().unwrap_or_default();
        let Some(row) = by_id.get(id) else {
            issues.push(json!({"path":issue_path(&path),"field":"batch_workers","missing":id}));
            continue;
        };
        let status = row
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !["completed", "manual-fallback-completed"].contains(&status) {
            issues.push(
                json!({"path":issue_path(&path),"batch_id":id,"field":"status","actual":status}),
            );
        } else {
            if status != "manual-fallback-completed" && !nonempty(row.get("agent_id")) {
                issues.push(json!({"path":issue_path(&path),"batch_id":id,"field":"agent_id","expected":"non-empty string","actual":row.get("agent_id")}));
            }
            if !nonempty(row.get("runtime_provenance")) {
                issues.push(json!({"path":issue_path(&path),"batch_id":id,"field":"runtime_provenance","expected":"non-empty string","actual":row.get("runtime_provenance")}));
            }
        }
    }
    let audit = context.raw["ui_implementation_audit"].as_object().unwrap();
    let mut visual_workers = vec!["visual_comparison_worker"];
    if audit
        .get("mockup_asset_prompt")
        .is_some_and(Value::is_string)
    {
        visual_workers.push("mockup_asset_worker");
    }
    if audit
        .get("visual_tooling_prompt")
        .is_some_and(Value::is_string)
    {
        visual_workers.push("visual_tooling_worker");
    }
    for key in visual_workers {
        let row = ledger.get(key).and_then(Value::as_object);
        let status = row
            .and_then(|row| row.get("status"))
            .and_then(Value::as_str);
        if !status
            .is_some_and(|status| ["completed", "manual-fallback-completed"].contains(&status))
        {
            issues.push(
                json!({"path":issue_path(&path),"field":format!("{key}.status"),"actual":status}),
            );
        } else if let Some(row) = row {
            if status != Some("manual-fallback-completed") && !nonempty(row.get("agent_id")) {
                issues.push(json!({"path":issue_path(&path),"field":format!("{key}.agent_id"),"expected":"non-empty string","actual":row.get("agent_id")}));
            }
            if !nonempty(row.get("runtime_provenance")) {
                issues.push(json!({"path":issue_path(&path),"field":format!("{key}.runtime_provenance"),"expected":"non-empty string","actual":row.get("runtime_provenance")}));
            }
        }
    }
    issues
}

fn verify_current_hashes(context: &VerifyContext) -> Vec<Value> {
    let mut expected = context.source_hashes.clone();
    expected.extend(context.asset_hashes.clone());
    expected.extend(context.requirement_hashes.clone());
    expected
        .into_iter()
        .filter_map(|(rel, digest)| {
            let path = context.repo.join(&rel);
            match read_bytes_nofollow(&path, Some(&context.repo)) {
                Ok(Some(_)) => match sha256_file(&path) {
                    Ok(actual) if actual == digest => None,
                    Ok(actual) => Some(json!({"path":rel,"expected":digest,"actual":actual,"reason":"current input hash differs from manifest"})),
                    Err(error) => Some(json!({"path":issue_path(&path),"reason":error})),
                },
                _ => Some(json!({"path":issue_path(&path),"reason":"manifest input file is missing"})),
            }
        })
        .collect()
}

pub fn verify(
    manifest_path: &Path,
    reports: &[PathBuf],
    skip_current_hash_check: bool,
) -> Result<Value, String> {
    let context = load_context(manifest_path)?;
    let report_paths = iter_report_files(reports)?;
    let reports_by_name = report_paths
        .into_iter()
        .filter_map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)?;
            Some((name, path))
        })
        .collect::<BTreeMap<_, _>>();
    let mut marker = verify_marker(&context);
    let mut excluded = verify_excluded(&context)?;
    let mut ledger = verify_execution_ledger(&context);
    let mut missing_reports = Vec::new();
    let mut report_issues = Vec::new();
    let mut final_report =
        verify_final_report(&context, reports_by_name.get("visual_comparison_audit.md"));
    let mut hashes = if skip_current_hash_check {
        Vec::new()
    } else {
        verify_current_hashes(&context)
    };
    for batch in &context.batches {
        let id = batch["id"].as_str().unwrap_or_default();
        let name = format!("{id}.md");
        if let Some(path) = reports_by_name.get(&name) {
            report_issues.extend(verify_batch_report(path, &context, id));
        } else {
            missing_reports.push(json!({"report":name}));
        }
    }
    let audit = context.raw["ui_implementation_audit"].as_object().unwrap();
    let mut auxiliary = vec![(
        "visual_comparison_audit.md",
        VISUAL_SECTIONS,
        "visual_comparison_audit",
    )];
    if audit
        .get("mockup_asset_prompt")
        .is_some_and(Value::is_string)
    {
        auxiliary.push((
            "mockup_asset_audit.md",
            MOCKUP_SECTIONS,
            "mockup_asset_audit",
        ));
    }
    if audit
        .get("visual_tooling_prompt")
        .is_some_and(Value::is_string)
    {
        auxiliary.push((
            "visual_tooling_audit.md",
            TOOLING_SECTIONS,
            "visual_tooling_audit",
        ));
    }
    for (name, sections, worker) in auxiliary {
        if let Some(path) = reports_by_name.get(name) {
            report_issues.extend(verify_aux_report(path, &context, sections, worker));
        } else {
            missing_reports.push(json!({"report":name}));
        }
    }
    let ok = marker.is_empty()
        && excluded.is_empty()
        && ledger.is_empty()
        && missing_reports.is_empty()
        && report_issues.is_empty()
        && final_report.is_empty()
        && hashes.is_empty();
    let issues = json!({
        "completion_marker_mismatches":std::mem::take(&mut marker),
        "excluded_file_issues":std::mem::take(&mut excluded),
        "execution_ledger_issues":std::mem::take(&mut ledger),
        "missing_reports":missing_reports,"report_issues":report_issues,
        "final_report_issues":std::mem::take(&mut final_report),
        "current_hash_mismatches":std::mem::take(&mut hashes),
    });
    Ok(json!({
        "ok":ok,"manifest":manifest_path.to_string_lossy(),
        "run_id":context.raw["run_id"],"issues":issues,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_queue::CollectOptions;
    use crate::ui_audit::{BuildOptions, ImportFormalOptions};

    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    ];
    const CHECKLIST: &str = "badge-detail=pass; row-hit-target=pass; navigation-cursor=pass; transient-disclosure=pass; disclosure-scrollbar=pass; icon-meaning=pass; stable-expansion-width=pass; hover-copy=pass; status-summary=pass; message-metadata=pass.";

    struct Fixture {
        _directory: tempfile::TempDir,
        repo: PathBuf,
        out: PathBuf,
        manifest: Value,
    }

    fn write(path: &Path, value: impl AsRef<[u8]>) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value).unwrap();
    }

    fn git(repo: &Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        let out = directory.path().join("out");
        write(
            &repo.join("src/App.tsx"),
            "export function Dashboard() { return <main><h1>Operations Dashboard</h1><button type=\"button\">Resolve incident</button></main>; }",
        );
        write(
            &repo.join("docs/journeys.md"),
            "# Dashboard User Journey\nGoal: resolve the highest priority incident.\nPrimary action: Resolve incident.\nResponsive requirement: mobile keeps the incident first.\nAcceptance criteria: desktop and mobile screenshots match.\n",
        );
        write(&repo.join("design/mockups/dashboard.png"), PNG);
        write(
            &repo.join("formal-web-ui.json"),
            br#"{"targets":[{"route":"/dashboard"}]}"#,
        );
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        let manifest = crate::ui_audit::build(&BuildOptions {
            repo: repo.clone(),
            out: out.clone(),
            run_id: "audit-run".to_owned(),
            generated_at: "2026-09-04T00:00:00Z".to_owned(),
            archive_stamp: "20260904T000000Z".to_owned(),
            verifier_program: PathBuf::from("/usr/local/bin/devcoordinator2-tooling"),
            batch_size: 6,
            max_batch_bytes: 60_000,
            collection: CollectOptions {
                include_config: true,
                include_assets: true,
                include_files: BTreeSet::from(["src/App.tsx".to_owned()]),
                ..Default::default()
            },
            forced_mockups: BTreeSet::new(),
            forced_journey_files: BTreeSet::new(),
            implementation_evidence: BTreeMap::from([("src/App.tsx".to_owned(), None)]),
            split_visual_discovery: false,
            ui_platform: "web".to_owned(),
            formal_config: Some("formal-web-ui.json".to_owned()),
        })
        .unwrap();
        Fixture {
            _directory: directory,
            repo,
            out,
            manifest,
        }
    }

    fn complete_ledger(fixture: &Fixture) {
        let path = fixture.out.join("execution_ledger.json");
        let mut ledger: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        ledger["worker_capability_check"] =
            json!({"status":"completed","spawn_tool":"rust-self-test","notes":"fixture"});
        ledger["lead"] = json!({"status":"completed","agent_id":"rust-lead","runtime_provenance":"Rust fixture"});
        ledger["fallback"] = json!({"status":"not-used","reason":""});
        for worker in ledger["batch_workers"].as_array_mut().unwrap() {
            worker["status"] = json!("completed");
            worker["agent_id"] = json!("rust-worker");
            worker["runtime_provenance"] = json!("Rust fixture");
        }
        ledger["visual_comparison_worker"]["status"] = json!("completed");
        ledger["visual_comparison_worker"]["agent_id"] = json!("rust-visual");
        ledger["visual_comparison_worker"]["runtime_provenance"] = json!("Rust fixture");
        audit_queue::write_json(&path, &ledger).unwrap();
    }

    fn write_batch(fixture: &Fixture, findings: bool) {
        let batch = &fixture.manifest["batches"][0];
        let unit = &fixture.manifest["coverage_units"][0];
        let finding = if findings {
            "- Priority: P1\n- Files: src/App.tsx\n- Mockup/requirement evidence: dashboard journey requires incident resolution\n- Interface evidence: Resolve incident lacks handler and persistence wiring\n- Expected behavior/standard: primary action has observable success and failure behavior\n- Gap: handler, backend, persistence, and tests are missing\n- Suggested implementation direction: implement and test the resolution workflow"
        } else {
            "No findings."
        };
        let report = format!(
            "## Run ID\n{}\n\n## Batch ID\n{}\n\n## Batch Summary\nDashboard UI source.\n\n## File Coverage\n| Unit | Status | SHA-256 | Purpose |\n| --- | --- | --- | --- |\n| {} | CHECKED | {} | Defines dashboard UI |\n\n## UI Source Inventory\n| Unit | File | Surface | Visible Element | Source Evidence | Expected Behavior | Actual Implementation | Handler Reference | Backend/API Reference | Permission Reference | Persistence Reference | Test Reference | Responsive/State Notes |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n| {} | src/App.tsx | dashboard | Resolve incident | visible button label | resolve selected incident | visible control exists | missing | missing | not-applicable: fixture has no authenticated role model | missing | missing | desktop and mobile evidence required |\n\n## Mockup And Journey Alignment\nThe source exposes the dashboard action required by the journey.\n\n## Implementation Gap Findings\n{}\n\n## No Gap Notes\nThe visible source element is inventoried.\n\n## Open Questions\nNone.\n",
            fixture.manifest["run_id"].as_str().unwrap(),
            batch["id"].as_str().unwrap(),
            unit["unit_id"].as_str().unwrap(),
            unit["sha256"].as_str().unwrap(),
            unit["unit_id"].as_str().unwrap(),
            finding,
        );
        write(&fixture.out.join("reports/batch_001.md"), report);
    }

    fn import_formal(fixture: &Fixture) {
        let artifacts = fixture.out.join("artifacts");
        std::fs::create_dir(&artifacts).unwrap();
        let image_sha = {
            write(&artifacts.join("desktop.png"), PNG);
            write(&artifacts.join("desktop-full.png"), PNG);
            write(&artifacts.join("mobile.png"), PNG);
            write(&artifacts.join("mobile-full.png"), PNG);
            sha256_file(&artifacts.join("desktop.png")).unwrap()
        };
        let source = "a".repeat(64);
        let intent = "b".repeat(64);
        let cells = ["desktop-cell", "mobile-cell"];
        let queue = json!({
            "schemaVersion":1,"kind":"formal-web-ui-review-queue","runId":"formal-run",
            "entries":cells.iter().map(|cell|json!({
                "reviewCellKey":cell,"sourceFingerprint":source,"intentFingerprint":intent,
                "screenshots":{"viewport":{"sha256":image_sha},"fullPage":{"sha256":image_sha}}
            })).collect::<Vec<_>>()
        });
        audit_queue::write_json(&artifacts.join("queue.json"), &queue).unwrap();
        let queue_sha = sha256_file(&artifacts.join("queue.json")).unwrap();
        let pages = [
            (
                "desktop-cell",
                "desktop",
                1440,
                900,
                "desktop.png",
                "desktop-full.png",
            ),
            (
                "mobile-cell",
                "mobile",
                390,
                844,
                "mobile.png",
                "mobile-full.png",
            ),
        ]
        .into_iter()
        .map(|(cell, label, width, height, viewport, full)|json!({
            "cellId":cell,"outcome":"checked","metrics":{"visibleScrollbars":[]},
            "screenshots":{
                "viewport":{"path":artifacts.join(viewport),"mime":"image/png","sha256":image_sha,"width":1,"height":1},
                "fullPage":{"path":artifacts.join(full),"mime":"image/png","sha256":image_sha,"width":1,"height":1}},
            "review":{"reviewCellKey":cell,"sourceFingerprint":source,"intentFingerprint":intent},
            "viewport":{"name":label,"width":width,"height":height},
            "requestedPath":"/dashboard","finalPath":"/dashboard","target":{"stateName":"base"},
        })).collect::<Vec<_>>();
        let formal = json!({
            "schemaVersion":2,"runId":"formal-run","pages":pages,"findings":[],
            "coverage":{"checkedPages":2,"failed":false},
            "review":{"queueSha256":queue_sha,"cells":[]},
        });
        audit_queue::write_json(&artifacts.join("formal.json"), &formal).unwrap();
        let formal_sha = sha256_file(&artifacts.join("formal.json")).unwrap();
        let journey = json!({
            "schemaVersion":1,"kind":"formal-web-ui-journey-evidence","runId":"formal-run",
            "governedRunId":Value::Null,"governedCheck":Value::Null,
            "cells":cells.iter().map(|cell|json!({
                "cellId":cell,"targetName":"Dashboard","stateName":"base","outcome":"checked",
                "screenshots":{"viewport":{"path":"viewport.png","sha256":image_sha},"fullPage":{"path":"full.png","sha256":image_sha}}
            })).collect::<Vec<_>>()
        });
        audit_queue::write_json(&artifacts.join("journey.json"), &journey).unwrap();
        let review = json!({
            "schemaVersion":1,"kind":"formal-web-ui-manual-review","reviewedRunId":"formal-run",
            "reportSha256":formal_sha,"reviewQueueSha256":queue_sha,
            "decisions":cells.iter().map(|cell|json!({
                "reviewCellKey":cell,"decision":"pass","note":"","sourceFingerprint":source,
                "intentFingerprint":intent,"screenshots":{"viewportSha256":image_sha,"fullPageSha256":image_sha}
            })).collect::<Vec<_>>()
        });
        audit_queue::write_json(&artifacts.join("review.json"), &review).unwrap();
        crate::ui_audit::import_formal_evidence(&ImportFormalOptions {
            audit_root: fixture.out.clone(),
            run_id: fixture.manifest["run_id"].as_str().unwrap().to_owned(),
            formal_report: artifacts.join("formal.json"),
            journey_evidence: artifacts.join("journey.json"),
            review_queue: artifacts.join("queue.json"),
            manual_review: artifacts.join("review.json"),
        })
        .unwrap();
    }

    fn replace_section(report: &str, heading: &str, body: &str) -> String {
        let marker = format!("## {heading}\n");
        let start = report.find(&marker).unwrap() + marker.len();
        let end = report[start..]
            .find("\n## ")
            .map_or(report.len(), |offset| start + offset);
        format!("{}{}\n{}", &report[..start], body, &report[end..])
    }

    fn ux_fixture() -> String {
        let mut review = String::from(
            "| Journey ID | Requirement evidence | Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n\
             | review-incidents | docs/journeys.md | dashboard | review incidents | choose an incident | summary and severity | urgent status | resolve | archive detail | none |\n\n\
             ## Journey Flow Review\n\
             | Journey ID | Starting situation | Intended outcome | Observed path | Surfaces | Outcome evidence | Unnecessary effort | Simpler alternative | Result | Reason | Evidence | Finding |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n\
             | review-incidents | owner has an open incident | choose the urgent incident | 1. Open dashboard; 2. Read severity and choose incident | dashboard | evidence:formal-journey-evidence | None observed in the selection path | Keep severity beside each incident | PASS | The owner can decide in the existing collection | evidence:formal-desktop-cell-viewport | none |\n\n\
             ## UX Guideline Review\n\
             | Journey ID | Criterion | Guideline source | Surface | Item | User benefit | Observation | Result | Reason | Evidence | Finding |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        for criterion in crate::ui_audit::UX_CRITERIA {
            let contextual = crate::ui_audit::CONTEXT_CRITERIA.contains(criterion);
            let surface = if ["surface-purpose", "copy-purpose"].contains(criterion) || contextual {
                "dashboard"
            } else {
                "all"
            };
            let (item, observation, result, reason) = if *criterion == "copy-purpose" {
                (
                    "none",
                    "No supporting copy exists in the declared states",
                    "NOT_APPLICABLE",
                    "The labels already identify the actions; source and rendered inventory contain no helper text",
                )
            } else {
                (
                    "incident review",
                    "Summary and severity are visible beside the action",
                    "PASS",
                    "Users can make the required decision without leaving the collection",
                )
            };
            let evidence = if contextual {
                "evidence:formal-journey-evidence"
            } else {
                "evidence:formal-desktop-cell-viewport"
            };
            review.push_str(&format!("| review-incidents | {criterion} | universal UI guidelines and docs/journeys.md | {surface} | {item} | Choose which incident needs attention | {observation} | {result} | {reason} | {evidence} | none |\n"));
        }
        review.push_str(&configuration_fixture());
        review
    }

    fn configuration_fixture() -> String {
        let mut report = String::from(
            "\n## UI Configuration Contract\n\
             | Config ID | Platform | Theme | Viewport | Input mode | Journey IDs | Update journey | Requirement source |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- |\n\
             | wide-light | web | light | desktop | keyboard | review-incidents | none | docs/journeys.md at audit source |\n\
             | narrow-light | web | light | mobile | touch | review-incidents | none | docs/journeys.md at audit source |\n\
             \n## Interaction Coverage\n\
             | Journey ID | Config ID | Scenario | Target | Expected result | Observation | Result | Reason | Evidence | Finding |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        for configuration in ["wide-light", "narrow-light"] {
            for scenario in crate::ui_audit::INTERACTION_SCENARIOS {
                report.push_str(&format!("| review-incidents | {configuration} | {scenario} | /dashboard | Resolve an incident in context | The recorded journey demonstrates {scenario} | PASS | The expected state and input behavior were observed | evidence:formal-journey-evidence | none |\n"));
            }
        }
        report.push_str(
            "\n## Rendered Build Review\n\
             | Config ID | Target | Expected snapshot | Observed snapshot | Result | Reason | Evidence | Finding |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- |\n\
             | wide-light | https://preview.invalid/dashboard | build-42 | build-42 | PASS | The reachable app identifies the audited build | evidence:formal-journey-evidence | none |\n\
             | narrow-light | https://preview.invalid/dashboard | build-42 | build-42 | PASS | The reachable app identifies the audited build | evidence:formal-journey-evidence | none |\n\
             \n## Design Decision Review\n\
             | Review ID | Journey IDs | Applicability | Design target | Options | Selection | Authority | Distinctions | Result | Reason | Evidence | Finding |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n\
             | current-design | review-incidents | approved-design | current dashboard | none | current dashboard | confirmed selection decision | existing approved layout | PASS | Rendered structure follows the selected direction | evidence:formal-desktop-cell-viewport | none |\n",
        );
        report
    }

    fn write_visual_and_final(fixture: &Fixture) {
        let visual = format!(
            "## Run ID\n{run}\n\n## Worker\nvisual_comparison_audit\n\n## Mockup And Asset Inventory\nThe dashboard mockup and journey were reviewed.\n\n## Visual Tooling\nPlaywright formal evidence covers desktop and mobile.\n\n## Journey Decision Model\n| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unconfirmed assumptions |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| dashboard | review incidents | choose an incident | summary and severity | urgent status | resolve | archive detail | none |\n\n## Rendered Journey Usability\n| Platform | Viewport | Decision supported | Visible decision-driving content | Visible secondary/detail content | Detail access pattern | Readability/contrast evidence | Layout quality result | Evidence |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n| web | desktop | choose an incident | summary and action | archive | {checks} | screenshot evidence:formal-desktop-cell-viewport | PASS | browser viewport screenshot evidence:formal-desktop-cell-viewport |\n| web | mobile | choose an incident | summary and action | archive | secondary after primary | screenshot evidence:formal-mobile-cell-viewport | PASS | browser viewport screenshot evidence:formal-mobile-cell-viewport |\n\n## Visual Comparison Checks\n| Platform | Journey | Viewport | Route/Screen | Mockup/Requirement | Implementation Screenshot/Tool Evidence | Differences | Result |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| web | Dashboard review | desktop | /dashboard | dashboard mockup | playwright screenshot evidence:formal-desktop-cell-viewport | No material mismatch | MATCHED |\n| web | Dashboard review | mobile | /dashboard | dashboard journey | playwright screenshot evidence:formal-mobile-cell-viewport | No material mismatch | MATCHED |\n\n## Formal Evidence\nImported evidence:formal-web, evidence:formal-journey-evidence, evidence:formal-review-queue, and evidence:formal-manual-review; no critical findings.\n\n## Findings\nNo findings.\n\n## Open Questions\nNone.\n",
            run = fixture.manifest["run_id"].as_str().unwrap(),
            checks = CHECKLIST,
        );
        write(
            &fixture.out.join("reports/visual_comparison_audit.md"),
            replace_section(&visual, "Journey Decision Model", &ux_fixture()),
        );
        let final_report = format!(
            "## Coverage\nRun {run} audited the declared web UI platform.\n\n## Mockup And Requirement Inputs\nThe manifest-bound mockup and journey were reviewed.\n\n## Journey Decision Model\nThe primary decision is selecting an incident.\n\n## Rendered Journey Usability Findings\nDesktop and mobile evidence:formal-desktop-cell-viewport and evidence:formal-mobile-cell-viewport support the decision.\n\n## Visual Audit Findings\nEvidence: evidence:formal-web, evidence:formal-journey-evidence, evidence:formal-review-queue, and evidence:formal-manual-review.\n\n## Source Implementation Findings\nMissing wiring is recorded without claiming source-only outcome proof.\n\n## Journey And Responsive Findings\nDesktop and mobile constraints were reviewed.\n\n## Accessibility And Interaction Findings\n{checks} Evidence: evidence:formal-desktop-cell-viewport.\n\n## Implementation Plan\nImplement the reported missing wiring.\n\n## Verification Plan\nUse runtime interaction evidence and focused tests.\n",
            run = fixture.manifest["run_id"].as_str().unwrap(),
            checks = CHECKLIST,
        );
        write(
            &fixture.out.join("final-report.md"),
            replace_section(&final_report, "Journey Decision Model", &ux_fixture()),
        );
    }

    fn complete_fixture() -> Fixture {
        let fixture = fixture();
        complete_ledger(&fixture);
        write_batch(&fixture, true);
        import_formal(&fixture);
        write_visual_and_final(&fixture);
        fixture
    }

    #[test]
    fn complete_rust_native_web_audit_verifies() {
        let fixture = complete_fixture();
        let result = verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        )
        .unwrap();
        assert_eq!(result["ok"], true, "{result:#}");
    }

    #[test]
    fn ux_regression_screen_only_reports_are_incomplete() {
        let fixture = complete_fixture();
        let path = fixture.out.join("reports/visual_comparison_audit.md");
        let original = std::fs::read_to_string(&path).unwrap();
        let mut omit = false;
        let report = original
            .lines()
            .filter(|line| {
                if line.starts_with("## ") {
                    omit = matches!(*line, "## Journey Flow Review" | "## UX Guideline Review");
                }
                !omit
            })
            .collect::<Vec<_>>()
            .join("\n");
        write(&path, report);
        let result = verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        )
        .unwrap();
        assert_eq!(result["ok"], false, "{result:#}");
        assert!(result.to_string().contains("journey flow review"));
    }

    #[test]
    fn ux_regression_necessary_copy_and_absent_defects_are_not_dangers() {
        for observation in [
            "No helper text is displayed.",
            "Required helper text prevents entering the wrong account number.",
            "No duplicate summaries appear.",
            "The panel is not crowded.",
        ] {
            assert!(!visual_danger(observation), "{observation}");
        }
        for observation in [
            "Unnecessary helper text repeats every label.",
            "The panel is crowded.",
            "No clipping occurs, but the controls are crowded.",
        ] {
            assert!(visual_danger(observation), "{observation}");
        }
    }

    fn verify_fixture(fixture: &Fixture) -> Value {
        verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        )
        .unwrap()
    }

    #[test]
    fn contextual_regression_missing_configuration_contract_is_not_complete() {
        let fixture = complete_fixture();
        for path in [
            fixture.out.join("reports/visual_comparison_audit.md"),
            fixture.out.join("final-report.md"),
        ] {
            let mut report = std::fs::read_to_string(&path).unwrap();
            for section in [
                "UI Configuration Contract",
                "Interaction Coverage",
                "Rendered Build Review",
                "Design Decision Review",
            ] {
                if report.contains(&format!("## {section}\n")) {
                    report = replace_section(&report, section, "Not supplied.");
                }
            }
            write(&path, report);
        }
        let result = verify_fixture(&fixture);
        assert_eq!(
            result["ok"], false,
            "a screenshot-complete audit omitted the required configurations: {result:#}"
        );
    }

    #[test]
    fn contextual_regression_guidelines_are_individually_required() {
        for criterion in [
            "context-inheritance",
            "contextual-actions",
            "compact-choices",
            "progressive-disclosure",
            "overlay-behavior",
            "generated-results",
            "contextual-help",
        ] {
            assert!(
                crate::ui_audit::UX_CRITERIA.contains(&criterion),
                "missing concrete guideline: {criterion}"
            );
        }
    }

    #[test]
    fn contextual_controls_require_named_items_and_runtime_but_allow_reasoned_exceptions() {
        let fixture = complete_fixture();
        let path = fixture.out.join("reports/visual_comparison_audit.md");
        let original = std::fs::read_to_string(&path).unwrap();
        for criterion in crate::ui_audit::CONTEXT_CRITERIA {
            let row = original
                .lines()
                .find(|line| line.starts_with(&format!("| review-incidents | {criterion} |")))
                .unwrap();
            for changed in [
                String::new(),
                row.replace("| dashboard |", "| all |"),
                row.replace("| incident review |", "| none |"),
                row.replace(
                    "evidence:formal-journey-evidence",
                    "evidence:formal-desktop-cell-viewport",
                ),
            ] {
                write(&path, original.replace(row, &changed));
                assert_eq!(
                    verify_fixture(&fixture)["ok"],
                    false,
                    "{criterion}: {changed}"
                );
            }
        }
        for path in [path, fixture.out.join("final-report.md")] {
            let original = std::fs::read_to_string(&path).unwrap();
            let baseline = if path.ends_with("visual_comparison_audit.md") {
                std::fs::read_to_string(fixture.out.join("final-report.md")).unwrap()
            } else {
                original.clone()
            };
            let baseline_rows = section_bodies(&baseline)["ux guideline review"].clone();
            let mut rows = baseline_rows.clone();
            for row in baseline_rows
                .lines()
                .filter(|row| row.starts_with("| review-incidents |"))
            {
                if crate::ui_audit::CONTEXT_CRITERIA
                    .iter()
                    .any(|criterion| row.contains(&format!("| {criterion} |")))
                {
                    let changed = row.replace("| incident review |", "| none |")
                        .replace("| PASS |", "| NOT_APPLICABLE |")
                        .replace("Summary and severity are visible beside the action", "No such control is present in the source and rendered inventory")
                        .replace("Users can make the required decision without leaving the collection", "This read-only view has no editable or generated value; necessary labels remain visible");
                    rows = rows.replace(row, &changed);
                }
            }
            write(
                &path,
                replace_section(&original, "UX Guideline Review", &rows),
            );
        }
        let result = verify_fixture(&fixture);
        assert_eq!(
            result["ok"], true,
            "legitimate non-applicability must not force new controls: {result:#}"
        );
    }

    #[test]
    fn configuration_reviews_reject_missing_cells_stale_builds_and_screenshot_proof() {
        let fixture = complete_fixture();
        let path = fixture.out.join("reports/visual_comparison_audit.md");
        let original = std::fs::read_to_string(&path).unwrap();
        for scenario in crate::ui_audit::INTERACTION_SCENARIOS {
            let prefix = format!("| review-incidents | narrow-light | {scenario} |");
            let report = original
                .lines()
                .filter(|line| !line.starts_with(&prefix))
                .collect::<Vec<_>>()
                .join("\n");
            write(&path, report);
            let result = verify_fixture(&fixture);
            assert_eq!(result["ok"], false, "{scenario}: {result:#}");
            assert!(
                result
                    .to_string()
                    .contains("missing required interaction cell")
            );
        }
        let persistence = original
            .lines()
            .find(|line| line.starts_with("| review-incidents | narrow-light | persistence |"))
            .unwrap();
        let builds = section_bodies(&original)["rendered build review"].clone();
        for (report, expected) in [
            (
                original.replace(
                    "| wide-light | web | light |",
                    "| wide-light | web | unknown |",
                ),
                "must remain BLOCKED",
            ),
            (
                original.replace(
                    persistence,
                    &persistence.replace(
                        "evidence:formal-journey-evidence",
                        "evidence:formal-desktop-cell-viewport",
                    ),
                ),
                "platform-bound runtime observation",
            ),
            (
                original.replace(persistence, &format!("{persistence}\n{persistence}")),
                "duplicate interaction cell",
            ),
            (
                replace_section(
                    &original,
                    "Rendered Build Review",
                    &builds.replace("| build-42 | build-42 |", "| build-42 | build-41 |"),
                ),
                "matching snapshots",
            ),
            (
                replace_section(
                    &original,
                    "Rendered Build Review",
                    &builds.replace(
                        "evidence:formal-journey-evidence",
                        "evidence:formal-desktop-cell-viewport",
                    ),
                ),
                "runtime observation",
            ),
        ] {
            write(&path, report);
            let result = verify_fixture(&fixture);
            assert_eq!(result["ok"], false, "{expected}: {result:#}");
            assert!(result.to_string().contains(expected), "{result:#}");
        }
        let contract = section_bodies(&original)["ui configuration contract"].clone();
        let dark = "| wide-dark | web | dark | desktop | keyboard | review-incidents | none | docs/journeys.md at audit source |\n";
        write(
            &path,
            replace_section(
                &original,
                "UI Configuration Contract",
                &format!("{contract}\n{dark}"),
            ),
        );
        let result = verify_fixture(&fixture);
        assert_eq!(result["ok"], false, "{result:#}");
        assert!(result.to_string().contains("wide-dark"));
        assert!(
            result
                .to_string()
                .contains("missing required interaction cell")
        );
    }

    #[test]
    fn configuration_synthesis_preserves_snapshot_evidence_and_supported_combinations() {
        let fixture = complete_fixture();
        let path = fixture.out.join("final-report.md");
        let original = std::fs::read_to_string(&path).unwrap();
        for report in [
            original.replace("| build-42 | build-42 |", "| build-43 | build-43 |"),
            original
                .lines()
                .filter(|line| {
                    !line.starts_with("| narrow-light |")
                        && !line.starts_with("| review-incidents | narrow-light |")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            original.replace("| desktop | keyboard |", "| desktop | pointer |"),
        ] {
            write(&path, report);
            let result = verify_fixture(&fixture);
            assert_eq!(result["ok"], false, "{result:#}");
            assert!(
                result.to_string().contains("preserve the visual worker"),
                "{result:#}"
            );
        }
    }

    #[test]
    fn configuration_review_accepts_honest_blockers_and_optional_scenarios() {
        let records = BTreeMap::from([
            (
                "formal-journey-evidence".to_owned(),
                json!({"kind":"journey-evidence"}),
            ),
            (
                "formal-desktop-cell-viewport".to_owned(),
                json!({"kind":"screenshot"}),
            ),
        ]);
        let journeys = BTreeSet::from(["review-incidents".to_owned()]);
        let findings = BTreeSet::from(["UI-SUPPORT".to_owned()]);
        let mut blocked = section_bodies(&ux_fixture().replace("| light |", "| unknown |"));
        for section in ["interaction coverage", "rendered build review"] {
            let body = blocked[section]
                .replace("| PASS |", "| BLOCKED |")
                .replace(
                    "evidence:formal-journey-evidence | none |",
                    "none | UI-SUPPORT |",
                )
                .replace(
                    "The expected state and input behavior were observed",
                    "Supported themes are unconfirmed; execution cannot be attributed",
                )
                .replace(
                    "The reachable app identifies the audited build",
                    "The supported configuration is unknown; build identity cannot be confirmed",
                );
            blocked.insert(section.to_owned(), body);
        }
        blocked.insert(
            "journey flow review".to_owned(),
            blocked["journey flow review"].replace("| PASS |", "| BLOCKED |"),
        );
        let issues = verify_configuration_reviews(
            Path::new("review.md"),
            &blocked,
            &records,
            &findings,
            &journeys,
            "web",
        );
        assert!(
            issues.is_empty(),
            "honest blocked coverage is a valid audit, not a product pass: {issues:#?}"
        );

        let issues = verify_configuration_reviews(
            Path::new("review.md"),
            &blocked,
            &records,
            &BTreeSet::new(),
            &journeys,
            "web",
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.to_string().contains("existing complete Finding ID"))
        );
        blocked.insert(
            "journey flow review".to_owned(),
            blocked["journey flow review"].replace("| BLOCKED |", "| PASS |"),
        );
        let issues = verify_configuration_reviews(
            Path::new("review.md"),
            &blocked,
            &records,
            &findings,
            &journeys,
            "web",
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.to_string().contains("flow cannot PASS"))
        );

        let baseline = section_bodies(&ux_fixture());
        for scenario in crate::ui_audit::INTERACTION_SCENARIOS {
            let mut optional = baseline.clone();
            let prefix = format!("| review-incidents | narrow-light | {scenario} |");
            let row = baseline["interaction coverage"]
                .lines()
                .find(|row| row.starts_with(&prefix))
                .unwrap();
            let replacement = row.replace("| PASS |", "| NOT_APPLICABLE |").replace(
                "The expected state and input behavior were observed",
                "This read-only selection journey has no applicable editable or asynchronous state",
            );
            optional.insert(
                "interaction coverage".to_owned(),
                baseline["interaction coverage"].replace(row, &replacement),
            );
            let issues = verify_configuration_reviews(
                Path::new("review.md"),
                &optional,
                &records,
                &BTreeSet::new(),
                &journeys,
                "web",
            );
            assert_eq!(
                issues.is_empty(),
                *scenario != "completion",
                "{scenario}: {issues:#?}"
            );
        }
    }

    #[test]
    fn design_review_accepts_current_approvals_routine_fixes_and_missing_historical_mockups() {
        let fixture = complete_fixture();
        let visual_path = fixture.out.join("reports/visual_comparison_audit.md");
        let final_path = fixture.out.join("final-report.md");
        let original_visual = std::fs::read_to_string(&visual_path).unwrap();
        let original_final = std::fs::read_to_string(&final_path).unwrap();
        let designs = section_bodies(&original_visual)["design decision review"].clone();
        let alternatives = designs.replace("| approved-design |", "| alternatives |")
            .replace("| none | current dashboard |", "| list; split; board | split |")
            .replace("existing approved layout", "List prioritizes scan order; split keeps editing beside results; board groups work by state")
            .replace("confirmed selection decision", "explicit user authorization to select the strongest option");
        for review in [
            designs.clone(),
            designs.replace("| approved-design |", "| routine-fix |"),
            designs.replace("| approved-design |", "| unavailable |")
                .replace("| PASS |", "| NOT_APPLICABLE |")
                .replace("Rendered structure follows the selected direction", "No mockup was supplied; requirements and runtime evidence still establish the product audit"),
            alternatives.clone(),
        ] {
            write(&visual_path, replace_section(&original_visual, "Design Decision Review", &review));
            write(&final_path, replace_section(&original_final, "Design Decision Review", &review));
            let result = verify_fixture(&fixture);
            assert_eq!(result["ok"], true, "no dates or historical mockups are required: {result:#}");
        }
        for invalid in [
            alternatives.replace("list; split; board", "list; split"),
            alternatives.replace("| split | explicit", "| absent-option | explicit"),
            alternatives.replace("explicit user authorization to select the strongest option", "none"),
            alternatives.replace("List prioritizes scan order; split keeps editing beside results; board groups work by state", "none"),
        ] {
            write(&visual_path, replace_section(&original_visual, "Design Decision Review", &invalid));
            assert_eq!(verify_fixture(&fixture)["ok"], false);
        }
    }

    #[test]
    fn desktop_updates_require_every_scenario_and_native_runtime_evidence() {
        let report = ux_fixture()
            .replace("| web |", "| desktop:linux-x64 |")
            .replace(
                "| review-incidents | none | docs/journeys.md",
                "| review-incidents | review-incidents | docs/journeys.md",
            );
        let mut bodies = section_bodies(&report);
        let records = BTreeMap::from([
            (
                "formal-journey-evidence".to_owned(),
                json!({"kind":"journey-evidence"}),
            ),
            (
                "formal-desktop-cell-viewport".to_owned(),
                json!({"kind":"screenshot"}),
            ),
            (
                "native-trace".to_owned(),
                json!({"kind":"trace","platform":"desktop:linux-x64"}),
            ),
        ]);
        let journeys = BTreeSet::from(["review-incidents".to_owned()]);
        let issues = verify_configuration_reviews(
            Path::new("review.md"),
            &bodies,
            &records,
            &BTreeSet::new(),
            &journeys,
            "native",
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue["scenario"] == "update-startup")
        );
        assert!(issues.iter().any(|issue| {
            issue
                .to_string()
                .contains("platform-bound runtime observation")
        }));
        for config in ["wide-light", "narrow-light"] {
            for scenario in crate::ui_audit::UPDATE_SCENARIOS {
                bodies.get_mut("interaction coverage").unwrap().push_str(&format!("\n| review-incidents | {config} | {scenario} | isolated desktop app | Update safely | The expected update state is observed | PASS | Trace shows background progress and user control without lost work | evidence:native-trace | none |\n"));
            }
        }
        for section in ["interaction coverage", "rendered build review"] {
            let replacement = bodies[section]
                .replace("evidence:formal-journey-evidence", "evidence:native-trace");
            bodies.insert(section.to_owned(), replacement);
        }
        let issues = verify_configuration_reviews(
            Path::new("review.md"),
            &bodies,
            &records,
            &BTreeSet::new(),
            &journeys,
            "native",
        );
        assert!(issues.is_empty(), "{issues:#?}");
        let mut wrong_platform = records.clone();
        wrong_platform.get_mut("native-trace").unwrap()["platform"] = json!("desktop:windows-x64");
        let issues = verify_configuration_reviews(
            Path::new("review.md"),
            &bodies,
            &wrong_platform,
            &BTreeSet::new(),
            &journeys,
            "native",
        );
        assert!(issues.iter().any(|issue| {
            issue
                .to_string()
                .contains("platform-bound runtime observation")
        }));
        for scenario in crate::ui_audit::UPDATE_SCENARIOS {
            let mut missing = bodies.clone();
            missing.insert(
                "interaction coverage".to_owned(),
                bodies["interaction coverage"]
                    .lines()
                    .filter(|line| {
                        !line.starts_with(&format!(
                            "| review-incidents | narrow-light | {scenario} |"
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            let issues = verify_configuration_reviews(
                Path::new("review.md"),
                &missing,
                &records,
                &BTreeSet::new(),
                &journeys,
                "native",
            );
            assert!(
                issues.iter().any(|issue| issue["scenario"] == *scenario),
                "{scenario}: {issues:#?}"
            );
            let mut waived = bodies.clone();
            let prefix = format!("| review-incidents | narrow-light | {scenario} |");
            let row = bodies["interaction coverage"]
                .lines()
                .find(|row| row.starts_with(&prefix))
                .unwrap();
            waived.insert(
                "interaction coverage".to_owned(),
                bodies["interaction coverage"]
                    .replace(row, &row.replace("| PASS |", "| NOT_APPLICABLE |")),
            );
            let issues = verify_configuration_reviews(
                Path::new("review.md"),
                &waived,
                &records,
                &BTreeSet::new(),
                &journeys,
                "native",
            );
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.to_string().contains("invalid UX review result")),
                "{scenario}: {issues:#?}"
            );
        }
    }

    #[test]
    fn ux_review_rejects_missing_criteria_surfaces_and_outcome_proof() {
        let fixture = complete_fixture();
        let path = fixture.out.join("reports/visual_comparison_audit.md");
        let original = std::fs::read_to_string(&path).unwrap();
        let mutations = [
            (original.lines().filter(|line| !line.starts_with("| review-incidents | step-necessity |")).collect::<Vec<_>>().join("\n"), "missing required guideline assessment"),
            (original.replace("| dashboard | evidence:formal-journey-evidence |", "| dashboard; details | evidence:formal-journey-evidence |"), "each surface requires purpose, supporting-copy, and contextual-control review"),
            (original.replace("| dashboard | evidence:formal-journey-evidence |", "| dashboard | evidence:formal-desktop-cell-viewport |"), "PASS requires runtime outcome evidence"),
            (original.replace("| review-incidents | copy-purpose |", "| invented-journey | copy-purpose |"), "unknown journey or criterion"),
            (original.replace("| evidence:formal-desktop-cell-viewport | none |", "| evidence:invented-evidence | none |"), "unknown visual evidence ids"),
            (original.replace("| PASS | The owner can decide in the existing collection |", "| NOT_APPLICABLE | The owner can decide in the existing collection |"), "invalid UX review result"),
            (original.replace("| review-incidents | step-necessity |", "| review-incidents | optional-polish |"), "unknown journey or criterion"),
            (original.replace("The labels already identify the actions; source and rendered inventory contain no helper text", "none"), "concrete assessment rationale"),
            (original.replace("| evidence:formal-desktop-cell-viewport | none |", "| evidence:formal-manual-review | none |"), "registered observation evidence"),
            (original.replace("| None observed in the selection path |", "|  |"), "missing_or_empty_columns"),
        ];
        for (report, expected) in mutations {
            write(&path, report);
            let result = verify_fixture(&fixture);
            assert_eq!(result["ok"], false, "{expected}: {result:#}");
            assert!(
                result.to_string().contains(expected),
                "{expected}: {result:#}"
            );
        }
    }

    fn ux_gap_report(report: &str, blocked: bool) -> String {
        let status = if blocked { "BLOCKED" } else { "GAP" };
        let mut result = report.to_owned();
        for line in report
            .lines()
            .filter(|line| line.starts_with("| review-incidents |"))
        {
            if line.contains("| step-necessity |")
                || line.contains("| owner has an open incident |")
            {
                let changed = line
                    .replace("| PASS |", &format!("| {status} |"))
                    .replace("None observed in the selection path", "The owner must enter the incident number again")
                    .replace("2. Read severity and choose incident", "2. Read severity; 3. Re-enter the incident number on a separate form; 4. Choose incident")
                    .replace("Summary and severity are visible beside the action", "The mockup-matching flow requires repeated entry before selection")
                    .replace("Users can make the required decision without leaving the collection", "The extra entry step adds no user benefit and should be removed")
                    .trim_end_matches(" none |")
                    .to_owned() + " UX-001 |";
                result = result.replace(line, &changed);
            }
        }
        let finding = "- Priority: P2\n- Finding ID: UX-001\n- Files: src/App.tsx\n- Mockup/requirement evidence: docs/journeys.md; the mockup also contains this detour\n- Interface evidence: evidence:formal-desktop-cell-viewport and evidence:formal-journey-evidence\n- Expected behavior/standard: users choose an incident without repeating information\n- Gap: the rendered journey requires repeated entry before selection\n- Suggested implementation direction: retain the selection in context and remove the repeated entry\n";
        if result.contains("## Findings\n") {
            replace_section(&result, "Findings", finding)
        } else {
            replace_section(&result, "Implementation Plan", finding)
        }
    }

    #[test]
    fn ux_review_keeps_honest_gaps_and_blockers_but_rejects_unlinked_findings() {
        let fixture = complete_fixture();
        let visual_path = fixture.out.join("reports/visual_comparison_audit.md");
        let final_path = fixture.out.join("final-report.md");
        let visual = std::fs::read_to_string(&visual_path).unwrap();
        let final_report = std::fs::read_to_string(&final_path).unwrap();
        for blocked in [false, true] {
            let reviewed = ux_gap_report(&visual, blocked);
            write(&visual_path, &reviewed);
            write(&final_path, ux_gap_report(&final_report, blocked));
            let result = verify_fixture(&fixture);
            assert_eq!(result["ok"], true, "{result:#}");
            write(&visual_path, reviewed.replace("- Finding ID: UX-001\n", ""));
            let result = verify_fixture(&fixture);
            assert_eq!(result["ok"], false, "{result:#}");
            assert!(result.to_string().contains("existing complete Finding ID"));
        }
    }

    #[test]
    fn ux_review_rejects_final_synthesis_that_drops_or_clears_assessments() {
        let fixture = complete_fixture();
        let visual_path = fixture.out.join("reports/visual_comparison_audit.md");
        let final_path = fixture.out.join("final-report.md");
        let visual = std::fs::read_to_string(&visual_path).unwrap();
        let final_report = std::fs::read_to_string(&final_path).unwrap();
        write(&visual_path, ux_gap_report(&visual, false));
        let result = verify_fixture(&fixture);
        assert_eq!(result["ok"], false, "{result:#}");
        assert!(result.to_string().contains("cannot silently clear"));
        write(&visual_path, &visual);
        let shortened = final_report
            .lines()
            .filter(|line| !line.starts_with("| review-incidents | copy-purpose |"))
            .collect::<Vec<_>>()
            .join("\n");
        write(&final_path, shortened);
        let result = verify_fixture(&fixture);
        assert_eq!(result["ok"], false, "{result:#}");
        assert!(result.to_string().contains("preserve the visual worker"));
    }

    #[test]
    fn ux_review_accepts_necessary_guidance_and_justified_separate_surfaces() {
        let fixture = complete_fixture();
        for path in [
            fixture.out.join("reports/visual_comparison_audit.md"),
            fixture.out.join("final-report.md"),
        ] {
            let original = std::fs::read_to_string(&path).unwrap();
            let mut report = original.replace(
                "| dashboard | evidence:formal-journey-evidence |",
                "| dashboard; confirmation | evidence:formal-journey-evidence |",
            );
            let extra = "| review-incidents | surface-purpose | confirmed deletion requirement | confirmation | final review | Review the exact irreversible target | Target and consequence are shown | PASS | The additional surface serves the explicitly requested final decision | evidence:formal-desktop-cell-viewport | none |\n\
                         | review-incidents | copy-purpose | confirmed deletion requirement | confirmation | consequence explanation | Avoid deleting the wrong incident | Necessary helper text names the target and consequence | PASS | The text prevents a concrete user error rather than narrating implementation | evidence:formal-desktop-cell-viewport | none |\n";
            let mut extra = extra.to_owned();
            for criterion in crate::ui_audit::CONTEXT_CRITERIA {
                extra.push_str(&format!("| review-incidents | {criterion} | confirmed deletion requirement | confirmation | none | Keep final review focused | No such control exists in this confirmation | NOT_APPLICABLE | Source and rendered inventory contain only the target and confirmation actions | evidence:formal-desktop-cell-viewport | none |\n"));
            }
            report = report.replace(
                "\n## UI Configuration Contract",
                &format!("\n{extra}\n## UI Configuration Contract"),
            );
            write(&path, report);
        }
        let result = verify_fixture(&fixture);
        assert_eq!(result["ok"], true, "{result:#}");
    }

    #[test]
    fn verifier_catches_missing_findings_bad_wiring_and_incomplete_ledgers() {
        let fixture = complete_fixture();
        let batch = fixture.out.join("reports/batch_001.md");
        let original_batch = std::fs::read_to_string(&batch).unwrap();
        let finding_start = original_batch.find("- Priority: P1").unwrap();
        let finding_end = original_batch.find("\n\n## No Gap Notes").unwrap();
        let without_finding = format!(
            "{}No findings.{}",
            &original_batch[..finding_start],
            &original_batch[finding_end..]
        );
        write(&batch, without_finding);
        let missing = verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        )
        .unwrap();
        assert!(missing.to_string().contains("missing handler/backend"));

        write(
            &batch,
            original_batch.replace(
                "| missing | missing |",
                "| src/App.tsx#invented | missing |",
            ),
        );
        let invented = verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        )
        .unwrap();
        assert!(invented.to_string().contains("symbol/text is absent"));
        write(&batch, &original_batch);

        let ledger_path = fixture.out.join("execution_ledger.json");
        let original_ledger = std::fs::read(&ledger_path).unwrap();
        let mut ledger: Value = serde_json::from_slice(&original_ledger).unwrap();
        ledger["batch_workers"][0]["required_reasoning_effort"] = json!("low");
        audit_queue::write_json(&ledger_path, &ledger).unwrap();
        let forbidden = verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        )
        .unwrap();
        assert!(forbidden.to_string().contains("must not prescribe"));
        write(&ledger_path, original_ledger);
    }

    #[test]
    fn verifier_catches_source_and_visual_tampering_and_missing_reports() {
        let fixture = complete_fixture();
        let source = fixture.repo.join("src/App.tsx");
        let original_source = std::fs::read(&source).unwrap();
        write(&source, "export function Changed(){ return null; }");
        let drift = verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        );
        assert!(
            drift
                .unwrap_err()
                .contains("not a qualifying product UI source")
        );
        write(&source, original_source);

        let image = fixture.out.join("artifacts/desktop.png");
        let original_image = std::fs::read(&image).unwrap();
        write(&image, b"tampered");
        let tampered = verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        )
        .unwrap();
        assert_eq!(tampered["ok"], false);
        assert!(tampered.to_string().contains("sha256"));
        write(&image, original_image);

        std::fs::remove_file(fixture.out.join("reports/visual_comparison_audit.md")).unwrap();
        let missing = verify(
            &fixture.out.join("manifest.json"),
            &[fixture.out.join("reports")],
            false,
        )
        .unwrap();
        assert!(
            !missing["issues"]["missing_reports"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
