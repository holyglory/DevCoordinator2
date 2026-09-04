//! Rust-native fixture driver for the retained Node formal Web UI verifier.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::audit_common::sha256_file;
use crate::audit_ledger::{
    create_directory_all_nofollow, read_bytes_nofollow, validate_directory_nofollow,
};

const RECEIPT_LIMIT: usize = 2048;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Matrix {
    schema_version: u64,
    cases: Vec<MatrixCase>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatrixCase {
    name: String,
    path: String,
    critical: bool,
    #[serde(default)]
    critical_rules: Vec<String>,
    #[serde(default)]
    warning_rules: Vec<String>,
    #[serde(default)]
    forbidden_rules: Vec<String>,
    #[serde(default)]
    minimum_rule_counts: BTreeMap<String, usize>,
    #[serde(default)]
    forbidden_text: Vec<String>,
    #[serde(default)]
    scrollbars: Vec<ScrollbarExpectation>,
    #[serde(default)]
    forbidden_scrollbar_suffixes: Vec<String>,
    #[serde(default)]
    metric_contains: Vec<String>,
    #[serde(default)]
    target: Map<String, Value>,
    final_path: Option<String>,
    continuation_focus: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScrollbarExpectation {
    selector_suffix: String,
    axis: String,
    depth: u64,
}

struct StaticServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl StaticServer {
    fn start(pages: BTreeMap<String, Vec<u8>>) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("cannot bind self-test server: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("cannot configure self-test server: {error}"))?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        let pages = Arc::new(pages);
        let thread = thread::spawn(move || {
            while !stop_worker.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let pages = Arc::clone(&pages);
                        thread::spawn(move || serve_connection(stream, &pages));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            stop,
            thread: Some(thread),
        })
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve_connection(mut stream: TcpStream, pages: &BTreeMap<String, Vec<u8>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = [0u8; 16 * 1024];
    let Ok(count) = stream.read(&mut request) else {
        return;
    };
    let first = String::from_utf8_lossy(&request[..count]);
    let path = first
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_start_matches('/');
    let (status, content_type, body) = match pages.get(path) {
        Some(body) => (
            "200 OK",
            if path.ends_with(".json") {
                "application/json; charset=utf-8"
            } else if path.ends_with(".txt") {
                "text/plain; charset=utf-8"
            } else {
                "text/html; charset=utf-8"
            },
            body.as_slice(),
        ),
        None => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found".as_slice(),
        ),
    };
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tooling package is nested under the repository root")
        .to_owned()
}

fn load_json(path: &Path, root: &Path, label: &str) -> Result<Value, String> {
    let bytes = read_bytes_nofollow(path, Some(root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{label} is missing: {}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{label} is invalid JSON: {error}"))
}

fn load_pages(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let fixture_root = root.join("skills/formal-web-ui-verification/fixtures/self-test");
    let value = load_json(
        &fixture_root.join("pages.json"),
        &fixture_root,
        "formal UI pages",
    )?;
    value
        .as_object()
        .ok_or_else(|| "formal UI pages must be a JSON object".to_owned())?
        .iter()
        .map(|(name, body)| {
            body.as_str()
                .map(|body| (name.clone(), body.as_bytes().to_vec()))
                .ok_or_else(|| format!("formal UI page {name} must be text"))
        })
        .collect()
}

fn load_matrix(root: &Path) -> Result<Matrix, String> {
    let fixture_root = root.join("skills/formal-web-ui-verification/fixtures/self-test");
    let value = load_json(
        &fixture_root.join("matrix.json"),
        &fixture_root,
        "formal UI matrix",
    )?;
    let matrix: Matrix = serde_json::from_value(value)
        .map_err(|error| format!("invalid formal UI matrix: {error}"))?;
    if matrix.schema_version != 1 || matrix.cases.is_empty() {
        return Err("formal UI matrix schema or cases are invalid".to_owned());
    }
    let mut names = BTreeSet::new();
    for case in &matrix.cases {
        if case.name.is_empty() || !names.insert(&case.name) {
            return Err("formal UI matrix case names must be unique and non-empty".to_owned());
        }
    }
    Ok(matrix)
}

fn playwright_module_dir(root: &Path) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(explicit) = std::env::var_os("FORMAL_WEB_UI_PLAYWRIGHT_NODE_MODULES") {
        candidates.push(PathBuf::from(explicit));
    }
    candidates.push(root.join("ci/playwright/node_modules"));
    if let Some(node_path) = std::env::var_os("NODE_PATH") {
        candidates.extend(std::env::split_paths(&node_path));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates
            .push(PathBuf::from(home).join(
                ".cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules",
            ));
    }
    let mut checked = Vec::new();
    for candidate in candidates {
        if checked.contains(&candidate) {
            continue;
        }
        checked.push(candidate.clone());
        if candidate.join("playwright/package.json").is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "Playwright is unavailable for the formal UI self-test; checked {checked:?}"
    ))
}

fn unique_work_dir(parent: Option<&Path>) -> Result<PathBuf, String> {
    let mut random = [0u8; 12];
    getrandom::fill(&mut random).map_err(|error| error.to_string())?;
    let token = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let root = parent
        .map(Path::to_owned)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("formal-web-ui-rust-self-test-{token}"));
    create_directory_all_nofollow(&root, 0o700).map_err(|error| error.to_string())?;
    Ok(root)
}

fn wait_child(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), String> {
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "formal UI verifier exceeded {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_end(&mut stdout)
            .map_err(|error| error.to_string())?;
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr)
            .map_err(|error| error.to_string())?;
    }
    Ok((status, stdout, stderr))
}

fn default_target_contract() -> Map<String, Value> {
    json!({
        "journeys":[{"id":"fixture-primary","name":"Exercise the fixture","frequencyPercent":100,"risk":"normal","rationale":"Rust self-test target"}],
        "primaryJourney":"fixture-primary",
        "regions":[{"selector":"main","role":"primary-content","journey":"fixture-primary","name":"Fixture content"}],
        "theme":"light","reviewInputs":[{"path":"SKILL.md","kind":"ui-code"}],
    })
    .as_object()
    .unwrap()
    .clone()
}

fn build_matrix_config(source_root: &Path, server: &StaticServer, matrix: &Matrix) -> Value {
    let defaults = default_target_contract();
    let targets = matrix
        .cases
        .iter()
        .map(|case| {
            let mut target = case.target.clone();
            target.insert("name".to_owned(), json!(case.name));
            target.insert(
                "url".to_owned(),
                json!(format!("{}/{}", server.base_url(), case.path)),
            );
            for (key, value) in &defaults {
                target.entry(key.clone()).or_insert_with(|| value.clone());
            }
            Value::Object(target)
        })
        .collect::<Vec<_>>();
    json!({
        "repoRoot":source_root.join("skills/formal-web-ui-verification"),
        "performance":{"ttfbMs":10000,"lcpMs":10000,"ttfbLocalOnly":false},
        "targets":targets,
        "viewports":[{"name":"mobile","width":390,"height":844}],
        "maxPageCount":matrix.cases.len(),
    })
}

fn sha_text(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|value| {
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn receipt_paths(receipt: &Value) -> Result<(PathBuf, PathBuf), String> {
    let artifacts = receipt
        .get("artifacts")
        .and_then(Value::as_object)
        .ok_or_else(|| "receipt must name report artifacts".to_owned())?;
    if let Some(directory) = artifacts.get("directory").and_then(Value::as_str) {
        return Ok((
            Path::new(directory).join(
                artifacts
                    .get("json")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            Path::new(directory).join(
                artifacts
                    .get("markdown")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        ));
    }
    Ok((
        PathBuf::from(
            artifacts
                .get("json")
                .and_then(Value::as_str)
                .ok_or_else(|| "receipt JSON artifact path is missing".to_owned())?,
        ),
        PathBuf::from(
            artifacts
                .get("markdown")
                .and_then(Value::as_str)
                .ok_or_else(|| "receipt Markdown artifact path is missing".to_owned())?,
        ),
    ))
}

fn validate_complete_report(report: &Value, markdown: &str, output: &Path) -> Result<(), String> {
    if report.get("schemaVersion") != Some(&json!(2))
        || !report.get("runId").is_some_and(Value::is_string)
        || !report.get("pages").is_some_and(Value::is_array)
        || !report.get("findings").is_some_and(Value::is_array)
        || !report.get("coverage").is_some_and(Value::is_object)
        || !report.get("startedAt").is_some_and(Value::is_string)
        || !report.get("endedAt").is_some_and(Value::is_string)
        || !sha_text(report.pointer("/evidence/verifier/sha256"))
        || !sha_text(report.pointer("/evidence/config/sha256"))
        || report.pointer("/plan/widthCoverage") != Some(&json!("sampled-only"))
        || !report.get("review").is_some_and(Value::is_object)
        || !sha_text(report.pointer("/review/queueSha256"))
    {
        return Err("formal verifier JSON omitted complete report evidence".to_owned());
    }
    for heading in [
        "# Formal Web UI Verification Report",
        "## Target Coverage",
        "## Changed Visual Review",
        "## Findings",
    ] {
        if !markdown.contains(heading) {
            return Err(format!("formal Markdown report lacks {heading}"));
        }
    }
    let pages = report["pages"].as_array().unwrap();
    if report
        .pointer("/coverage/cells")
        .and_then(Value::as_array)
        .is_none_or(|cells| cells.len() != pages.len())
    {
        return Err("formal coverage cells do not match page results".to_owned());
    }
    for page in pages {
        if !page.get("requestedPath").is_some_and(Value::is_string)
            || !page.get("finalPath").is_some_and(Value::is_string)
            || !page.get("sourceBinding").is_some_and(Value::is_object)
            || !page.get("startedAt").is_some_and(Value::is_string)
            || !page.get("endedAt").is_some_and(Value::is_string)
        {
            return Err("formal page omitted path, source binding, or timing evidence".to_owned());
        }
        if page.get("outcome") != Some(&json!("checked")) {
            continue;
        }
        for role in ["viewport", "fullPage"] {
            let screenshot = page
                .pointer(&format!("/screenshots/{role}"))
                .and_then(Value::as_object)
                .ok_or_else(|| format!("checked page lacks {role} screenshot"))?;
            let path = screenshot
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .ok_or_else(|| format!("{role} screenshot path is missing"))?;
            read_bytes_nofollow(&path, Some(output))
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{role} screenshot is missing"))?;
            if screenshot.get("mime") != Some(&json!("image/png"))
                || screenshot.get("sha256") != Some(&json!(sha256_file(&path)?))
            {
                return Err(format!("{role} screenshot evidence is invalid"));
            }
        }
    }
    let progress = report
        .pointer("/execution/progressPath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "progress artifact path is missing".to_owned())?;
    let progress = read_bytes_nofollow(&progress, Some(output))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "progress artifact is missing".to_owned())?;
    let rows = String::from_utf8(progress).map_err(|_| "progress artifact is not UTF-8")?;
    let values = rows
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    if values.first().and_then(|value| value.get("kind")) != Some(&json!("run-start"))
        || values.last().and_then(|value| value.get("kind")) != Some(&json!("run-complete"))
    {
        return Err("progress artifact does not bind the complete run".to_owned());
    }
    Ok(())
}

fn run_verifier(
    source_root: &Path,
    config: &Value,
    output: &Path,
    expected_exit: i32,
    timeout: Duration,
    extra: &[&str],
) -> Result<Value, String> {
    create_directory_all_nofollow(output, 0o700).map_err(|error| error.to_string())?;
    let config_path = output.join("formal-web-ui.json");
    let json_out = output.join("report.json");
    let markdown_out = output.join("report.md");
    crate::audit_queue::write_json(&config_path, config)?;
    let verifier =
        source_root.join("skills/formal-web-ui-verification/scripts/formal_web_ui_verify.mjs");
    let node = std::env::var_os("FORMAL_WEB_UI_NODE").unwrap_or_else(|| "node".into());
    let playwright = playwright_module_dir(source_root)?;
    let mut command = Command::new(node);
    command
        .arg(verifier)
        .arg("--playwright-module-dir")
        .arg(playwright)
        .arg("--config")
        .arg(&config_path)
        .arg("--json-out")
        .arg(&json_out)
        .arg("--markdown-out")
        .arg(&markdown_out)
        .args(["--fail-on", "critical"])
        .args(extra)
        .env_remove("NODE_PATH")
        .env_remove("DEVCOORDINATOR_EVIDENCE_DIR")
        .env_remove("DEVCOORDINATOR_RUN_ID")
        .env_remove("DEVCOORDINATOR_CHECK_NAME")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command
        .spawn()
        .map_err(|error| format!("cannot launch formal verifier: {error}"))?;
    let (status, stdout, stderr) = wait_child(child, timeout)?;
    let exit = status.code().unwrap_or(2);
    if exit != expected_exit {
        return Err(format!(
            "formal verifier exit mismatch: expected {expected_exit}, got {exit}; stderr={} stdout={}",
            String::from_utf8_lossy(&stderr)
                .chars()
                .take(500)
                .collect::<String>(),
            String::from_utf8_lossy(&stdout)
                .chars()
                .take(500)
                .collect::<String>(),
        ));
    }
    if !stderr.is_empty() {
        return Err(format!(
            "default formal verifier invocation emitted stderr: {}",
            String::from_utf8_lossy(&stderr)
                .chars()
                .take(500)
                .collect::<String>()
        ));
    }
    if stdout.len() > RECEIPT_LIMIT {
        return Err("default formal verifier receipt exceeded 2048 bytes".to_owned());
    }
    let receipt_text =
        String::from_utf8(stdout).map_err(|_| "formal verifier receipt is not UTF-8".to_owned())?;
    if receipt_text.trim().is_empty() || receipt_text.trim().contains('\n') {
        return Err("default stdout must be exactly one bounded receipt line".to_owned());
    }
    if receipt_text.contains("# Formal Web UI Verification") || receipt_text.contains("## Findings")
    {
        return Err("default stdout leaked the report body".to_owned());
    }
    let receipt: Value = serde_json::from_str(receipt_text.trim())
        .map_err(|error| format!("default stdout is not a JSON receipt: {error}"))?;
    if receipt.get("exitCode") != Some(&json!(expected_exit)) {
        return Err("formal verifier receipt exit code mismatch".to_owned());
    }
    let (receipt_json, receipt_markdown) = receipt_paths(&receipt)?;
    if receipt_json != json_out || receipt_markdown != markdown_out {
        return Err(
            "formal verifier receipt does not point to the exact report artifacts".to_owned(),
        );
    }
    let report_bytes = read_bytes_nofollow(&json_out, Some(output))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "formal JSON report is missing".to_owned())?;
    let markdown = read_bytes_nofollow(&markdown_out, Some(output))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "formal Markdown report is missing".to_owned())?;
    let report: Value = serde_json::from_slice(&report_bytes)
        .map_err(|error| format!("formal JSON report is invalid: {error}"))?;
    let markdown =
        String::from_utf8(markdown).map_err(|_| "formal Markdown report is not UTF-8")?;
    validate_complete_report(&report, &markdown, output)?;
    Ok(report)
}

fn page_rules(page: &Value) -> Vec<(&str, &str)> {
    page.get("findings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|finding| {
            Some((
                finding.get("rule")?.as_str()?,
                finding.get("severity")?.as_str()?,
            ))
        })
        .collect()
}

fn visible_scrollbars(page: &Value) -> Vec<&Map<String, Value>> {
    page.pointer("/metrics/visibleScrollbars")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect()
}

fn validate_matrix_report(report: &Value, matrix: &Matrix) -> Result<(), String> {
    let pages = report
        .get("pages")
        .and_then(Value::as_array)
        .ok_or_else(|| "matrix report pages are missing".to_owned())?;
    let by_name = pages
        .iter()
        .filter_map(|page| {
            let declared = page.pointer("/target/name")?.as_str()?;
            let base = declared.split_once(" [").map_or(declared, |(name, _)| name);
            Some((base.to_owned(), page))
        })
        .collect::<BTreeMap<_, _>>();
    if by_name.len() != matrix.cases.len() {
        return Err(format!(
            "matrix report page count mismatch: expected {}, got {}",
            matrix.cases.len(),
            by_name.len()
        ));
    }
    let report_text = serde_json::to_string(report).map_err(|error| error.to_string())?;
    for case in &matrix.cases {
        let page = by_name
            .get(&case.name)
            .ok_or_else(|| format!("matrix report lacks {}", case.name))?;
        if page.get("outcome") != Some(&json!("checked")) {
            return Err(format!("matrix case {} was not checked", case.name));
        }
        let rules = page_rules(page);
        let has_critical = rules.iter().any(|(_, severity)| *severity == "critical");
        if has_critical != case.critical {
            return Err(format!(
                "matrix case {} critical result mismatch: expected {}, got {}",
                case.name, case.critical, has_critical
            ));
        }
        for rule in &case.critical_rules {
            if !rules
                .iter()
                .any(|(actual, severity)| actual == rule && *severity == "critical")
            {
                return Err(format!(
                    "matrix case {} lacks critical rule {rule}",
                    case.name
                ));
            }
        }
        for rule in &case.warning_rules {
            if !rules
                .iter()
                .any(|(actual, severity)| actual == rule && *severity == "warning")
            {
                return Err(format!(
                    "matrix case {} lacks warning rule {rule}",
                    case.name
                ));
            }
        }
        for rule in &case.forbidden_rules {
            if rules.iter().any(|(actual, _)| actual == rule) {
                return Err(format!(
                    "matrix case {} unexpectedly produced {rule}",
                    case.name
                ));
            }
        }
        for (rule, minimum) in &case.minimum_rule_counts {
            let count = rules.iter().filter(|(actual, _)| actual == rule).count();
            if count < *minimum {
                return Err(format!(
                    "matrix case {} produced {count} {rule} findings, expected at least {minimum}",
                    case.name
                ));
            }
        }
        for secret in &case.forbidden_text {
            if report_text.contains(secret) {
                return Err(format!(
                    "matrix case {} leaked forbidden report text",
                    case.name
                ));
            }
        }
        let scrollbars = visible_scrollbars(page);
        for expected in &case.scrollbars {
            if !scrollbars.iter().any(|entry| {
                entry
                    .get("selector")
                    .and_then(Value::as_str)
                    .is_some_and(|selector| selector.contains(&expected.selector_suffix))
                    && entry.get("axis") == Some(&json!(expected.axis))
                    && entry.get("sameAxisDepth") == Some(&json!(expected.depth))
            }) {
                return Err(format!(
                    "matrix case {} lacks scrollbar {} {} depth {}",
                    case.name, expected.selector_suffix, expected.axis, expected.depth
                ));
            }
        }
        for suffix in &case.forbidden_scrollbar_suffixes {
            if scrollbars.iter().any(|entry| {
                entry
                    .get("selector")
                    .and_then(Value::as_str)
                    .is_some_and(|selector| selector.contains(suffix))
            }) {
                return Err(format!(
                    "matrix case {} invented scrollbar {suffix}",
                    case.name
                ));
            }
        }
        let metrics = serde_json::to_string(page.get("metrics").unwrap_or(&Value::Null))
            .map_err(|error| error.to_string())?;
        for token in &case.metric_contains {
            if !metrics
                .to_ascii_lowercase()
                .contains(&token.to_ascii_lowercase())
            {
                return Err(format!("matrix case {} metrics lack {token}", case.name));
            }
        }
        if let Some(expected) = &case.final_path
            && page.get("finalPath") != Some(&json!(expected))
        {
            return Err(format!("matrix case {} final path mismatch", case.name));
        }
        if let Some(expected) = case.continuation_focus
            && page.pointer("/continuation/evidence/focusSatisfied") != Some(&json!(expected))
        {
            return Err(format!(
                "matrix case {} continuation focus mismatch",
                case.name
            ));
        }
    }
    Ok(())
}

fn validate_skill_contract(root: &Path) -> Result<(), String> {
    let path = root.join("skills/formal-web-ui-verification/SKILL.md");
    let bytes = read_bytes_nofollow(&path, Some(root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "formal UI skill contract is missing".to_owned())?;
    let contract =
        String::from_utf8(bytes).map_err(|_| "formal UI skill contract is not UTF-8".to_owned())?;
    let required = [
        "--human-readable-stdout",
        "--receipt-only",
        "human-only",
        "bounded",
        "control-text-clipped",
        "data-ui-verify-min-content-inset",
        "breakpointProfile",
        "maxPageCount",
        "sampled-only",
        "sourceBinding",
        "journey_review_contract.md",
        "review-queue.json",
        "journey-evidence.json",
        "formal_web_ui_review.py",
        "secondary-workflow-precedes-primary",
        "insufficient-text-contrast",
        "declared-theme-contradiction",
    ];
    let missing = required
        .iter()
        .filter(|token| !contract.contains(**token))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() || !contract.to_ascii_lowercase().contains("deprecated") {
        return Err(format!(
            "formal UI skill contract omits required behavior: {missing:?}"
        ));
    }
    let reference =
        root.join("skills/formal-web-ui-verification/references/journey_review_contract.md");
    let reference = read_bytes_nofollow(&reference, Some(root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "journey review reference is missing".to_owned())?;
    let reference =
        String::from_utf8(reference).map_err(|_| "journey review reference is not UTF-8")?;
    for token in [
        "primaryJourney",
        "continuation",
        "reviewInputs",
        "manual-review.json",
        "Screenshot SHA-256",
    ] {
        if !reference.contains(token) {
            return Err(format!("journey review reference lacks {token}"));
        }
    }
    let verifier = root.join("skills/formal-web-ui-verification/scripts/formal_web_ui_verify.mjs");
    let verifier = read_bytes_nofollow(&verifier, Some(root))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "formal Node verifier is missing".to_owned())?;
    let verifier = String::from_utf8(verifier).map_err(|_| "formal Node verifier is not UTF-8")?;
    if verifier.contains("settleMs = 120") || verifier.contains("waitForTimeout(120") {
        return Err("formal Node verifier retained a deliberate delay above 100 ms".to_owned());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct SelfTestOptions {
    pub workspace_parent: Option<PathBuf>,
    pub keep: bool,
    pub timeout_seconds: u64,
}

pub fn run(options: &SelfTestOptions) -> Result<Value, String> {
    if options.timeout_seconds == 0 {
        return Err("formal UI self-test timeout must be positive".to_owned());
    }
    let root = source_root();
    validate_skill_contract(&root)?;
    let pages = load_pages(&root)?;
    let matrix = load_matrix(&root)?;
    for case in &matrix.cases {
        if !pages.contains_key(&case.path) {
            return Err(format!(
                "formal UI matrix case {} references missing page {}",
                case.name, case.path
            ));
        }
    }
    let work = unique_work_dir(options.workspace_parent.as_deref())?;
    let result: Result<Value, String> = (|| {
        let server = StaticServer::start(pages)?;
        let config = build_matrix_config(&root, &server, &matrix);
        let expected_exit = if matrix.cases.iter().any(|case| case.critical) {
            1
        } else {
            0
        };
        let report = run_verifier(
            &root,
            &config,
            &work.join("core-matrix"),
            expected_exit,
            Duration::from_secs(options.timeout_seconds),
            &[],
        )?;
        validate_matrix_report(&report, &matrix)?;
        Ok(json!({
            "ok":true,"suite":"formal-web-ui-verification","static_cases":matrix.cases.len(),
            "report":work.join("core-matrix/report.json"),
        }))
    })();
    match result {
        Ok(mut summary) => {
            if options.keep {
                summary["workspace"] = json!(work);
            } else {
                validate_directory_nofollow(&work).map_err(|error| error.to_string())?;
                std::fs::remove_dir_all(&work)
                    .map_err(|error| format!("cannot clean self-test workspace: {error}"))?;
                summary["workspace"] = Value::Null;
            }
            Ok(summary)
        }
        Err(error) => Err(format!(
            "{error}; preserved self-test workspace: {}",
            work.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_fixture_matrix_is_complete_and_contract_bound() {
        let root = source_root();
        validate_skill_contract(&root).unwrap();
        let pages = load_pages(&root).unwrap();
        let matrix = load_matrix(&root).unwrap();
        assert!(matrix.cases.len() >= 60);
        assert!(
            matrix
                .cases
                .iter()
                .all(|case| pages.contains_key(&case.path))
        );
        assert!(matrix.cases.iter().any(|case| case.critical));
        assert!(matrix.cases.iter().any(|case| !case.critical));
        assert!(
            matrix
                .cases
                .iter()
                .any(|case| !case.forbidden_text.is_empty())
        );
    }
}
