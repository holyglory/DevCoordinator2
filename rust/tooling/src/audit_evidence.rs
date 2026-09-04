//! Attested visual-evidence manifests shared by repository-audit skills.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::audit_findings::strict_json_object;
use crate::audit_ledger::{read_bytes_nofollow, validate_directory_nofollow};

pub const SCHEMA_VERSION: u64 = 1;
pub const VISUAL_EVIDENCE_FILENAME: &str = "visual_evidence.json";
const ALLOWED_KINDS: [&str; 8] = [
    "screenshot",
    "native-snapshot",
    "trace",
    "video",
    "formal-web-verifier",
    "review-queue",
    "manual-review",
    "journey-evidence",
];
const IMAGE_KINDS: [&str; 2] = ["screenshot", "native-snapshot"];

static EVIDENCE_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z][A-Za-z0-9_-]{0,63}$").expect("constant evidence-id regex")
});
static REFERENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bevidence:([A-Za-z][A-Za-z0-9_-]{0,63})\b")
        .expect("constant evidence-reference regex")
});
static SHA256_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").expect("constant SHA-256 regex"));

pub type EvidenceRecords = BTreeMap<String, Value>;
pub type EvidenceIssues = Vec<Value>;

pub fn evidence_references(text: &str) -> BTreeSet<String> {
    REFERENCE_RE
        .captures_iter(text)
        .map(|capture| capture[1].to_owned())
        .collect()
}

pub fn detected_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if data.starts_with(b"PK\x03\x04") {
        Some("application/zip")
    } else if data.len() >= 12 && data.get(4..8) == Some(b"ftyp") {
        Some("video/mp4")
    } else if std::str::from_utf8(data)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .is_some()
    {
        Some("application/json")
    } else {
        None
    }
}

fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut cursor = 2usize;
    while cursor + 9 < data.len() {
        if data[cursor] != 0xff {
            cursor += 1;
            continue;
        }
        let marker = data[cursor + 1];
        cursor += 2;
        if matches!(marker, 0xd8 | 0xd9) {
            continue;
        }
        let length = u16::from_be_bytes(data.get(cursor..cursor + 2)?.try_into().ok()?) as usize;
        if length < 2 || cursor + length > data.len() {
            return None;
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            let height = u16::from_be_bytes(data.get(cursor + 3..cursor + 5)?.try_into().ok()?);
            let width = u16::from_be_bytes(data.get(cursor + 5..cursor + 7)?.try_into().ok()?);
            return Some((u32::from(width), u32::from(height)));
        }
        cursor += length;
    }
    None
}

pub fn image_dimensions(data: &[u8], mime: &str) -> Option<(u32, u32)> {
    match mime {
        "image/png" if data.len() >= 24 => Some((
            u32::from_be_bytes(data[16..20].try_into().ok()?),
            u32::from_be_bytes(data[20..24].try_into().ok()?),
        )),
        "image/gif" if data.len() >= 10 => Some((
            u32::from(u16::from_le_bytes(data[6..8].try_into().ok()?)),
            u32::from(u16::from_le_bytes(data[8..10].try_into().ok()?)),
        )),
        "image/jpeg" => jpeg_dimensions(data),
        "image/webp" if data.len() >= 30 && data.get(12..16) == Some(b"VP8X") => {
            let width = u32::from_le_bytes([data[24], data[25], data[26], 0]) + 1;
            let height = u32::from_le_bytes([data[27], data[28], data[29], 0]) + 1;
            Some((width, height))
        }
        _ => None,
    }
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

fn object(
    data: &[u8],
    record_id: &str,
    label: &str,
) -> (Option<Map<String, Value>>, EvidenceIssues) {
    match strict_json_object(data, label) {
        Ok(payload) => (Some(payload), Vec::new()),
        Err(error) => (
            None,
            vec![json!({
                "record": record_id,
                "field": "path",
                "reason": format!("{label} is not valid JSON: {error}"),
            })],
        ),
    }
}

fn is_sha(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| SHA256_RE.is_match(value))
}

fn nonempty_text(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

fn validate_formal_report(
    data: &[u8],
    record_id: &str,
) -> (Option<Map<String, Value>>, EvidenceIssues) {
    let (payload, mut issues) = object(data, record_id, "formal verifier evidence");
    let Some(payload) = payload else {
        return (None, issues);
    };
    if payload.get("schemaVersion") != Some(&json!(2)) {
        issues.push(json!({
            "record": record_id,
            "field": "schemaVersion",
            "expected": 2,
            "actual": payload.get("schemaVersion").cloned().unwrap_or(Value::Null),
        }));
    }
    for (field, valid, type_name) in [
        (
            "runId",
            payload.get("runId").is_some_and(Value::is_string),
            "str",
        ),
        (
            "pages",
            payload.get("pages").is_some_and(Value::is_array),
            "list",
        ),
        (
            "findings",
            payload.get("findings").is_some_and(Value::is_array),
            "list",
        ),
        (
            "coverage",
            payload.get("coverage").is_some_and(Value::is_object),
            "dict",
        ),
    ] {
        if !valid {
            issues.push(json!({
                "record": record_id,
                "field": field,
                "reason": format!("formal verifier JSON requires {type_name}"),
            }));
        }
    }
    let coverage = payload.get("coverage").and_then(Value::as_object);
    let checked_pages = coverage
        .and_then(|coverage| coverage.get("checkedPages"))
        .and_then(Value::as_u64);
    if checked_pages.is_none_or(|checked| checked < 1) {
        issues.push(json!({
            "record": record_id,
            "field": "coverage.checkedPages",
            "reason": "formal verifier evidence must include at least one checked page",
        }));
    }
    if !coverage
        .and_then(|coverage| coverage.get("failed"))
        .is_some_and(Value::is_boolean)
    {
        issues.push(json!({
            "record": record_id,
            "field": "coverage.failed",
            "reason": "formal verifier evidence must preserve coverage status",
        }));
    }
    for (index, page) in payload
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(page) = page
            .as_object()
            .filter(|page| page.get("outcome") == Some(&json!("checked")))
        else {
            continue;
        };
        let metrics = page.get("metrics").and_then(Value::as_object);
        if !metrics
            .and_then(|metrics| metrics.get("visibleScrollbars"))
            .is_some_and(Value::is_array)
        {
            issues.push(json!({
                "record": record_id,
                "field": format!("pages[{index}].metrics.visibleScrollbars"),
                "reason": "checked formal-verifier pages must preserve the visible scrollbar inventory",
            }));
        }
        let screenshots = page.get("screenshots").and_then(Value::as_object);
        for name in ["viewport", "fullPage"] {
            let Some(screenshot) = screenshots
                .and_then(|screenshots| screenshots.get(name))
                .and_then(Value::as_object)
            else {
                issues.push(json!({
                    "record": record_id,
                    "field": format!("pages[{index}].screenshots.{name}"),
                    "reason": "checked pages require both screenshot evidence records",
                }));
                continue;
            };
            for field in ["path", "mime", "sha256", "width", "height"] {
                if !screenshot.contains_key(field) {
                    issues.push(json!({
                        "record": record_id,
                        "field": format!("pages[{index}].screenshots.{name}.{field}"),
                        "reason": "missing screenshot evidence field",
                    }));
                }
            }
            if !is_sha(screenshot.get("sha256")) {
                issues.push(json!({
                    "record": record_id,
                    "field": format!("pages[{index}].screenshots.{name}.sha256"),
                    "reason": "must be SHA-256",
                }));
            }
        }
        let review = page.get("review").and_then(Value::as_object);
        for field in ["reviewCellKey", "sourceFingerprint", "intentFingerprint"] {
            if !nonempty_text(review.and_then(|review| review.get(field))) {
                issues.push(json!({
                    "record": record_id,
                    "field": format!("pages[{index}].review.{field}"),
                    "reason": "checked pages require changed-review identity",
                }));
            }
        }
        for field in ["sourceFingerprint", "intentFingerprint"] {
            if !is_sha(review.and_then(|review| review.get(field))) {
                issues.push(json!({
                    "record": record_id,
                    "field": format!("pages[{index}].review.{field}"),
                    "reason": "must be SHA-256",
                }));
            }
        }
    }
    let review = payload.get("review").and_then(Value::as_object);
    if !is_sha(review.and_then(|review| review.get("queueSha256"))) {
        issues.push(json!({
            "record": record_id,
            "field": "review.queueSha256",
            "reason": "formal report must bind its changed visual-review queue",
        }));
    }
    if !review
        .and_then(|review| review.get("cells"))
        .is_some_and(Value::is_array)
    {
        issues.push(json!({
            "record": record_id,
            "field": "review.cells",
            "reason": "formal report must preserve changed-review cells",
        }));
    }
    (Some(payload), issues)
}

fn validate_review_queue(
    data: &[u8],
    record_id: &str,
) -> (Option<Map<String, Value>>, EvidenceIssues) {
    let (payload, mut issues) = object(data, record_id, "review queue");
    let Some(payload) = payload else {
        return (None, issues);
    };
    if payload.get("schemaVersion") != Some(&json!(1))
        || payload.get("kind") != Some(&json!("formal-web-ui-review-queue"))
    {
        issues.push(json!({"record":record_id,"field":"schema/kind","reason":"unsupported formal review queue"}));
    }
    if !nonempty_text(payload.get("runId")) {
        issues.push(
            json!({"record":record_id,"field":"runId","reason":"review queue requires a run id"}),
        );
    }
    let entries = payload.get("entries").and_then(Value::as_array);
    if entries.is_none() {
        issues.push(json!({"record":record_id,"field":"entries","reason":"review queue entries must be a list"}));
    }
    let mut seen = BTreeSet::new();
    for (index, item) in entries.into_iter().flatten().enumerate() {
        let Some(item) = item.as_object() else {
            issues.push(json!({"record":record_id,"field":format!("entries[{index}]"),"reason":"must be an object"}));
            continue;
        };
        let key = item.get("reviewCellKey").and_then(Value::as_str);
        if key.is_none_or(|key| key.is_empty() || !seen.insert(key.to_owned())) {
            issues.push(json!({"record":record_id,"field":format!("entries[{index}].reviewCellKey"),"reason":"must be unique and non-empty"}));
        }
        for field in ["sourceFingerprint", "intentFingerprint"] {
            if !is_sha(item.get(field)) {
                issues.push(json!({"record":record_id,"field":format!("entries[{index}].{field}"),"reason":"must be SHA-256"}));
            }
        }
        let screenshots = item.get("screenshots").and_then(Value::as_object);
        for name in ["viewport", "fullPage"] {
            if !is_sha(
                screenshots
                    .and_then(|screenshots| screenshots.get(name))
                    .and_then(Value::as_object)
                    .and_then(|screenshot| screenshot.get("sha256")),
            ) {
                issues.push(json!({"record":record_id,"field":format!("entries[{index}].screenshots.{name}"),"reason":"must bind screenshot SHA-256"}));
            }
        }
    }
    (Some(payload), issues)
}

fn validate_manual_review(
    data: &[u8],
    record_id: &str,
) -> (Option<Map<String, Value>>, EvidenceIssues) {
    let (payload, mut issues) = object(data, record_id, "manual review");
    let Some(payload) = payload else {
        return (None, issues);
    };
    if payload.get("schemaVersion") != Some(&json!(1))
        || payload.get("kind") != Some(&json!("formal-web-ui-manual-review"))
    {
        issues.push(json!({"record":record_id,"field":"schema/kind","reason":"unsupported formal manual-review manifest"}));
    }
    if !nonempty_text(payload.get("reviewedRunId")) {
        issues.push(json!({"record":record_id,"field":"reviewedRunId","reason":"manual review requires a reviewed run id"}));
    }
    for field in ["reportSha256", "reviewQueueSha256"] {
        if !is_sha(payload.get(field)) {
            issues.push(json!({"record":record_id,"field":field,"reason":"must be SHA-256"}));
        }
    }
    let decisions = payload.get("decisions").and_then(Value::as_array);
    if decisions.is_none_or(Vec::is_empty) {
        issues.push(json!({"record":record_id,"field":"decisions","reason":"manual review requires at least one decision"}));
    }
    let mut seen = BTreeSet::new();
    for (index, item) in decisions.into_iter().flatten().enumerate() {
        let Some(item) = item.as_object() else {
            issues.push(json!({"record":record_id,"field":format!("decisions[{index}]"),"reason":"must be an object"}));
            continue;
        };
        let key = item.get("reviewCellKey").and_then(Value::as_str);
        if key.is_none_or(|key| key.is_empty() || !seen.insert(key.to_owned())) {
            issues.push(json!({"record":record_id,"field":format!("decisions[{index}].reviewCellKey"),"reason":"must be unique and non-empty"}));
        }
        let decision = item.get("decision").and_then(Value::as_str);
        if !matches!(decision, Some("pass" | "gap" | "blocked")) {
            issues.push(json!({"record":record_id,"field":format!("decisions[{index}].decision"),"reason":"must be pass, gap, or blocked"}));
        }
        if decision != Some("pass")
            && item
                .get("note")
                .map(value_text)
                .unwrap_or_default()
                .trim()
                .len()
                < 2
        {
            issues.push(json!({"record":record_id,"field":format!("decisions[{index}].note"),"reason":"gap/blocked decisions require a note"}));
        }
        for field in ["sourceFingerprint", "intentFingerprint"] {
            if !is_sha(item.get(field)) {
                issues.push(json!({"record":record_id,"field":format!("decisions[{index}].{field}"),"reason":"must be SHA-256"}));
            }
        }
        let screenshots = item.get("screenshots").and_then(Value::as_object);
        for field in ["viewportSha256", "fullPageSha256"] {
            if !is_sha(screenshots.and_then(|screenshots| screenshots.get(field))) {
                issues.push(json!({"record":record_id,"field":format!("decisions[{index}].screenshots.{field}"),"reason":"must be SHA-256"}));
            }
        }
    }
    (Some(payload), issues)
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(value) => if *value { "True" } else { "False" }.to_owned(),
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn confined_relative(value: &str) -> bool {
    !value.is_empty()
        && !Path::new(value).is_absolute()
        && !Path::new(value).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn validate_journey_evidence(
    data: &[u8],
    record_id: &str,
) -> (Option<Map<String, Value>>, EvidenceIssues) {
    let (payload, mut issues) = object(data, record_id, "journey evidence");
    let Some(payload) = payload else {
        return (None, issues);
    };
    if payload.get("schemaVersion") != Some(&json!(1))
        || payload.get("kind") != Some(&json!("formal-web-ui-journey-evidence"))
    {
        issues.push(json!({"record":record_id,"field":"schema/kind","reason":"unsupported formal journey-evidence manifest"}));
    }
    if !nonempty_text(payload.get("runId")) {
        issues.push(json!({"record":record_id,"field":"runId","reason":"journey evidence requires a formal run id"}));
    }
    for field in ["governedRunId", "governedCheck"] {
        if let Some(value) = payload.get(field)
            && !value.is_null()
            && !nonempty_text(Some(value))
        {
            issues.push(json!({"record":record_id,"field":field,"reason":"governed identity must be null or non-empty text"}));
        }
    }
    let cells = payload.get("cells").and_then(Value::as_array);
    if cells.is_none_or(|cells| cells.len() > 512) {
        issues.push(json!({"record":record_id,"field":"cells","reason":"journey evidence cells must be a bounded list"}));
    }
    let mut seen = BTreeSet::new();
    for (index, cell) in cells
        .filter(|cells| cells.len() <= 512)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(cell) = cell.as_object() else {
            issues.push(json!({"record":record_id,"field":format!("cells[{index}]"),"reason":"must be an object"}));
            continue;
        };
        let cell_id = cell.get("cellId").and_then(Value::as_str);
        if cell_id.is_none_or(|cell_id| cell_id.is_empty() || !seen.insert(cell_id.to_owned())) {
            issues.push(json!({"record":record_id,"field":format!("cells[{index}].cellId"),"reason":"must be unique and non-empty"}));
        }
        for field in ["targetName", "stateName", "outcome"] {
            if !nonempty_text(cell.get(field)) {
                issues.push(json!({"record":record_id,"field":format!("cells[{index}].{field}"),"reason":"must be non-empty text"}));
            }
        }
        let Some(screenshots) = cell.get("screenshots").and_then(Value::as_object) else {
            issues.push(json!({"record":record_id,"field":format!("cells[{index}].screenshots"),"reason":"must preserve viewport/full-page evidence"}));
            continue;
        };
        for name in ["viewport", "fullPage"] {
            let Some(screenshot) = screenshots.get(name) else {
                continue;
            };
            let screenshot = screenshot.as_object();
            if !is_sha(screenshot.and_then(|screenshot| screenshot.get("sha256"))) {
                issues.push(json!({"record":record_id,"field":format!("cells[{index}].screenshots.{name}"),"reason":"must bind a screenshot SHA-256"}));
            }
            let raw_path = screenshot
                .and_then(|screenshot| screenshot.get("path"))
                .and_then(Value::as_str);
            if raw_path.is_none_or(|path| !confined_relative(path)) {
                issues.push(json!({"record":record_id,"field":format!("cells[{index}].screenshots.{name}.path"),"reason":"must be a confined path relative to the bundle"}));
            }
        }
    }
    (Some(payload), issues)
}

fn record_value(record: &Map<String, Value>, field: &str) -> Value {
    record.get(field).cloned().unwrap_or(Value::Null)
}

pub fn validate_visual_evidence_manifest(
    audit_root: &Path,
    expected_run_id: &str,
    required: bool,
) -> (EvidenceRecords, EvidenceIssues) {
    let root = match validate_directory_nofollow(audit_root) {
        Ok(root) => root,
        Err(error) => {
            return (
                BTreeMap::new(),
                vec![json!({
                    "path": audit_root.join(VISUAL_EVIDENCE_FILENAME),
                    "reason": format!("visual evidence root is unsafe: {error}"),
                })],
            );
        }
    };
    let path = root.join(VISUAL_EVIDENCE_FILENAME);
    let data = match read_bytes_nofollow(&path, Some(&root)) {
        Ok(Some(data)) => data,
        _ => {
            return (
                BTreeMap::new(),
                if required {
                    vec![json!({"path":path,"reason":"visual evidence manifest is missing"})]
                } else {
                    Vec::new()
                },
            );
        }
    };
    let payload = match strict_json_object(&data, "visual evidence manifest") {
        Ok(payload) => payload,
        Err(error) => {
            return (
                BTreeMap::new(),
                vec![
                    json!({"path":path,"reason":format!("visual evidence manifest is invalid JSON: {error}")}),
                ],
            );
        }
    };
    let mut issues = Vec::new();
    if payload.get("schema_version") != Some(&json!(SCHEMA_VERSION)) {
        issues.push(json!({
            "path":path,
            "field":"schema_version",
            "expected":SCHEMA_VERSION,
            "actual":payload.get("schema_version").cloned().unwrap_or(Value::Null),
        }));
    }
    if payload.get("run_id") != Some(&json!(expected_run_id)) {
        issues.push(json!({
            "path":path,
            "field":"run_id",
            "expected":expected_run_id,
            "actual":payload.get("run_id").cloned().unwrap_or(Value::Null),
        }));
    }
    let Some(artifacts) = payload.get("artifacts").and_then(Value::as_array) else {
        issues.push(json!({"path":path,"field":"artifacts","reason":"must be a list"}));
        return (BTreeMap::new(), issues);
    };
    let mut records = BTreeMap::new();
    let mut actual_shas = BTreeMap::new();
    let mut formal_payloads = BTreeMap::new();
    let mut queue_payloads = BTreeMap::new();
    let mut review_payloads = BTreeMap::new();
    let mut journey_payloads = BTreeMap::new();

    for (index, record) in artifacts.iter().enumerate() {
        let Some(record) = record.as_object() else {
            issues.push(
                json!({"path":path,"record":index,"reason":"artifact record must be an object"}),
            );
            continue;
        };
        let Some(record_id) = record
            .get("id")
            .and_then(Value::as_str)
            .filter(|record_id| EVIDENCE_ID_RE.is_match(record_id))
        else {
            issues.push(
                json!({"path":path,"record":index,"field":"id","reason":"invalid evidence id"}),
            );
            continue;
        };
        if records.contains_key(record_id) {
            issues.push(json!({"path":path,"record":record_id,"reason":"duplicate evidence id"}));
            continue;
        }
        records.insert(record_id.to_owned(), Value::Object(record.clone()));
        let kind = record.get("kind").and_then(Value::as_str);
        if kind.is_none_or(|kind| !ALLOWED_KINDS.contains(&kind)) {
            let mut expected = ALLOWED_KINDS.to_vec();
            expected.sort();
            issues.push(json!({
                "path":path,"record":record_id,"field":"kind",
                "expected":expected,"actual":record_value(record,"kind"),
            }));
        }
        let Some(raw_artifact_path) = record
            .get("path")
            .and_then(Value::as_str)
            .filter(|value| confined_relative(value))
        else {
            issues.push(json!({"path":path,"record":record_id,"field":"path","reason":"must be a confined relative path"}));
            continue;
        };
        let artifact_path = root.join(raw_artifact_path);
        let artifact_data = match read_bytes_nofollow(&artifact_path, Some(&root)) {
            Ok(Some(data)) => data,
            _ => {
                issues.push(json!({"path":path,"record":record_id,"field":"path","reason":"artifact must be an existing regular non-symlink file inside the audit output"}));
                continue;
            }
        };
        let actual_sha = sha256_hex(&artifact_data);
        actual_shas.insert(record_id.to_owned(), actual_sha.clone());
        if !is_sha(record.get("sha256")) || record.get("sha256") != Some(&json!(actual_sha)) {
            issues.push(json!({
                "path":path,"record":record_id,"field":"sha256",
                "expected":actual_sha,"actual":record_value(record,"sha256"),
            }));
        }
        let actual_mime = detected_mime(&artifact_data);
        if record.get("mime").and_then(Value::as_str) != actual_mime {
            issues.push(json!({
                "path":path,"record":record_id,"field":"mime",
                "expected":actual_mime,"actual":record_value(record,"mime"),
            }));
        }
        let metadata_fields: &[&str] = if matches!(
            kind,
            Some("review-queue" | "manual-review" | "journey-evidence")
        ) {
            &["captured_by"]
        } else {
            &["route", "state", "captured_by"]
        };
        for field in metadata_fields {
            if record
                .get(*field)
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().len() < 2)
            {
                issues.push(json!({"path":path,"record":record_id,"field":field,"reason":"must be a non-empty metadata string"}));
            }
        }
        if !matches!(
            kind,
            Some("review-queue" | "manual-review" | "journey-evidence")
        ) {
            let viewport = record.get("viewport").and_then(Value::as_object);
            if viewport.is_none() {
                issues.push(json!({"path":path,"record":record_id,"field":"viewport","reason":"must be an object"}));
            } else if let Some(viewport) = viewport {
                for field in ["width", "height"] {
                    if viewport
                        .get(field)
                        .and_then(Value::as_u64)
                        .is_none_or(|value| value < 1)
                    {
                        issues.push(json!({"path":path,"record":record_id,"field":format!("viewport.{field}"),"reason":"must be a positive integer"}));
                    }
                }
                if !nonempty_text(viewport.get("label")) {
                    issues.push(json!({"path":path,"record":record_id,"field":"viewport.label","reason":"must be a non-empty string"}));
                }
            }
        }
        if kind.is_some_and(|kind| IMAGE_KINDS.contains(&kind)) {
            if let Some((width, height)) =
                image_dimensions(&artifact_data, actual_mime.unwrap_or(""))
            {
                if record.get("width") != Some(&json!(width))
                    || record.get("height") != Some(&json!(height))
                {
                    issues.push(json!({
                        "path":path,"record":record_id,"field":"dimensions",
                        "expected":{"width":width,"height":height},
                        "actual":{"width":record_value(record,"width"),"height":record_value(record,"height")},
                    }));
                }
            } else {
                issues.push(json!({"path":path,"record":record_id,"field":"dimensions","reason":"image dimensions could not be parsed"}));
            }
            if !matches!(
                actual_mime,
                Some("image/png" | "image/jpeg" | "image/gif" | "image/webp")
            ) {
                issues.push(json!({"path":path,"record":record_id,"field":"mime","reason":"screenshot evidence must be a supported raster image"}));
            }
        }
        match kind {
            Some("formal-web-verifier") if actual_mime != Some("application/json") => {
                issues.push(json!({"path":path,"record":record_id,"field":"mime","reason":"formal verifier evidence must be JSON"}));
            }
            Some("formal-web-verifier") => {
                let (loaded, mut nested) = validate_formal_report(&artifact_data, record_id);
                issues.append(&mut nested);
                if let Some(loaded) = loaded {
                    formal_payloads.insert(record_id.to_owned(), loaded);
                }
            }
            Some("review-queue") if actual_mime != Some("application/json") => {
                issues.push(json!({"path":path,"record":record_id,"field":"mime","reason":"review queue evidence must be JSON"}));
            }
            Some("review-queue") => {
                let (loaded, mut nested) = validate_review_queue(&artifact_data, record_id);
                issues.append(&mut nested);
                if let Some(loaded) = loaded {
                    queue_payloads.insert(record_id.to_owned(), loaded);
                }
            }
            Some("manual-review") if actual_mime != Some("application/json") => {
                issues.push(json!({"path":path,"record":record_id,"field":"mime","reason":"manual review evidence must be JSON"}));
            }
            Some("manual-review") => {
                let (loaded, mut nested) = validate_manual_review(&artifact_data, record_id);
                issues.append(&mut nested);
                if let Some(loaded) = loaded {
                    review_payloads.insert(record_id.to_owned(), loaded);
                }
            }
            Some("journey-evidence") if actual_mime != Some("application/json") => {
                issues.push(json!({"path":path,"record":record_id,"field":"mime","reason":"journey evidence must be JSON"}));
            }
            Some("journey-evidence") => {
                let (loaded, mut nested) = validate_journey_evidence(&artifact_data, record_id);
                issues.append(&mut nested);
                if let Some(loaded) = loaded {
                    journey_payloads.insert(record_id.to_owned(), loaded);
                }
            }
            _ => {}
        }
    }

    let screenshot_shas = records
        .iter()
        .filter(|(_, record)| {
            record
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| IMAGE_KINDS.contains(&kind))
        })
        .filter_map(|(record_id, _)| actual_shas.get(record_id).cloned())
        .collect::<BTreeSet<_>>();

    for (journey_id, journey) in &journey_payloads {
        let matching_report = formal_payloads
            .values()
            .any(|report| report.get("runId") == journey.get("runId"));
        if !matching_report {
            issues.push(json!({"path":path,"record":journey_id,"reason":"journey evidence has no registered formal report for the same run"}));
        }
        for (index, cell) in journey
            .get("cells")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let screenshots = cell.get("screenshots").and_then(Value::as_object);
            for name in ["viewport", "fullPage"] {
                if let Some(screenshot) = screenshots
                    .and_then(|screenshots| screenshots.get(name))
                    .and_then(Value::as_object)
                    && screenshot
                        .get("sha256")
                        .and_then(Value::as_str)
                        .is_none_or(|sha| !screenshot_shas.contains(sha))
                {
                    issues.push(json!({"path":path,"record":journey_id,"field":format!("cells[{index}].screenshots.{name}.sha256"),"reason":"journey screenshot hash is not registered as screenshot evidence"}));
                }
            }
        }
    }

    for (review_id, review) in &review_payloads {
        let report_matches = formal_payloads
            .iter()
            .filter(|(record_id, payload)| {
                actual_shas.get(*record_id).map(String::as_str)
                    == review.get("reportSha256").and_then(Value::as_str)
                    && payload.get("runId") == review.get("reviewedRunId")
            })
            .map(|(record_id, _)| record_id)
            .collect::<Vec<_>>();
        let queue_matches = queue_payloads
            .iter()
            .filter(|(record_id, payload)| {
                actual_shas.get(*record_id).map(String::as_str)
                    == review.get("reviewQueueSha256").and_then(Value::as_str)
                    && payload.get("runId") == review.get("reviewedRunId")
            })
            .map(|(record_id, _)| record_id)
            .collect::<Vec<_>>();
        if report_matches.is_empty() {
            issues.push(json!({"path":path,"record":review_id,"reason":"manual review does not bind a registered formal report for the same run"}));
        }
        if queue_matches.is_empty() {
            issues.push(json!({"path":path,"record":review_id,"reason":"manual review does not bind a registered review queue for the same run"}));
        }
        for (index, decision) in review
            .get("decisions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let screenshots = decision.get("screenshots").and_then(Value::as_object);
            for field in ["viewportSha256", "fullPageSha256"] {
                if screenshots
                    .and_then(|screenshots| screenshots.get(field))
                    .and_then(Value::as_str)
                    .is_none_or(|sha| !screenshot_shas.contains(sha))
                {
                    issues.push(json!({"path":path,"record":review_id,"field":format!("decisions[{index}].screenshots.{field}"),"reason":"manual review screenshot hash is not registered as screenshot evidence"}));
                }
            }
        }
        for report_id in report_matches {
            if let Some(report_review) = formal_payloads[report_id]
                .get("review")
                .and_then(Value::as_object)
                && report_review.get("queueSha256") != review.get("reviewQueueSha256")
            {
                issues.push(json!({"path":path,"record":review_id,"reason":"formal report and manual review bind different review queues"}));
            }
        }
    }
    if required && artifacts.is_empty() {
        issues.push(json!({"path":path,"field":"artifacts","reason":"at least one visual evidence artifact is required"}));
    }
    (records, issues)
}

pub fn validate_references(
    text: &str,
    records: &EvidenceRecords,
    required_kinds: Option<&BTreeSet<String>>,
) -> EvidenceIssues {
    let references = evidence_references(text);
    let unknown = references
        .iter()
        .filter(|reference| !records.contains_key(*reference))
        .cloned()
        .collect::<Vec<_>>();
    let mut issues = Vec::new();
    if !unknown.is_empty() {
        issues.push(
            json!({"reason":"report references unknown visual evidence ids","unknown":unknown}),
        );
    }
    if let Some(required_kinds) = required_kinds {
        let referenced_kinds = references
            .iter()
            .filter_map(|reference| records.get(reference))
            .filter_map(|record| record.get("kind"))
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let missing = required_kinds
            .difference(&referenced_kinds)
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            issues.push(json!({"reason":"report does not bind required visual evidence kinds","missing_kinds":missing}));
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    ];

    fn artifact(id: &str, kind: &str, path: &str, bytes: &[u8], image: bool) -> Value {
        let mut value = json!({
            "id":id,"kind":kind,"path":path,"sha256":sha256_hex(bytes),
            "mime":detected_mime(bytes),"captured_by":"Rust fixture",
        });
        if !matches!(kind, "review-queue" | "manual-review" | "journey-evidence") {
            value.as_object_mut().unwrap().extend([
                ("route".to_owned(), json!("Fixture UI")),
                ("state".to_owned(), json!("default state")),
                (
                    "viewport".to_owned(),
                    json!({"width":390,"height":844,"label":"mobile"}),
                ),
            ]);
        }
        if image {
            value.as_object_mut().unwrap().extend([
                ("width".to_owned(), json!(1)),
                ("height".to_owned(), json!(1)),
            ]);
        }
        value
    }

    #[test]
    fn mime_and_dimensions_use_file_signatures_not_extensions() {
        assert_eq!(detected_mime(PNG_1X1), Some("image/png"));
        assert_eq!(image_dimensions(PNG_1X1, "image/png"), Some((1, 1)));
        assert_eq!(detected_mime(br#"{"ok":true}"#), Some("application/json"));
        assert_eq!(detected_mime(b"not-json"), None);
    }

    #[test]
    fn complete_formal_review_chain_is_hash_bound() {
        let directory = tempfile::tempdir().unwrap();
        let artifacts = directory.path().join("artifacts");
        std::fs::create_dir(&artifacts).unwrap();
        for name in ["viewport.png", "full.png"] {
            std::fs::write(artifacts.join(name), PNG_1X1).unwrap();
        }
        let screenshot_sha = sha256_hex(PNG_1X1);
        let fingerprint = "a".repeat(64);
        let queue = json!({
            "schemaVersion":1,"kind":"formal-web-ui-review-queue","runId":"formal-1",
            "entries":[{"reviewCellKey":"cell-1","sourceFingerprint":fingerprint,
                "intentFingerprint":"b".repeat(64),"screenshots":{
                    "viewport":{"sha256":screenshot_sha},"fullPage":{"sha256":screenshot_sha}}}]
        });
        let queue_bytes = serde_json::to_vec(&queue).unwrap();
        std::fs::write(artifacts.join("queue.json"), &queue_bytes).unwrap();
        let formal = json!({
            "schemaVersion":2,"runId":"formal-1","pages":[{"outcome":"checked",
                "metrics":{"visibleScrollbars":[]},"screenshots":{
                    "viewport":{"path":"viewport.png","mime":"image/png","sha256":screenshot_sha,"width":1,"height":1},
                    "fullPage":{"path":"full.png","mime":"image/png","sha256":screenshot_sha,"width":1,"height":1}},
                "review":{"reviewCellKey":"cell-1","sourceFingerprint":fingerprint,"intentFingerprint":"b".repeat(64)}}],
            "findings":[],"coverage":{"checkedPages":1,"failed":false},
            "review":{"queueSha256":sha256_hex(&queue_bytes),"cells":[]}
        });
        let formal_bytes = serde_json::to_vec(&formal).unwrap();
        std::fs::write(artifacts.join("formal.json"), &formal_bytes).unwrap();
        let review = json!({
            "schemaVersion":1,"kind":"formal-web-ui-manual-review","reviewedRunId":"formal-1",
            "reportSha256":sha256_hex(&formal_bytes),"reviewQueueSha256":sha256_hex(&queue_bytes),
            "decisions":[{"reviewCellKey":"cell-1","decision":"pass","note":"",
                "sourceFingerprint":fingerprint,"intentFingerprint":"b".repeat(64),
                "screenshots":{"viewportSha256":screenshot_sha,"fullPageSha256":screenshot_sha}}]
        });
        let review_bytes = serde_json::to_vec(&review).unwrap();
        std::fs::write(artifacts.join("review.json"), &review_bytes).unwrap();
        let manifest = json!({"schema_version":1,"run_id":"run-1","artifacts":[
            artifact("viewport","screenshot","artifacts/viewport.png",PNG_1X1,true),
            artifact("full","screenshot","artifacts/full.png",PNG_1X1,true),
            artifact("formal","formal-web-verifier","artifacts/formal.json",&formal_bytes,false),
            artifact("queue","review-queue","artifacts/queue.json",&queue_bytes,false),
            artifact("review","manual-review","artifacts/review.json",&review_bytes,false)
        ]});
        std::fs::write(
            directory.path().join(VISUAL_EVIDENCE_FILENAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let (records, issues) = validate_visual_evidence_manifest(directory.path(), "run-1", true);
        assert!(issues.is_empty(), "{issues:#?}");
        assert_eq!(records.len(), 5);
        let required = ["screenshot", "formal-web-verifier", "manual-review"]
            .map(str::to_owned)
            .into_iter()
            .collect();
        assert!(
            validate_references(
                "evidence:viewport evidence:formal evidence:review",
                &records,
                Some(&required),
            )
            .is_empty()
        );
    }

    #[test]
    fn evidence_rejects_unknown_refs_tampering_and_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(VISUAL_EVIDENCE_FILENAME),
            br#"{"schema_version":1,"run_id":"run","artifacts":[]}"#,
        )
        .unwrap();
        let (records, issues) = validate_visual_evidence_manifest(directory.path(), "run", true);
        assert!(records.is_empty());
        assert!(
            issues
                .iter()
                .any(|issue| issue.get("field") == Some(&json!("artifacts")))
        );
        let refs = validate_references("evidence:missing", &records, None);
        assert_eq!(refs[0]["unknown"], json!(["missing"]));

        let real = directory.path().join("real.json");
        std::fs::rename(directory.path().join(VISUAL_EVIDENCE_FILENAME), &real).unwrap();
        std::os::unix::fs::symlink(&real, directory.path().join(VISUAL_EVIDENCE_FILENAME)).unwrap();
        let (_, issues) = validate_visual_evidence_manifest(directory.path(), "run", true);
        assert!(issues.iter().any(
            |issue| issue.get("reason") == Some(&json!("visual evidence manifest is missing"))
        ));
    }
}
