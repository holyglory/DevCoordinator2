//! Marker-free hidden-oracle evaluation for the full-repository audit skill.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::audit_ledger::{read_bytes_nofollow, validate_directory_nofollow};

pub const SCHEMA_VERSION: u64 = 1;
pub const CASE_CLASSES: &[&str] = &[
    "ignored-input-config",
    "partial-plumbing-no-dependency-call",
    "false-persistence-success",
    "missing-registration-lifecycle",
    "production-fixture-mock",
    "shallow-outcome-tests",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub schema_version: u64,
    pub cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub class: String,
    pub repo: String,
    pub oracle: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedAnchor {
    pub path: String,
    pub symbols: Vec<String>,
    pub why: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Oracle {
    pub schema_version: u64,
    pub case_id: String,
    pub class: String,
    pub expected_findings: Vec<ExpectedAnchor>,
    pub precision_controls: Vec<ExpectedAnchor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub class: String,
    pub path: String,
    pub symbol: String,
    pub summary: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrecisionControl {
    pub path: String,
    pub symbol: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub schema_version: u64,
    pub case_id: String,
    pub findings: Vec<Finding>,
    pub precision_controls: Vec<PrecisionControl>,
}

pub fn default_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tooling package is nested under the repository root")
        .join("skills/full-repo-audit/evals/marker-free")
}

fn strict_object(path: &Path, anchor: &Path, label: &str) -> Result<Map<String, Value>, String> {
    let bytes = read_bytes_nofollow(path, Some(anchor))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("missing JSON file: {}", path.display()))?;
    crate::audit_findings::strict_json_object(&bytes, label)
        .map_err(|error| format!("invalid JSON file {}: {error}", path.display()))
}

fn typed<T: for<'de> Deserialize<'de>>(
    object: Map<String, Value>,
    label: &str,
) -> Result<T, String> {
    serde_json::from_value(Value::Object(object))
        .map_err(|error| format!("{label} has invalid fields: {error}"))
}

fn require_text(value: &str, label: &str, minimum: usize) -> Result<(), String> {
    if value.trim().len() < minimum {
        Err(format!(
            "{label} must be a string with at least {minimum} non-whitespace characters"
        ))
    } else {
        Ok(())
    }
}

fn relative_path(value: &str, label: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.contains('\\')
        || value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
        || path.to_string_lossy() != value
    {
        return Err(format!(
            "{label} must be a normalized repository-relative path"
        ));
    }
    Ok(())
}

fn repo_anchor(repo: &Path, relative: &str, symbol: &str, label: &str) -> Result<(), String> {
    relative_path(relative, label)?;
    let path = repo.join(relative);
    let bytes = read_bytes_nofollow(&path, Some(repo))
        .map_err(|error| format!("{label} references unsafe source path {relative:?}: {error}"))?
        .ok_or_else(|| format!("{label} references missing source path {relative:?}"))?;
    let source = String::from_utf8(bytes)
        .map_err(|_| format!("{label} source path is not readable UTF-8: {relative:?}"))?;
    let leaf = symbol.rsplit('.').next().unwrap_or(symbol);
    if !source.contains(symbol) && !source.contains(leaf) {
        return Err(format!(
            "{label} symbol {symbol:?} is absent from {relative:?}"
        ));
    }
    Ok(())
}

fn validate_schema(root: &Path) -> Result<(), String> {
    let schema = strict_object(
        &root.join("response.schema.json"),
        root,
        "response.schema.json",
    )?;
    if schema.get("type") != Some(&json!("object"))
        || schema.get("additionalProperties") != Some(&json!(false))
    {
        return Err("response.schema.json must define a closed object schema".to_owned());
    }
    if schema.get("required")
        != Some(&json!([
            "schema_version",
            "case_id",
            "findings",
            "precision_controls"
        ]))
    {
        return Err(
            "response.schema.json root required fields drifted from the scorer contract".to_owned(),
        );
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| "response.schema.json properties must be an object".to_owned())?;
    if properties
        .get("schema_version")
        .and_then(Value::as_object)
        .and_then(|value| value.get("const"))
        != Some(&json!(SCHEMA_VERSION))
    {
        return Err(
            "response.schema.json schema_version drifted from the scorer contract".to_owned(),
        );
    }
    let classes = properties
        .get("findings")
        .and_then(Value::as_object)
        .and_then(|value| value.get("items"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("properties"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("class"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("enum"));
    if classes != Some(&json!(CASE_CLASSES)) {
        return Err(
            "response.schema.json finding classes drifted from the scorer contract".to_owned(),
        );
    }
    Ok(())
}

fn validate_anchor_entry(entry: &ExpectedAnchor, repo: &Path, label: &str) -> Result<(), String> {
    relative_path(&entry.path, &format!("{label}.path"))?;
    if entry.symbols.is_empty() {
        return Err(format!("{label}.symbols must be a non-empty array"));
    }
    let mut symbols = BTreeSet::new();
    for symbol in &entry.symbols {
        require_text(symbol, &format!("{label}.symbols"), 1)?;
        if !symbols.insert(symbol) {
            return Err(format!("{label}.symbols contains duplicates"));
        }
    }
    require_text(&entry.why, &format!("{label}.why"), 12)?;
    let mut errors = Vec::new();
    for symbol in &entry.symbols {
        if let Err(error) = repo_anchor(repo, &entry.path, symbol, label) {
            errors.push(error);
        }
    }
    if errors.len() == entry.symbols.len() {
        return Err(errors.remove(0));
    }
    Ok(())
}

fn validate_oracle(oracle: Oracle, case: &Case, repo: &Path) -> Result<Oracle, String> {
    let label = format!("oracle for {}", case.id);
    if oracle.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{label} schema_version must equal {SCHEMA_VERSION}"
        ));
    }
    if oracle.case_id != case.id || oracle.class != case.class {
        return Err(format!("{label} identity does not match suite.json"));
    }
    if oracle.expected_findings.len() != 1 || oracle.precision_controls.len() != 1 {
        return Err(format!(
            "{label} expected_findings and precision_controls must contain exactly one entry"
        ));
    }
    validate_anchor_entry(
        &oracle.expected_findings[0],
        repo,
        &format!("{label} expected_findings[0]"),
    )?;
    validate_anchor_entry(
        &oracle.precision_controls[0],
        repo,
        &format!("{label} precision_controls[0]"),
    )?;
    let gap = &oracle.expected_findings[0];
    let control = &oracle.precision_controls[0];
    if gap.path == control.path
        && gap
            .symbols
            .iter()
            .any(|symbol| control.symbols.contains(symbol))
    {
        return Err(format!(
            "{label} gap and precision control anchors must be disjoint"
        ));
    }
    Ok(oracle)
}

fn recursive_files(repo: &Path, case_id: &str) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| format!("{case_id} repo cannot be read: {error}"))?
        {
            let entry = entry.map_err(|error| format!("{case_id} repo cannot be read: {error}"))?;
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|error| format!("{case_id} repo cannot be read: {error}"))?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "{case_id} agent input must not contain symlinks: {}",
                    entry.path().strip_prefix(repo).unwrap().display()
                ));
            }
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn validate_agent_input(repo: &Path, case_id: &str) -> Result<(), String> {
    let repo = validate_directory_nofollow(repo)
        .map_err(|_| format!("{case_id} repo must be a real directory"))?;
    let readme = read_bytes_nofollow(&repo.join("README.md"), Some(&repo))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{case_id} agent input must contain a readable README.md"))?;
    let readme = String::from_utf8(readme)
        .map_err(|_| format!("{case_id} agent input must contain a readable README.md"))?;
    if !readme.contains("## Requirements") || readme.matches("\n-").count() < 2 {
        return Err(format!(
            "{case_id} README.md must contain explicit repo-owned requirements"
        ));
    }
    let files = recursive_files(&repo, case_id)?;
    if files.len() < 2 {
        return Err(format!(
            "{case_id} repo must contain requirements and source"
        ));
    }
    let markers =
        Regex::new(r"(?i)\b(?:TODO|NotImplemented(?:Error)?|stub)\b").expect("answer marker regex");
    for path in files {
        if path.file_name().and_then(|name| name.to_str()) == Some("oracle.json") {
            return Err(format!(
                "{case_id} oracle must not be inside the agent input"
            ));
        }
        let bytes = read_bytes_nofollow(&path, Some(&repo))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{case_id} agent input file disappeared"))?;
        let text = String::from_utf8(bytes).map_err(|_| {
            format!(
                "{case_id} agent input must be UTF-8 text: {}",
                path.strip_prefix(&repo).unwrap().display()
            )
        })?;
        if let Some(found) = markers.find(&text) {
            return Err(format!(
                "{case_id} agent input contains prohibited answer marker {:?} in {}",
                found.as_str(),
                path.strip_prefix(&repo).unwrap().display()
            ));
        }
    }
    Ok(())
}

pub fn load_and_validate_suite(root: &Path) -> Result<(Suite, BTreeMap<String, Oracle>), String> {
    let root = validate_directory_nofollow(root).map_err(|error| error.to_string())?;
    validate_schema(&root)?;
    let suite: Suite = typed(
        strict_object(&root.join("suite.json"), &root, "suite.json")?,
        "suite.json",
    )?;
    if suite.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "suite.json schema_version must equal {SCHEMA_VERSION}"
        ));
    }
    if suite.cases.len() != CASE_CLASSES.len() {
        return Err(format!(
            "suite.json must contain exactly {} cases",
            CASE_CLASSES.len()
        ));
    }
    let case_id = Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("case id regex");
    let mut ids = Vec::new();
    let mut classes = Vec::new();
    let mut oracles = BTreeMap::new();
    for (index, case) in suite.cases.iter().enumerate() {
        require_text(&case.id, &format!("suite.json cases[{index}].id"), 1)?;
        if !case_id.is_match(&case.id) {
            return Err(format!("invalid case id: {:?}", case.id));
        }
        require_text(&case.class, &format!("suite.json cases[{index}].class"), 1)?;
        relative_path(&case.repo, &format!("suite.json cases[{index}].repo"))?;
        relative_path(&case.oracle, &format!("suite.json cases[{index}].oracle"))?;
        let repo = root.join(&case.repo);
        let oracle_path = root.join(&case.oracle);
        validate_agent_input(&repo, &case.id)?;
        let oracle: Oracle = typed(
            strict_object(&oracle_path, &root, &format!("oracle for {}", case.id))?,
            &format!("oracle for {}", case.id),
        )?;
        oracles.insert(case.id.clone(), validate_oracle(oracle, case, &repo)?);
        ids.push(case.id.as_str());
        classes.push(case.class.as_str());
    }
    if ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err("suite.json case ids must be unique".to_owned());
    }
    if ids != CASE_CLASSES || classes != CASE_CLASSES {
        return Err(
            "suite.json must contain the six canonical classes in canonical order".to_owned(),
        );
    }
    let case_dirs = std::fs::read_dir(root.join("cases"))
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let expected = ids.into_iter().map(str::to_owned).collect::<BTreeSet<_>>();
    if case_dirs != expected {
        return Err("case directories and suite.json case ids do not match exactly".to_owned());
    }
    Ok((suite, oracles))
}

fn validate_response_value(
    object: Map<String, Value>,
    case: &Case,
    repo: &Path,
) -> Result<Response, String> {
    let label = format!("response for {}", case.id);
    let response: Response = typed(object, &label)?;
    if response.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{label} schema_version must equal {SCHEMA_VERSION}"
        ));
    }
    if response.case_id != case.id {
        return Err(format!("{label} case_id does not match the selected case"));
    }
    let mut finding_keys = BTreeSet::new();
    let mut finding_anchors = BTreeSet::new();
    for (index, finding) in response.findings.iter().enumerate() {
        let item = format!("{label} findings[{index}]");
        require_text(&finding.class, &format!("{item}.class"), 1)?;
        if !CASE_CLASSES.contains(&finding.class.as_str()) {
            return Err(format!("{item}.class is not a supported marker-free class"));
        }
        relative_path(&finding.path, &format!("{item}.path"))?;
        require_text(&finding.symbol, &format!("{item}.symbol"), 1)?;
        require_text(&finding.summary, &format!("{item}.summary"), 12)?;
        require_text(&finding.evidence, &format!("{item}.evidence"), 12)?;
        repo_anchor(repo, &finding.path, &finding.symbol, &item)?;
        let key = (
            finding.class.clone(),
            finding.path.clone(),
            finding.symbol.clone(),
        );
        if !finding_keys.insert(key.clone()) {
            return Err(format!(
                "{label} contains a duplicate finding anchor: {key:?}"
            ));
        }
        finding_anchors.insert((finding.path.clone(), finding.symbol.clone()));
    }
    let mut control_keys = BTreeSet::new();
    for (index, control) in response.precision_controls.iter().enumerate() {
        let item = format!("{label} precision_controls[{index}]");
        relative_path(&control.path, &format!("{item}.path"))?;
        require_text(&control.symbol, &format!("{item}.symbol"), 1)?;
        require_text(&control.reason, &format!("{item}.reason"), 12)?;
        repo_anchor(repo, &control.path, &control.symbol, &item)?;
        let key = (control.path.clone(), control.symbol.clone());
        if !control_keys.insert(key.clone()) {
            return Err(format!(
                "{label} contains a duplicate precision-control anchor: {key:?}"
            ));
        }
        if finding_anchors.contains(&key) {
            return Err(format!(
                "{label} cannot classify the same anchor as a finding and precision control: {key:?}"
            ));
        }
    }
    Ok(response)
}

pub fn validate_response_file(
    root: &Path,
    case: &Case,
    response_path: &Path,
) -> Result<Response, String> {
    let repo =
        validate_directory_nofollow(&root.join(&case.repo)).map_err(|error| error.to_string())?;
    let response_root = response_path.parent().unwrap_or(root);
    validate_response_value(
        strict_object(response_path, response_root, "response")?,
        case,
        &repo,
    )
}

fn finding_matches(item: &Finding, expected: &ExpectedAnchor, class: Option<&str>) -> bool {
    class.is_none_or(|class| item.class == class)
        && item.path == expected.path
        && expected.symbols.contains(&item.symbol)
}

fn control_matches(item: &PrecisionControl, expected: &ExpectedAnchor) -> bool {
    item.path == expected.path && expected.symbols.contains(&item.symbol)
}

fn public_anchor(entry: &ExpectedAnchor) -> Value {
    json!({"path":entry.path,"symbols":entry.symbols})
}

pub fn score_response(response: &Response, case: &Case, oracle: &Oracle) -> Value {
    let mut consumed_findings = BTreeSet::new();
    let mut matched_findings = Vec::new();
    for expected in &oracle.expected_findings {
        if let Some(index) = response
            .findings
            .iter()
            .enumerate()
            .find(|(index, item)| {
                !consumed_findings.contains(index)
                    && finding_matches(item, expected, Some(&case.class))
            })
            .map(|(index, _)| index)
        {
            consumed_findings.insert(index);
            matched_findings.push(expected);
        }
    }
    let unmatched_findings = response
        .findings
        .iter()
        .enumerate()
        .filter(|(index, _)| !consumed_findings.contains(index))
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    let mut consumed_controls = BTreeSet::new();
    let mut matched_controls = Vec::new();
    for expected in &oracle.precision_controls {
        if let Some(index) = response
            .precision_controls
            .iter()
            .enumerate()
            .find(|(index, item)| {
                !consumed_controls.contains(index) && control_matches(item, expected)
            })
            .map(|(index, _)| index)
        {
            consumed_controls.insert(index);
            matched_controls.push(expected);
        }
    }
    let unexpected_controls = response
        .precision_controls
        .iter()
        .enumerate()
        .filter(|(index, _)| !consumed_controls.contains(index))
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    let controls_as_findings = response
        .findings
        .iter()
        .filter(|item| {
            oracle
                .precision_controls
                .iter()
                .any(|expected| finding_matches(item, expected, None))
        })
        .collect::<Vec<_>>();
    let recall = (60.0 * matched_findings.len() as f64 / oracle.expected_findings.len() as f64)
        .round() as u64;
    let control = (20.0 * matched_controls.len() as f64 / oracle.precision_controls.len() as f64)
        .round() as u64;
    let precision = if unmatched_findings.is_empty() { 20 } else { 0 };
    let score = recall + control + precision;
    json!({
        "case_id":case.id,"class":case.class,"score":score,"passed":score==100,
        "points":{"finding_recall":recall,"precision_control":control,"unmatched-finding_precision":precision},
        "matched_findings":matched_findings.iter().map(|item|public_anchor(item)).collect::<Vec<_>>(),
        "missed_findings":oracle.expected_findings.iter().filter(|item|!matched_findings.contains(item)).map(public_anchor).collect::<Vec<_>>(),
        "matched_precision_controls":matched_controls.iter().map(|item|public_anchor(item)).collect::<Vec<_>>(),
        "missed_precision_controls":oracle.precision_controls.iter().filter(|item|!matched_controls.contains(item)).map(public_anchor).collect::<Vec<_>>(),
        "unmatched_findings":unmatched_findings,"unexpected_precision_controls":unexpected_controls,
        "precision_controls_misclassified_as_findings":controls_as_findings,
    })
}

pub fn validate_suite_result(root: &Path) -> Result<Value, String> {
    let (suite, _) = load_and_validate_suite(root)?;
    Ok(json!({
        "valid":true,"schema_version":SCHEMA_VERSION,"case_count":suite.cases.len(),
        "cases":suite.cases.iter().map(|case|case.id.clone()).collect::<Vec<_>>(),
    }))
}

pub fn validate_one_response(root: &Path, case_id: &str, response: &Path) -> Result<Value, String> {
    let (suite, _) = load_and_validate_suite(root)?;
    let case = suite
        .cases
        .iter()
        .find(|case| case.id == case_id)
        .ok_or_else(|| format!("unknown marker-free case: {case_id}"))?;
    validate_response_file(root, case, response)?;
    Ok(json!({"valid":true,"case_id":case_id}))
}

pub fn score_response_directory(root: &Path, responses: &Path) -> Result<Value, String> {
    let (suite, oracles) = load_and_validate_suite(root)?;
    let responses = validate_directory_nofollow(responses)
        .map_err(|_| format!("responses path is not a directory: {}", responses.display()))?;
    let expected = suite
        .cases
        .iter()
        .map(|case| format!("{}.json", case.id))
        .collect::<BTreeSet<_>>();
    let actual = std::fs::read_dir(&responses)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
                && entry.path().extension().and_then(|value| value.to_str()) == Some("json")
        })
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let extra = actual.difference(&expected).cloned().collect::<Vec<_>>();
    if !missing.is_empty() || !extra.is_empty() {
        return Err(format!(
            "response set mismatch: missing={missing:?}, extra={extra:?}"
        ));
    }
    let mut results = Vec::new();
    for case in &suite.cases {
        let response =
            validate_response_file(root, case, &responses.join(format!("{}.json", case.id)))?;
        results.push(score_response(&response, case, &oracles[&case.id]));
    }
    let total = results
        .iter()
        .filter_map(|result| result.get("score").and_then(Value::as_u64))
        .sum::<u64>();
    let score = ((total as f64 / results.len() as f64) * 100.0).round() / 100.0;
    Ok(json!({
        "schema_version":SCHEMA_VERSION,"case_count":results.len(),"score":score,
        "passed":results.iter().all(|result|result["passed"]==true),"cases":results,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perfect(case: &Case, oracle: &Oracle) -> Response {
        Response {
            schema_version: SCHEMA_VERSION,
            case_id: case.id.clone(),
            findings: vec![Finding {
                class: case.class.clone(),
                path: oracle.expected_findings[0].path.clone(),
                symbol: oracle.expected_findings[0].symbols[0].clone(),
                summary: "The required observable behavior is incomplete.".to_owned(),
                evidence:
                    "The named source path does not carry the required operation to its outcome."
                        .to_owned(),
            }],
            precision_controls: vec![PrecisionControl {
                path: oracle.precision_controls[0].path.clone(),
                symbol: oracle.precision_controls[0].symbols[0].clone(),
                reason:
                    "The repository requirements explicitly define this behavior as intentional."
                        .to_owned(),
            }],
        }
    }

    fn response_object(response: &Response) -> Map<String, Value> {
        serde_json::to_value(response)
            .unwrap()
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn canonical_suite_and_perfect_six_case_response_set_score_one_hundred() {
        let root = default_root();
        let (suite, oracles) = load_and_validate_suite(&root).unwrap();
        assert_eq!(suite.cases.len(), 6);
        let directory = tempfile::tempdir().unwrap();
        for case in &suite.cases {
            let response = perfect(case, &oracles[&case.id]);
            let validated =
                validate_response_value(response_object(&response), case, &root.join(&case.repo))
                    .unwrap();
            let result = score_response(&validated, case, &oracles[&case.id]);
            assert_eq!(result["score"], 100);
            crate::audit_queue::write_json(
                &directory.path().join(format!("{}.json", case.id)),
                &serde_json::to_value(response).unwrap(),
            )
            .unwrap();
        }
        let aggregate = score_response_directory(&root, directory.path()).unwrap();
        assert_eq!(aggregate["score"], 100.0);
        assert_eq!(aggregate["passed"], true);
        std::fs::remove_file(directory.path().join("ignored-input-config.json")).unwrap();
        assert!(
            score_response_directory(&root, directory.path())
                .unwrap_err()
                .contains("response set mismatch")
        );
    }

    #[test]
    fn scoring_preserves_recall_control_and_false_positive_weights() {
        let root = default_root();
        let (suite, oracles) = load_and_validate_suite(&root).unwrap();
        let case = &suite.cases[0];
        let oracle = &oracles[&case.id];
        let mut response = perfect(case, oracle);
        response.findings.clear();
        assert_eq!(score_response(&response, case, oracle)["score"], 40);

        let mut response = perfect(case, oracle);
        response.precision_controls.clear();
        assert_eq!(score_response(&response, case, oracle)["score"], 80);

        let mut response = perfect(case, oracle);
        response.precision_controls.clear();
        response.findings.push(Finding {
            class: case.class.clone(),
            path: oracle.precision_controls[0].path.clone(),
            symbol: oracle.precision_controls[0].symbols[0].clone(),
            summary: "This intentional behavior was incorrectly classified as incomplete."
                .to_owned(),
            evidence:
                "The source anchor is real but the claim contradicts repository requirements."
                    .to_owned(),
        });
        let result = score_response(&response, case, oracle);
        assert_eq!(result["score"], 60);
        assert_eq!(
            result["precision_controls_misclassified_as_findings"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn response_validation_rejects_extra_fields_traversal_and_duplicates() {
        let root = default_root();
        let (suite, oracles) = load_and_validate_suite(&root).unwrap();
        let case = &suite.cases[0];
        let repo = root.join(&case.repo);
        let response = perfect(case, &oracles[&case.id]);

        let mut extra = response_object(&response);
        extra.insert("answer".to_owned(), json!("leaked"));
        assert!(validate_response_value(extra, case, &repo).is_err());

        let mut traversal = response.clone();
        traversal.findings[0].path = "../candidates.py".to_owned();
        assert!(
            validate_response_value(response_object(&traversal), case, &repo)
                .unwrap_err()
                .contains("normalized repository-relative")
        );

        let mut duplicate = response;
        duplicate.findings.push(duplicate.findings[0].clone());
        assert!(
            validate_response_value(response_object(&duplicate), case, &repo)
                .unwrap_err()
                .contains("duplicate finding anchor")
        );
    }
}
