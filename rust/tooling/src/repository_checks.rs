//! Portable repository-policy checks used by `devcoordinator2-tooling check`.
//!
//! Every public report is bounded and contains only rule identifiers, relative
//! paths, line numbers, and fixed explanatory text. Matched repository text,
//! forbidden instance strings, Git stderr, and private paths are never copied
//! into findings.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

pub const MAX_REPOSITORY_CHECK_FINDINGS: usize = 256;
const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CheckStatus {
    Clean,
    Findings,
}

impl CheckStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Findings => "findings",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CheckFinding {
    pub rule: String,
    pub path: String,
    pub line: Option<u64>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckReport {
    pub check: &'static str,
    pub status: CheckStatus,
    pub total_findings: usize,
    pub findings_truncated: bool,
    pub findings: Vec<CheckFinding>,
}

impl CheckReport {
    pub fn is_clean(&self) -> bool {
        self.status == CheckStatus::Clean
    }

    pub fn to_json(&self) -> Value {
        json!({
            "schema": 1,
            "check": self.check,
            "status": self.status.as_str(),
            "total_findings": self.total_findings,
            "findings_truncated": self.findings_truncated,
            "findings": self.findings.iter().map(|finding| json!({
                "rule": finding.rule,
                "path": finding.path,
                "line": finding.line,
                "detail": finding.detail,
            })).collect::<Vec<_>>(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckErrorKind {
    RepositoryUnavailable,
    InputUnavailable,
    InvalidInput,
    GitUnavailable,
    GitFailed,
    GitOutputInvalid,
    InputChanged,
}

impl CheckErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RepositoryUnavailable => "repository_unavailable",
            Self::InputUnavailable => "input_unavailable",
            Self::InvalidInput => "invalid_input",
            Self::GitUnavailable => "git_unavailable",
            Self::GitFailed => "git_failed",
            Self::GitOutputInvalid => "git_output_invalid",
            Self::InputChanged => "input_changed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckError {
    pub kind: CheckErrorKind,
    pub message: String,
}

impl CheckError {
    fn new(kind: CheckErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CheckError {}

#[derive(Default)]
struct FindingCollector {
    findings: Vec<CheckFinding>,
    total: usize,
}

impl FindingCollector {
    fn push(&mut self, finding: CheckFinding) {
        self.total += 1;
        if self.findings.len() < MAX_REPOSITORY_CHECK_FINDINGS {
            self.findings.push(finding);
        }
    }

    fn finish(mut self, check: &'static str) -> CheckReport {
        self.findings.sort();
        CheckReport {
            check,
            status: if self.total == 0 {
                CheckStatus::Clean
            } else {
                CheckStatus::Findings
            },
            total_findings: self.total,
            findings_truncated: self.total > self.findings.len(),
            findings: self.findings,
        }
    }
}

fn finding(rule: &str, path: &str, line: Option<usize>, detail: &str) -> CheckFinding {
    CheckFinding {
        rule: rule.to_owned(),
        path: bounded_string(path),
        line: line.and_then(|value| u64::try_from(value).ok()),
        detail: detail.to_owned(),
    }
}

fn bounded_string(value: &str) -> String {
    if value.len() <= MAX_PATH_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_PATH_BYTES.saturating_sub(3);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value.to_string_lossy().into_owned(),
            Component::ParentDir => "..".to_owned(),
            Component::CurDir => ".".to_owned(),
            Component::RootDir => String::new(),
            Component::Prefix(value) => value.as_os_str().to_string_lossy().into_owned(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn canonical_directory(path: &Path) -> Result<PathBuf, CheckError> {
    let canonical = path.canonicalize().map_err(|_| {
        CheckError::new(
            CheckErrorKind::RepositoryUnavailable,
            "repository directory is unavailable",
        )
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CheckError::new(
            CheckErrorKind::RepositoryUnavailable,
            "repository directory is unavailable",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CheckError::new(
            CheckErrorKind::RepositoryUnavailable,
            "repository must be a regular non-symlinked directory",
        ));
    }
    Ok(canonical)
}

fn command_output(mut command: Command) -> Result<Output, CheckError> {
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let output = command.output().map_err(|error| {
        let kind = if error.kind() == io::ErrorKind::NotFound {
            CheckErrorKind::GitUnavailable
        } else {
            CheckErrorKind::GitFailed
        };
        CheckError::new(kind, "could not execute Git")
    })?;
    if output.stdout.len() > MAX_GIT_OUTPUT_BYTES || output.stderr.len() > MAX_GIT_OUTPUT_BYTES {
        return Err(CheckError::new(
            CheckErrorKind::GitOutputInvalid,
            "Git returned an oversized result",
        ));
    }
    Ok(output)
}

fn git_output(repo: &Path, args: &[&str], require_success: bool) -> Result<Output, CheckError> {
    let mut command = Command::new("git");
    command.current_dir(repo).args(args);
    let output = command_output(command)?;
    if require_success && !output.status.success() {
        return Err(CheckError::new(
            CheckErrorKind::GitFailed,
            format!(
                "Git operation failed with exit code {}",
                output.status.code().unwrap_or(-1)
            ),
        ));
    }
    Ok(output)
}

fn utf8_stdout(output: &Output) -> Result<&str, CheckError> {
    std::str::from_utf8(&output.stdout).map_err(|_| {
        CheckError::new(
            CheckErrorKind::GitOutputInvalid,
            "Git returned non-UTF-8 output",
        )
    })
}

fn read_bounded_utf8(path: &Path) -> Result<String, CheckError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputUnavailable,
            "input file is unavailable",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "input must be a regular non-symlinked file",
        ));
    }
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "input file is too large",
        ));
    }
    fs::read_to_string(path).map_err(|error| {
        if error.kind() == io::ErrorKind::InvalidData {
            CheckError::new(CheckErrorKind::InvalidInput, "input must be valid UTF-8")
        } else {
            CheckError::new(
                CheckErrorKind::InputUnavailable,
                "input file is unavailable",
            )
        }
    })
}

fn walk_tree(root: &Path) -> Result<Vec<PathBuf>, CheckError> {
    fn visit(root: &Path, current: &Path, result: &mut Vec<PathBuf>) -> Result<(), CheckError> {
        let entries = fs::read_dir(current).map_err(|_| {
            CheckError::new(
                CheckErrorKind::InputUnavailable,
                "could not traverse repository content",
            )
        })?;
        let mut entries = entries.collect::<Result<Vec<_>, _>>().map_err(|_| {
            CheckError::new(
                CheckErrorKind::InputUnavailable,
                "could not traverse repository content",
            )
        })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).map_err(|_| {
                CheckError::new(
                    CheckErrorKind::InvalidInput,
                    "repository traversal escaped its root",
                )
            })?;
            if relative.components().any(|component| {
                matches!(component, Component::Normal(value) if value == OsStr::new(".git") || value == OsStr::new("node_modules") || value == OsStr::new("__pycache__") || value == OsStr::new("target"))
            }) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(|_| {
                CheckError::new(
                    CheckErrorKind::InputChanged,
                    "repository entry changed during traversal",
                )
            })?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                visit(root, &path, result)?;
            } else if metadata.is_file() {
                result.push(relative.to_path_buf());
            }
        }
        Ok(())
    }

    let mut result = Vec::new();
    visit(root, root, &mut result)?;
    result.sort();
    Ok(result)
}

/// Scan every Git-committable text file for installation-specific strings.
pub fn check_no_instance_data(
    repository_root: &Path,
    patterns_file: &Path,
) -> Result<CheckReport, CheckError> {
    let root = canonical_directory(repository_root)?;
    let patterns_text = read_bounded_utf8(patterns_file)?;
    let patterns = patterns_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if patterns.is_empty() {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "instance-data pattern list is empty",
        ));
    }

    let output = git_output(
        &root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        true,
    )?;
    let names = utf8_stdout(&output)?;
    let mut collector = FindingCollector::default();
    for name in names.split('\0').filter(|name| !name.is_empty()) {
        let relative = Path::new(name);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            collector.push(finding(
                "unsafe-committable-path",
                name,
                None,
                "Git reported a path outside the repository",
            ));
            continue;
        }
        let path = root.join(relative);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_TEXT_BYTES
        {
            continue;
        }
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        let Ok(text) = String::from_utf8(data) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let folded = line.to_lowercase();
            for pattern in &patterns {
                if folded.contains(pattern) {
                    collector.push(finding(
                        "instance-data",
                        &slash_path(relative),
                        Some(index + 1),
                        "contains a forbidden instance-specific pattern",
                    ));
                }
            }
        }
    }
    Ok(collector.finish("no_instance_data"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceLanguage {
    Python,
    JavaScript,
    Rust,
}

fn strip_comments_and_strings(text: &str, language: SourceLanguage) -> String {
    let bytes = text.as_bytes();
    let mut output = bytes.to_vec();
    let mut index = 0usize;
    let mut quote = None;
    let mut triple = false;
    let mut escaped = false;
    let mut block_comment = false;
    while index < bytes.len() {
        if block_comment {
            output[index] = if bytes[index] == b'\n' { b'\n' } else { b' ' };
            if index + 1 < bytes.len() && bytes[index] == b'*' && bytes[index + 1] == b'/' {
                output[index + 1] = b' ';
                block_comment = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            output[index] = if bytes[index] == b'\n' { b'\n' } else { b' ' };
            if escaped {
                escaped = false;
                index += 1;
                continue;
            }
            if bytes[index] == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            if triple
                && index + 2 < bytes.len()
                && bytes[index] == delimiter
                && bytes[index + 1] == delimiter
                && bytes[index + 2] == delimiter
            {
                output[index + 1] = b' ';
                output[index + 2] = b' ';
                quote = None;
                triple = false;
                index += 3;
            } else if !triple && bytes[index] == delimiter {
                quote = None;
                index += 1;
            } else {
                index += 1;
            }
            continue;
        }
        if language != SourceLanguage::Python
            && index + 1 < bytes.len()
            && bytes[index] == b'/'
            && bytes[index + 1] == b'*'
        {
            output[index] = b' ';
            output[index + 1] = b' ';
            block_comment = true;
            index += 2;
            continue;
        }
        let line_comment = match language {
            SourceLanguage::Python => bytes[index] == b'#',
            SourceLanguage::JavaScript | SourceLanguage::Rust => {
                index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'/'
            }
        };
        if line_comment {
            while index < bytes.len() && bytes[index] != b'\n' {
                output[index] = b' ';
                index += 1;
            }
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"' | b'`') {
            let delimiter = bytes[index];
            let python_triple = language == SourceLanguage::Python
                && delimiter != b'`'
                && index + 2 < bytes.len()
                && bytes[index + 1] == delimiter
                && bytes[index + 2] == delimiter;
            output[index] = b' ';
            if python_triple {
                output[index + 1] = b' ';
                output[index + 2] = b' ';
                index += 3;
            } else {
                index += 1;
            }
            quote = Some(delimiter);
            triple = python_triple;
            continue;
        }
        index += 1;
    }
    String::from_utf8(output).expect("input began as UTF-8")
}

fn line_at(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

fn first_call_argument(text: &str, call_end: usize) -> Option<&str> {
    let tail = &text[call_end..];
    let mut depth = 0usize;
    for (index, character) in tail.char_indices() {
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(tail[..index].trim()),
            ')' => depth -= 1,
            ',' if depth == 0 => return Some(tail[..index].trim()),
            _ => {}
        }
    }
    None
}

fn parse_first_numeric_argument(text: &str, call_end: usize) -> Option<f64> {
    let mut argument = first_call_argument(text, call_end)?;
    if argument.starts_with(['-', '+']) {
        return None;
    }
    while argument.starts_with('(') && argument.ends_with(')') {
        argument = argument[1..argument.len() - 1].trim();
    }
    argument.parse::<f64>().ok()
}

fn duration_millis(expression: &str) -> Option<f64> {
    let compact = expression
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    for (constructor, multiplier) in [
        ("Duration::from_millis(", 1.0),
        ("std::time::Duration::from_millis(", 1.0),
        ("Duration::from_secs_f64(", 1_000.0),
        ("std::time::Duration::from_secs_f64(", 1_000.0),
    ] {
        if let Some(value) = compact
            .strip_prefix(constructor)
            .and_then(|value| value.strip_suffix(')'))
            .and_then(|value| value.parse::<f64>().ok())
        {
            return Some(value * multiplier);
        }
    }
    None
}

fn rust_wait_is_bounded(text: &str, call_end: usize) -> bool {
    let Some(argument) = first_call_argument(text, call_end) else {
        return false;
    };
    if duration_millis(argument).is_some_and(|millis| millis <= 100.0) {
        return true;
    }
    let compact = argument
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let Some((name, _)) = compact.split_once(".min(") else {
        return false;
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return false;
    }
    let declaration = format!("const {name}:");
    text.lines().any(|line| {
        line.trim_start().starts_with(&declaration)
            && line
                .split_once('=')
                .and_then(|(_, value)| value.trim().strip_suffix(';'))
                .and_then(duration_millis)
                .is_some_and(|millis| millis <= 100.0)
    })
}

/// Inspect one governed source without returning matched source text.
pub fn scan_timer_waits_in_text(
    text: &str,
    relative_path: &str,
    language: SourceLanguage,
) -> Vec<CheckFinding> {
    let clean = if language == SourceLanguage::JavaScript {
        text.to_owned()
    } else {
        strip_comments_and_strings(text, language)
    };
    let markers: &[&str] = match language {
        SourceLanguage::Python => &["time.sleep(", "asyncio.sleep("],
        SourceLanguage::JavaScript => &["waitForTimeout(", "setTimeout("],
        SourceLanguage::Rust => &[
            "thread::sleep(",
            "std::thread::sleep(",
            "tokio::time::sleep(",
        ],
    };
    let mut findings = Vec::new();
    for marker in markers {
        let mut offset = 0usize;
        while let Some(relative) = clean[offset..].find(marker) {
            let found = offset + relative;
            let end = found + marker.len();
            let allowed = match language {
                SourceLanguage::Python => {
                    parse_first_numeric_argument(&clean, end).is_some_and(|seconds| seconds <= 0.1)
                }
                SourceLanguage::JavaScript => false,
                SourceLanguage::Rust => rust_wait_is_bounded(&clean, end),
            };
            if !allowed {
                findings.push(finding(
                    "clock-based-wait",
                    relative_path,
                    Some(line_at(&clean, found)),
                    "clock-based sleep is not completion evidence",
                ));
            }
            offset = end;
        }
    }
    findings.sort();
    findings
}

/// Reject clock-based progression in governed test and verification code.
pub fn check_no_test_timer_waits(root: &Path) -> Result<CheckReport, CheckError> {
    let root = canonical_directory(root)?;
    let legacy_python_targets = [
        "src/devcoordinator2/daemon/test_admission.py",
        "src/devcoordinator2/daemon/test_postgres.py",
        "src/devcoordinator2/daemon/tests_lifecycle.py",
        "src/devcoordinator2/daemon/systemd_unit.py",
        "src/devcoordinator2/daemon/metrics_sampler.py",
        "scripts/install.py",
        "tests/integration/helpers.py",
    ];
    let mut targets = BTreeMap::<PathBuf, SourceLanguage>::new();
    for relative in legacy_python_targets {
        let path = PathBuf::from(relative);
        if root.join(&path).is_file() {
            targets.insert(path, SourceLanguage::Python);
        }
    }
    let integration = root.join("tests/integration");
    if integration.is_dir() {
        for relative in walk_tree(&integration)? {
            let name = relative
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default();
            if name.starts_with("test_") && name.ends_with(".py") {
                targets.insert(
                    PathBuf::from("tests/integration").join(relative),
                    SourceLanguage::Python,
                );
            }
        }
    }
    let console = PathBuf::from("console/verify.mjs");
    if root.join(&console).is_file() {
        targets.insert(console, SourceLanguage::JavaScript);
    }
    for directory in ["rust/control/src", "rust/control/tests"] {
        let absolute = root.join(directory);
        if absolute.is_dir() {
            for relative in walk_tree(&absolute)? {
                if relative.extension() == Some(OsStr::new("rs")) {
                    targets.insert(
                        PathBuf::from(directory).join(relative),
                        SourceLanguage::Rust,
                    );
                }
            }
        }
    }

    let mut collector = FindingCollector::default();
    for (relative, language) in targets {
        let text = read_bounded_utf8(&root.join(&relative))?;
        for item in scan_timer_waits_in_text(&text, &slash_path(&relative), language) {
            collector.push(item);
        }
    }
    Ok(collector.finish("no_test_timer_waits"))
}

fn ascii_word_boundary(text: &str, start: usize, length: usize) -> bool {
    let bytes = text.as_bytes();
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    (start == 0 || !word(bytes[start - 1]))
        && (start + length == bytes.len() || !word(bytes[start + length]))
}

fn contains_word_case_insensitive(text: &str, needle: &str) -> bool {
    let folded = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut offset = 0usize;
    while let Some(relative) = folded[offset..].find(&needle) {
        let start = offset + relative;
        if ascii_word_boundary(&folded, start, needle.len()) {
            return true;
        }
        offset = start + needle.len();
    }
    false
}

fn neutrality_paths(root: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>), CheckError> {
    let mut markup = vec![
        PathBuf::from("reference/universal/AGENTS.md"),
        PathBuf::from("SKILL_AUDIT.md"),
    ];
    let skills = root.join("skills");
    if skills.is_dir() {
        let mut directories = fs::read_dir(&skills)
            .map_err(|_| CheckError::new(CheckErrorKind::InputUnavailable, "cannot read skills"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| CheckError::new(CheckErrorKind::InputUnavailable, "cannot read skills"))?;
        directories.sort_by_key(|entry| entry.file_name());
        for entry in directories {
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
                CheckError::new(
                    CheckErrorKind::InputChanged,
                    "skill entry changed during traversal",
                )
            })?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                continue;
            }
            let prefix = PathBuf::from("skills").join(entry.file_name());
            for name in ["SKILL.md", "README.md"] {
                let relative = prefix.join(name);
                if root.join(&relative).is_file() {
                    markup.push(relative);
                }
            }
            for directory in ["references", "agents"] {
                let absolute = root.join(&prefix).join(directory);
                if absolute.is_dir() {
                    for child in walk_tree(&absolute)? {
                        let include = if directory == "references" {
                            child.extension() == Some(OsStr::new("md"))
                        } else {
                            child
                                .parent()
                                .is_some_and(|parent| parent.as_os_str().is_empty())
                                && child.extension() == Some(OsStr::new("yaml"))
                        };
                        if include {
                            markup.push(prefix.join(directory).join(child));
                        }
                    }
                }
            }
        }
    }
    markup.sort();
    markup.dedup();

    let mut prompts = Vec::new();
    let queue = PathBuf::from("full_repo_harness/queue.py");
    if root.join(&queue).is_file() {
        prompts.push(queue);
    }
    if skills.is_dir() {
        for relative in walk_tree(&skills)? {
            let components = relative.components().collect::<Vec<_>>();
            let name = relative
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default();
            if components.len() == 3
                && components[1].as_os_str() == OsStr::new("scripts")
                && name.starts_with("build")
                && name.ends_with(".py")
            {
                prompts.push(PathBuf::from("skills").join(relative));
            }
        }
    }
    prompts.sort();
    Ok((markup, prompts))
}

fn scan_neutrality_file(collector: &mut FindingCollector, text: &str, path: &str, prompt: bool) {
    for (index, line) in text.lines().enumerate() {
        let folded = line.to_ascii_lowercase();
        let runtime_name = if prompt {
            contains_word_case_insensitive(line, "in Codex")
                || contains_word_case_insensitive(line, "Codex worker")
                || contains_word_case_insensitive(line, "Codex workers")
                || contains_word_case_insensitive(line, "Codex skills")
                || contains_word_case_insensitive(line, "Claude Code")
        } else {
            contains_word_case_insensitive(line, "Codex")
                || contains_word_case_insensitive(line, "Claude")
                || contains_word_case_insensitive(line, "Claude Code")
        };
        if runtime_name {
            collector.push(finding(
                "runtime-name",
                path,
                Some(index + 1),
                "shared contract is runtime-specific",
            ));
        }
        if !prompt
            && (folded.contains(".codex/")
                || folded.contains(".codex\\")
                || folded.contains(".claude/")
                || folded.contains(".claude\\")
                || contains_word_case_insensitive(line, ".codex")
                || contains_word_case_insensitive(line, ".claude"))
        {
            collector.push(finding(
                "runtime-home",
                path,
                Some(index + 1),
                "shared contract is runtime-specific",
            ));
        }
        if ["fork_turns", "request_user_input", "AskUserQuestion"]
            .iter()
            .any(|needle| line.contains(needle))
        {
            collector.push(finding(
                "runtime-api",
                path,
                Some(index + 1),
                "shared contract is runtime-specific",
            ));
        }
    }
}

/// Reject runtime-specific assumptions from shared contracts and prompts.
pub fn audit_agent_neutrality(root: &Path) -> Result<CheckReport, CheckError> {
    let root = canonical_directory(root)?;
    let (markup, prompts) = neutrality_paths(&root)?;
    let mut collector = FindingCollector::default();
    for relative in markup {
        let path = root.join(&relative);
        if !path.is_file() {
            continue;
        }
        let text = read_bounded_utf8(&path)?;
        scan_neutrality_file(&mut collector, &text, &slash_path(&relative), false);
    }
    for relative in prompts {
        let text = read_bounded_utf8(&root.join(&relative))?;
        scan_neutrality_file(&mut collector, &text, &slash_path(&relative), true);
    }
    Ok(collector.finish("agent_neutrality"))
}

fn yaml_without_comments(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            line.split('#')
                .next()
                .unwrap_or_default()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn workflow_has_pull_request_trigger(lines: &[String]) -> bool {
    for (index, line) in lines.iter().enumerate() {
        let Some(inline) = line.strip_prefix("on:") else {
            continue;
        };
        if contains_word_case_insensitive(inline, "pull_request")
            || contains_word_case_insensitive(inline, "pull_request_target")
        {
            return true;
        }
        for nested in &lines[index + 1..] {
            if !nested.is_empty() && !nested.starts_with(' ') {
                break;
            }
            let trimmed = nested.trim_start();
            if trimmed.starts_with("pull_request:") || trimmed.starts_with("pull_request_target:") {
                return true;
            }
        }
        return false;
    }
    false
}

fn valid_yaml_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn workflow_job_blocks(lines: &[String]) -> Result<BTreeMap<String, Vec<String>>, CheckError> {
    let Some(jobs_index) = lines.iter().position(|line| line == "jobs:") else {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "workflow has no top-level jobs section",
        ));
    };
    let mut jobs = BTreeMap::<String, Vec<String>>::new();
    let mut current: Option<String> = None;
    for line in &lines[jobs_index + 1..] {
        if !line.is_empty()
            && !line.starts_with(' ')
            && line
                .strip_suffix(':')
                .is_some_and(|name| valid_yaml_name(name.trim()))
        {
            break;
        }
        if line.starts_with("  ")
            && !line.starts_with("    ")
            && let Some(name) = line
                .strip_prefix("  ")
                .and_then(|value| value.strip_suffix(':'))
            && valid_yaml_name(name)
        {
            current = Some(name.to_owned());
            jobs.entry(name.to_owned()).or_default();
            continue;
        }
        if let Some(name) = &current {
            jobs.get_mut(name)
                .expect("job was inserted")
                .push(line.clone());
        }
    }
    if jobs.is_empty() {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "workflow jobs section is empty or malformed",
        ));
    }
    Ok(jobs)
}

fn strip_yaml_quotes(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn hosted_runner(value: &str) -> bool {
    let value = strip_yaml_quotes(value);
    let Some((family, version)) = value.rsplit_once('-') else {
        return false;
    };
    match family {
        "ubuntu" => {
            version == "latest"
                || (!version.is_empty()
                    && version.split('.').all(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    }))
        }
        "windows" | "macos" => {
            version == "latest"
                || (!version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit()))
        }
        _ => false,
    }
}

fn matrix_axis_values(lines: &[String], axis: &str) -> Option<Vec<String>> {
    let prefix = format!("        {axis}:");
    for line in lines {
        if let Some(inline) = line.strip_prefix(&prefix) {
            let inner = inline.trim().strip_prefix('[')?.strip_suffix(']')?;
            return Some(
                inner
                    .split(',')
                    .map(strip_yaml_quotes)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect(),
            );
        }
    }
    None
}

fn matrix_expression(value: &str) -> Option<&str> {
    let compact = value.trim();
    let inner = compact.strip_prefix("${{")?.strip_suffix("}}")?.trim();
    inner
        .strip_prefix("matrix.")
        .filter(|axis| valid_yaml_name(axis))
}

fn job_runs_self_hosted(lines: &[String]) -> bool {
    for (index, line) in lines.iter().enumerate() {
        let Some(inline) = line.strip_prefix("    runs-on:") else {
            continue;
        };
        let inline = inline.trim();
        if !inline.is_empty() {
            if let Some(axis) = matrix_expression(inline) {
                return matrix_axis_values(lines, axis)
                    .is_none_or(|values| values.iter().any(|value| !hosted_runner(value)));
            }
            if let Some(inner) = inline
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
            {
                let values = inner
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>();
                return values.len() != 1 || !hosted_runner(values[0]);
            }
            return !hosted_runner(inline);
        }
        let mut values = Vec::new();
        for nested in &lines[index + 1..] {
            let indent = nested.len() - nested.trim_start_matches(' ').len();
            if !nested.is_empty() && indent <= 4 {
                break;
            }
            if let Some(value) = nested.trim_start().strip_prefix("- ") {
                values.push(value.trim());
            }
        }
        return values.len() != 1 || !hosted_runner(values[0]);
    }
    false
}

fn job_condition(lines: &[String]) -> &str {
    lines
        .iter()
        .find_map(|line| line.strip_prefix("    if:"))
        .map(str::trim)
        .unwrap_or_default()
}

fn trusted_job_condition(condition: &str) -> bool {
    let compact = condition
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let Some(body) = compact
        .strip_prefix("${{")
        .and_then(|value| value.strip_suffix("}}"))
    else {
        return false;
    };
    let body = body
        .replace("\"push\"", "'push'")
        .replace("\"workflow_dispatch\"", "'workflow_dispatch'");
    let forward = "github.event_name=='push'||github.event_name=='workflow_dispatch'";
    let reverse = "github.event_name=='workflow_dispatch'||github.event_name=='push'";
    let allowed = |value: &str| value == forward || value == reverse;
    if allowed(&body)
        || body
            .strip_prefix('(')
            .and_then(|value| value.strip_suffix(')'))
            .is_some_and(allowed)
    {
        return true;
    }
    let Some((gate, events)) = body.split_once("&&(") else {
        return false;
    };
    let Some(events) = events.strip_suffix(')') else {
        return false;
    };
    let Some((variable, value)) = gate.split_once("=='") else {
        return false;
    };
    let Some(value) = value.strip_suffix('\'') else {
        return false;
    };
    let variable = variable.strip_prefix("vars.").unwrap_or_default();
    let mut variable_bytes = variable.bytes();
    variable_bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && variable_bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_'))
        && !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        && allowed(events)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiSecurityResult {
    pub report: CheckReport,
    pub self_hosted_jobs: usize,
    pub pull_request: bool,
}

impl CiSecurityResult {
    pub fn to_json(&self) -> Value {
        let mut report = self.report.to_json();
        if let Some(object) = report.as_object_mut() {
            object.insert("self_hosted_jobs".into(), self.self_hosted_jobs.into());
            object.insert("pull_request".into(), self.pull_request.into());
        }
        report
    }
}

/// Inspect the repository's intentionally simple workflow shape.
pub fn find_ci_security_violations(text: &str) -> Result<CiSecurityResult, CheckError> {
    let lines = yaml_without_comments(text);
    let jobs = workflow_job_blocks(&lines)?;
    let pull_request = workflow_has_pull_request_trigger(&lines);
    let self_hosted = jobs
        .iter()
        .filter(|(_, lines)| job_runs_self_hosted(lines))
        .collect::<Vec<_>>();
    let mut collector = FindingCollector::default();
    if pull_request {
        for (name, lines) in &self_hosted {
            if !trusted_job_condition(job_condition(lines)) {
                collector.push(finding(
                    "self-hosted-pull-request",
                    ".github/workflows/validate.yml",
                    None,
                    &format!(
                        "self-hosted job '{name}' must have a job-level event allowlist for only push and workflow_dispatch while pull_request is enabled"
                    ),
                ));
            }
        }
    }
    Ok(CiSecurityResult {
        report: collector.finish("ci_security"),
        self_hosted_jobs: self_hosted.len(),
        pull_request,
    })
}

pub fn check_ci_workflow(path: &Path) -> Result<CiSecurityResult, CheckError> {
    let text = read_bounded_utf8(path)?;
    let mut result = find_ci_security_violations(&text)?;
    let display = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("workflow");
    for item in &mut result.report.findings {
        item.path = bounded_string(display);
    }
    Ok(result)
}

pub const CANONICAL_SKILLS: [&str; 6] = [
    "dev-coordinator",
    "formal-web-ui-verification",
    "full-repo-audit",
    "full-repo-test-coverage-audit",
    "ui-implementation-audit",
    "user-journey-docs-audit",
];

fn active_boundary_file(relative: &Path) -> bool {
    let name = slash_path(relative);
    if matches!(
        name.as_str(),
        "scripts/skills/check_repository_boundaries.py"
            | "scripts/skills/check_repository_boundaries_self_test.py"
            | "rust/tooling/src/repository_checks.rs"
            | "DecisionHistory.md"
            | "docs/holy-skills-snapshot.md"
    ) {
        return false;
    }
    let root_files = [
        ".gitmodules",
        "AGENTS.md",
        "CLAUDE.md",
        "README.md",
        "security-assumptions.md",
    ];
    let active_directories = [
        ".github",
        "deploy",
        "edge",
        "reference",
        "rust",
        "scripts",
        "skills",
        "src",
    ];
    root_files.contains(&name.as_str())
        || relative.components().next().is_some_and(|component| {
            active_directories
                .iter()
                .any(|item| component.as_os_str() == OsStr::new(item))
        })
}

fn boundary_rules(line: &str) -> Vec<&'static str> {
    let folded = line.to_ascii_lowercase();
    let mut rules = Vec::new();
    if let Some(index) = folded.find("/home/holyskills") {
        let following = folded
            .as_bytes()
            .get(index + "/home/holyskills".len())
            .copied();
        if following.is_none_or(|byte| {
            byte.is_ascii_whitespace() || matches!(byte, b'/' | b'\'' | b'"' | b'`')
        }) {
            rules.push("retired-checkout");
        }
    }
    let without_dot_git = folded.replace(".git", "");
    if let Some(index) = without_dot_git.find("/holyskills") {
        let prefix = &without_dot_git[..index];
        let owner = prefix.rsplit(['/', ':']).next().unwrap_or_default();
        let following = without_dot_git
            .as_bytes()
            .get(index + "/holyskills".len())
            .copied();
        if !owner.is_empty()
            && owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
            && following.is_none_or(|byte| !byte.is_ascii_alphanumeric() && byte != b'_')
        {
            rules.push("retired-remote");
        }
    }
    if folded.contains("reference/codex-app-wide") {
        rules.push("retired-policy-path");
    }
    if ["skills/codex-dev-coordinator", "$codex-dev-coordinator"]
        .iter()
        .any(|needle| {
            folded.find(needle).is_some_and(|index| {
                folded
                    .as_bytes()
                    .get(index + needle.len())
                    .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
            })
        })
    {
        rules.push("retired-skill-name");
    }
    if [
        "/opt/devcoordinator2/current",
        "/opt/devcoordinator2/releases",
    ]
    .iter()
    .any(|needle| {
        folded.find(needle).is_some_and(|index| {
            folded
                .as_bytes()
                .get(index + needle.len())
                .is_none_or(|byte| {
                    byte.is_ascii_whitespace() || matches!(*byte, b'/' | b'\'' | b'"' | b'`')
                })
        })
    }) {
        rules.push("retired-release-source");
    }
    rules
}

/// Verify one live checkout, one universal policy, and exactly six skills.
pub fn audit_repository_boundaries(repository: &Path) -> Result<CheckReport, CheckError> {
    let root = canonical_directory(repository)?;
    let mut collector = FindingCollector::default();
    let skills = root.join("skills");
    let mut actual = BTreeSet::new();
    if skills.is_dir() {
        for entry in fs::read_dir(&skills).map_err(|_| {
            CheckError::new(CheckErrorKind::InputUnavailable, "cannot inspect skills")
        })? {
            let entry = entry.map_err(|_| {
                CheckError::new(
                    CheckErrorKind::InputChanged,
                    "skill entry changed during traversal",
                )
            })?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
                CheckError::new(
                    CheckErrorKind::InputChanged,
                    "skill entry changed during traversal",
                )
            })?;
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && entry.path().join("SKILL.md").is_file()
            {
                actual.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    let expected = CANONICAL_SKILLS
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<BTreeSet<_>>();
    if actual != expected {
        collector.push(finding(
            "canonical-skill-set",
            "skills",
            None,
            "the repository must contain exactly the six canonical skill sources",
        ));
    }
    let policy = root.join("reference/universal/AGENTS.md");
    if fs::symlink_metadata(&policy)
        .map(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
        .unwrap_or(true)
    {
        collector.push(finding(
            "canonical-policy",
            "reference/universal/AGENTS.md",
            None,
            "regular policy file is missing",
        ));
    }
    for retired in ["reference/codex-app-wide", "skills/codex-dev-coordinator"] {
        if fs::symlink_metadata(root.join(retired)).is_ok() {
            collector.push(finding(
                "retired-path-present",
                retired,
                None,
                "retired path exists",
            ));
        }
    }
    if fs::symlink_metadata(root.join(".gitmodules")).is_ok() {
        collector.push(finding(
            "submodule-present",
            ".gitmodules",
            None,
            "submodules are not part of the unified source",
        ));
    }
    for relative in walk_tree(&root)? {
        if !active_boundary_file(&relative) {
            continue;
        }
        let path = root.join(&relative);
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            CheckError::new(
                CheckErrorKind::InputChanged,
                "repository entry changed during traversal",
            )
        })?;
        if metadata.len() > MAX_TEXT_BYTES {
            continue;
        }
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        if data.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(data) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            for rule in boundary_rules(line) {
                collector.push(finding(
                    rule,
                    &slash_path(&relative),
                    Some(index + 1),
                    "active source references retired ownership",
                ));
            }
        }
    }
    Ok(collector.finish("repository_boundaries"))
}

pub const REPOSITORY_STALE_EXIT: i32 = 2;
pub const REPOSITORY_REMOTE_UNAVAILABLE_EXIT: i32 = 3;
pub const REPOSITORY_USAGE_ERROR_EXIT: i32 = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryFreshness {
    pub schema_version: u8,
    pub status: String,
    pub relation: String,
    pub ok: bool,
    pub repo: String,
    pub remote: String,
    pub branch: Option<String>,
    pub remote_ref: Option<String>,
    pub head: Option<String>,
    pub remote_head: Option<String>,
    pub merge_base: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub dirty: bool,
    pub fetched: bool,
    pub detail: Option<String>,
}

impl RepositoryFreshness {
    fn base(repo: &Path, remote: &str, dirty: bool, head: Option<String>) -> Self {
        Self {
            schema_version: 1,
            status: "remote-unavailable".to_owned(),
            relation: "unknown".to_owned(),
            ok: false,
            repo: bounded_string(&repo.to_string_lossy()),
            remote: bounded_string(remote),
            branch: None,
            remote_ref: None,
            head,
            remote_head: None,
            merge_base: None,
            ahead: 0,
            behind: 0,
            dirty,
            fetched: false,
            detail: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryFreshnessResult {
    pub exit_code: i32,
    pub payload: RepositoryFreshness,
}

impl RepositoryFreshnessResult {
    pub fn to_json(&self) -> Value {
        let payload = &self.payload;
        json!({
            "schema_version": payload.schema_version,
            "status": payload.status,
            "relation": payload.relation,
            "ok": payload.ok,
            "repo": payload.repo,
            "remote": payload.remote,
            "branch": payload.branch,
            "remote_ref": payload.remote_ref,
            "head": payload.head,
            "remote_head": payload.remote_head,
            "merge_base": payload.merge_base,
            "ahead": payload.ahead,
            "behind": payload.behind,
            "dirty": payload.dirty,
            "fetched": payload.fetched,
            "detail": payload.detail,
        })
    }
}

fn git_optional(repo: &Path, args: &[&str]) -> Result<Output, CheckError> {
    git_output(repo, args, false)
}

fn git_text(
    repo: &Path,
    args: &[&str],
    require_success: bool,
) -> Result<(bool, String), CheckError> {
    let output = git_output(repo, args, require_success)?;
    Ok((
        output.status.success(),
        utf8_stdout(&output)?.trim().to_owned(),
    ))
}

fn resolve_remote_branch(
    repo: &Path,
    remote: &str,
    requested: Option<&str>,
) -> Result<String, CheckError> {
    if let Some(requested) = requested {
        return Ok(requested
            .strip_prefix("refs/heads/")
            .unwrap_or(requested)
            .to_owned());
    }
    let remote_head = format!("refs/remotes/{remote}/HEAD");
    let (success, symbolic) = git_text(
        repo,
        &["symbolic-ref", "--quiet", "--short", &remote_head],
        false,
    )?;
    let prefix = format!("{remote}/");
    if success && symbolic.starts_with(&prefix) {
        return Ok(symbolic[prefix.len()..].to_owned());
    }
    let (success, advertised) = git_text(repo, &["ls-remote", "--symref", remote, "HEAD"], false)?;
    if success {
        for line in advertised.lines() {
            if let Some(branch) = line
                .strip_prefix("ref: refs/heads/")
                .and_then(|value| value.strip_suffix("\tHEAD"))
            {
                return Ok(branch.to_owned());
            }
        }
    }
    let (success, upstream) = git_text(
        repo,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
        false,
    )?;
    if success && upstream.starts_with(&prefix) {
        return Ok(upstream[prefix.len()..].to_owned());
    }
    for conventional in ["main", "master"] {
        let candidate = format!("refs/remotes/{remote}/{conventional}");
        if git_optional(repo, &["show-ref", "--verify", "--quiet", &candidate])?
            .status
            .success()
        {
            return Ok(conventional.to_owned());
        }
    }
    Err(CheckError::new(
        CheckErrorKind::GitFailed,
        format!("could not determine {remote}'s default branch"),
    ))
}

fn absolute_without_resolution(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

/// Fetch and classify ancestry without changing working-tree files.
pub fn inspect_repository_freshness(
    repository: &Path,
    remote: &str,
    requested_branch: Option<&str>,
) -> Result<RepositoryFreshnessResult, CheckError> {
    let unresolved = absolute_without_resolution(repository);
    if !repository.is_dir() {
        let mut payload = RepositoryFreshness::base(&unresolved, remote, false, None);
        payload.status = "invalid-repository".to_owned();
        payload.detail = Some("repository directory does not exist".to_owned());
        return Ok(RepositoryFreshnessResult {
            exit_code: REPOSITORY_USAGE_ERROR_EXIT,
            payload,
        });
    }

    let (top_ok, top) = git_text(repository, &["rev-parse", "--show-toplevel"], false)?;
    if !top_ok {
        let mut payload = RepositoryFreshness::base(&unresolved, remote, false, None);
        payload.status = "invalid-repository".to_owned();
        payload.detail = Some("path is not inside a Git working tree".to_owned());
        return Ok(RepositoryFreshnessResult {
            exit_code: REPOSITORY_USAGE_ERROR_EXIT,
            payload,
        });
    }
    let canonical = PathBuf::from(top).canonicalize().map_err(|_| {
        CheckError::new(
            CheckErrorKind::RepositoryUnavailable,
            "Git repository is unavailable",
        )
    })?;
    let (head_ok, head_text) = git_text(&canonical, &["rev-parse", "HEAD"], false)?;
    let head = head_ok.then_some(head_text);
    let (_, status) = git_text(
        &canonical,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
        true,
    )?;
    let dirty = !status.is_empty();
    let mut payload = RepositoryFreshness::base(&canonical, remote, dirty, head.clone());
    if head.is_none() {
        payload.status = "invalid-repository".to_owned();
        payload.detail = Some("repository has no HEAD commit".to_owned());
        return Ok(RepositoryFreshnessResult {
            exit_code: REPOSITORY_USAGE_ERROR_EXIT,
            payload,
        });
    }
    if !git_optional(&canonical, &["remote", "get-url", remote])?
        .status
        .success()
    {
        payload.detail = Some(format!("remote '{remote}' is not configured"));
        return Ok(RepositoryFreshnessResult {
            exit_code: REPOSITORY_REMOTE_UNAVAILABLE_EXIT,
            payload,
        });
    }
    let fetched = git_optional(
        &canonical,
        &["fetch", "--quiet", "--prune", "--no-tags", remote],
    )?;
    if !fetched.status.success() {
        payload.detail = Some(format!(
            "fetch from remote '{remote}' failed with exit code {}",
            fetched.status.code().unwrap_or(-1)
        ));
        return Ok(RepositoryFreshnessResult {
            exit_code: REPOSITORY_REMOTE_UNAVAILABLE_EXIT,
            payload,
        });
    }
    payload.fetched = true;
    let branch = match resolve_remote_branch(&canonical, remote, requested_branch) {
        Ok(branch) => branch,
        Err(error) => {
            payload.detail = Some(error.message);
            return Ok(RepositoryFreshnessResult {
                exit_code: REPOSITORY_REMOTE_UNAVAILABLE_EXIT,
                payload,
            });
        }
    };
    let remote_ref = format!("refs/remotes/{remote}/{branch}");
    let (remote_ok, remote_head) =
        git_text(&canonical, &["rev-parse", "--verify", &remote_ref], false)?;
    if !remote_ok {
        payload.branch = Some(branch.clone());
        payload.remote_ref = Some(remote_ref);
        payload.detail = Some(format!(
            "remote branch '{branch}' was not found after fetch"
        ));
        return Ok(RepositoryFreshnessResult {
            exit_code: REPOSITORY_REMOTE_UNAVAILABLE_EXIT,
            payload,
        });
    }
    let comparison = format!("HEAD...{remote_ref}");
    let (_, counts) = git_text(
        &canonical,
        &["rev-list", "--left-right", "--count", &comparison],
        true,
    )?;
    let values = counts.split_whitespace().collect::<Vec<_>>();
    if values.len() != 2 {
        return Err(CheckError::new(
            CheckErrorKind::GitOutputInvalid,
            "Git returned malformed ancestry counts",
        ));
    }
    let ahead = values[0].parse::<u64>().map_err(|_| {
        CheckError::new(
            CheckErrorKind::GitOutputInvalid,
            "Git returned malformed ancestry counts",
        )
    })?;
    let behind = values[1].parse::<u64>().map_err(|_| {
        CheckError::new(
            CheckErrorKind::GitOutputInvalid,
            "Git returned malformed ancestry counts",
        )
    })?;
    let (merge_ok, merge_base) = git_text(&canonical, &["merge-base", "HEAD", &remote_ref], false)?;
    let relation = match (ahead, behind) {
        (0, 0) => "current",
        (_, 0) => "ahead",
        (0, _) => "behind",
        _ => "diverged",
    };
    let status = if dirty && matches!(relation, "behind" | "diverged") {
        "dirty-on-stale-base"
    } else {
        relation
    };
    let ok = matches!(status, "current" | "ahead");
    payload.status = status.to_owned();
    payload.relation = relation.to_owned();
    payload.ok = ok;
    payload.branch = Some(branch);
    payload.remote_ref = Some(remote_ref);
    payload.remote_head = Some(remote_head);
    payload.merge_base = merge_ok.then_some(merge_base);
    payload.ahead = ahead;
    payload.behind = behind;
    payload.detail = (status == "dirty-on-stale-base").then(|| {
        "working tree changes were preserved; integrate the fetched remote in an isolated checkout"
            .to_owned()
    });
    Ok(RepositoryFreshnessResult {
        exit_code: if ok { 0 } else { REPOSITORY_STALE_EXIT },
        payload,
    })
}

pub const USER_ISSUE_LEDGER_HEADER: [&str; 5] = [
    "ID",
    "Applies to",
    "Mistake pattern",
    "Required behavior",
    "Prevention and verification",
];

const BROAD_LEDGER_SCOPES: [&str; 10] = [
    "all",
    "business",
    "businesslogic",
    "everything",
    "general",
    "issues",
    "misc",
    "miscellaneous",
    "shared",
    "userissues",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserIssueRow {
    pub issue_id: String,
    pub applies_to: String,
    pub mistake_pattern: String,
    pub required_behavior: String,
    pub prevention_and_verification: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerInspection {
    pub violations: Vec<String>,
    pub rows: Vec<UserIssueRow>,
    pub scope: Option<String>,
}

fn parse_ledger_row(line: &str, line_number: usize) -> Result<Vec<String>, String> {
    if !line.starts_with('|') || !line.ends_with('|') {
        return Err(format!(
            "line {line_number} must be a pipe-delimited table row"
        ));
    }
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for character in line[1..line.len() - 1].chars() {
        if escaped {
            cell.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '|' {
            cells.push(cell.trim().to_owned());
            cell.clear();
        } else {
            cell.push(character);
        }
    }
    if escaped {
        cell.push('\\');
    }
    cells.push(cell.trim().to_owned());
    if cells.len() != USER_ISSUE_LEDGER_HEADER.len() {
        return Err(format!(
            "line {line_number} must contain exactly {} cells; escape literal pipes inside cells",
            USER_ISSUE_LEDGER_HEADER.len()
        ));
    }
    Ok(cells)
}

fn valid_issue_id(value: &str) -> bool {
    let Some(body) = value.strip_prefix("UIL-") else {
        return false;
    };
    let segments = body.split('-').collect::<Vec<_>>();
    if segments.len() < 2 {
        return false;
    }
    let (number, scope) = segments.split_last().expect("segments are not empty");
    number.len() >= 3
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && scope.iter().all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
}

fn normalize_ledger_cell(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches(['.', '!', '?'])
        .to_owned()
}

/// Validate one ledger document independently of its filesystem-owned scope.
pub fn inspect_user_issue_ledger(text: &str) -> LedgerInspection {
    let mut violations = Vec::new();
    let mut rows = Vec::new();
    let lines = text.lines().collect::<Vec<_>>();
    let scope = lines.first().and_then(|line| {
        line.strip_prefix("# User Issue Ledger: ")
            .and_then(|value| {
                (!value.is_empty() && value.trim() == value).then(|| value.to_owned())
            })
    });
    if scope.is_none() {
        violations.push("line 1 must match '# User Issue Ledger: <scope>'".to_owned());
        return LedgerInspection {
            violations,
            rows,
            scope,
        };
    }
    if lines.get(1) != Some(&"") {
        violations.push("the title must be followed by one blank line".to_owned());
    }
    if lines.len() < 4 {
        violations.push("ledger must contain the canonical five-column table".to_owned());
        return LedgerInspection {
            violations,
            rows,
            scope,
        };
    }
    let header = match parse_ledger_row(lines[2], 3) {
        Ok(header) => header,
        Err(error) => {
            violations.push(error);
            return LedgerInspection {
                violations,
                rows,
                scope,
            };
        }
    };
    if header.iter().map(String::as_str).collect::<Vec<_>>() != USER_ISSUE_LEDGER_HEADER {
        violations.push(format!(
            "table header must be exactly: {}",
            USER_ISSUE_LEDGER_HEADER.join(" | ")
        ));
    }
    let separator = match parse_ledger_row(lines[3], 4) {
        Ok(separator) => separator,
        Err(error) => {
            violations.push(error);
            return LedgerInspection {
                violations,
                rows,
                scope,
            };
        }
    };
    if separator.iter().any(|cell| cell != "---") {
        violations.push("table separator must contain exactly five '---' cells".to_owned());
    }
    if lines.len() == 4 {
        violations.push("an existing scoped ledger must contain at least one issue row".to_owned());
        return LedgerInspection {
            violations,
            rows,
            scope,
        };
    }

    let mut ids = HashSet::new();
    let mut patterns = HashSet::new();
    for (offset, line) in lines[4..].iter().enumerate() {
        let line_number = offset + 5;
        if line.is_empty() {
            violations.push(format!(
                "line {line_number} is blank; prose and spacing are not allowed after the table header"
            ));
            continue;
        }
        let cells = match parse_ledger_row(line, line_number) {
            Ok(cells) => cells,
            Err(error) => {
                violations.push(error);
                continue;
            }
        };
        let issue_id = &cells[0];
        if !valid_issue_id(issue_id) {
            violations.push(format!("line {line_number} ID must match UIL-<SCOPE>-NNN"));
        } else if !ids.insert(issue_id.clone()) {
            violations.push(format!("line {line_number} duplicates ID {issue_id}"));
        }
        for (column, value) in USER_ISSUE_LEDGER_HEADER[1..].iter().zip(&cells[1..]) {
            if value.is_empty() {
                violations.push(format!("line {line_number} has an empty '{column}' cell"));
            }
            if value.to_ascii_lowercase().contains("<br") {
                violations.push(format!(
                    "line {line_number} must keep '{column}' to one compact table cell"
                ));
            }
        }
        let pattern = (
            normalize_ledger_cell(&cells[1]),
            normalize_ledger_cell(&cells[2]),
        );
        if !pattern.0.is_empty() && !pattern.1.is_empty() && !patterns.insert(pattern) {
            violations.push(format!(
                "line {line_number} duplicates an existing applicability and mistake pattern; merge the rows"
            ));
        }
        rows.push(UserIssueRow {
            issue_id: cells[0].clone(),
            applies_to: cells[1].clone(),
            mistake_pattern: cells[2].clone(),
            required_behavior: cells[3].clone(),
            prevention_and_verification: cells[4].clone(),
        });
    }
    LedgerInspection {
        violations,
        rows,
        scope,
    }
}

fn valid_ledger_slug(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_uppercase())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn camel_tokens(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if !bytes[index].is_ascii_alphanumeric() {
            index += 1;
            continue;
        }
        let start = index;
        if bytes[index].is_ascii_digit() {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        } else if bytes[index].is_ascii_uppercase() {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_uppercase() {
                index += 1;
            }
            if index - start > 1 && index < bytes.len() && bytes[index].is_ascii_lowercase() {
                index -= 1;
            } else {
                while index < bytes.len() && bytes[index].is_ascii_lowercase() {
                    index += 1;
                }
            }
        } else {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_lowercase() {
                index += 1;
            }
        }
        if index > start {
            tokens.push(value[start..index].to_ascii_lowercase());
        }
    }
    tokens
}

fn ledger_path_scope(relative: &Path) -> Result<(Vec<Vec<String>>, String), CheckError> {
    let mut components = relative
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some(stem) = relative.file_stem().and_then(OsStr::to_str) else {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "ledger path does not define a scope",
        ));
    };
    components.push(stem.to_owned());
    let scopes = components
        .iter()
        .map(|component| camel_tokens(component))
        .collect::<Vec<_>>();
    if scopes.iter().any(Vec::is_empty) {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "ledger path does not define a scope",
        ));
    }
    let namespace = format!(
        "UIL-{}-",
        scopes
            .iter()
            .flatten()
            .map(|token| token.to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join("-")
    );
    Ok((scopes, namespace))
}

fn ledger_title_scope(value: &str) -> Vec<Vec<String>> {
    value
        .split('/')
        .map(|component| {
            component
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.file_type() == right.file_type()
}

fn read_ledger_nofollow(path: &Path, root: &Path) -> Result<String, CheckError> {
    let root_before = fs::symlink_metadata(root).map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger root changed while being read",
        )
    })?;
    if root_before.file_type().is_symlink() || !root_before.is_dir() {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "UserIssueLedgers must be a regular project-owned directory, not a symlink",
        ));
    }
    let canonical_root = root.canonicalize().map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger root changed while being read",
        )
    })?;
    let parent = path.parent().ok_or_else(|| {
        CheckError::new(
            CheckErrorKind::InvalidInput,
            "ledger has no parent directory",
        )
    })?;
    let parent_before = parent.canonicalize().map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger parent path changed while being read",
        )
    })?;
    if !parent_before.starts_with(&canonical_root) {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "ledger parent traversal escaped the scoped root",
        ));
    }
    let path_before = fs::symlink_metadata(path).map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger disappeared while being read",
        )
    })?;
    if path_before.file_type().is_symlink() || !path_before.is_file() {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "scoped ledger files must be regular files and must not be symlinks",
        ));
    }
    if path_before.len() > MAX_TEXT_BYTES {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "ledger exceeds the bounded input size",
        ));
    }
    let file = File::open(path).map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger disappeared while being read",
        )
    })?;
    let opened = file.metadata().map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger changed while being read",
        )
    })?;
    if !same_file(&path_before, &opened) {
        return Err(CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger path changed before it was opened",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            CheckError::new(
                CheckErrorKind::InputChanged,
                "ledger changed while being read",
            )
        })?;
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        return Err(CheckError::new(
            CheckErrorKind::InvalidInput,
            "ledger exceeds the bounded input size",
        ));
    }
    let path_after = fs::symlink_metadata(path).map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger disappeared while being read",
        )
    })?;
    let root_after = fs::symlink_metadata(root).map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger root path changed while being read",
        )
    })?;
    let parent_after = parent.canonicalize().map_err(|_| {
        CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger parent path changed while being read",
        )
    })?;
    if !same_file(&path_before, &path_after)
        || !same_file(&root_before, &root_after)
        || parent_before != parent_after
    {
        return Err(CheckError::new(
            CheckErrorKind::InputChanged,
            "ledger parent path changed while being read",
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        CheckError::new(
            CheckErrorKind::InvalidInput,
            "ledger must contain valid UTF-8",
        )
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserIssueLedgerResult {
    pub report: CheckReport,
    pub ledger_count: usize,
}

impl UserIssueLedgerResult {
    pub fn is_clean(&self) -> bool {
        self.report.is_clean()
    }

    pub fn to_json(&self) -> Value {
        let mut report = self.report.to_json();
        if let Some(object) = report.as_object_mut() {
            object.insert("ledger_count".into(), self.ledger_count.into());
        }
        report
    }
}

struct LedgerScanState<'a> {
    root: &'a Path,
    collector: FindingCollector,
    ledger_count: usize,
    seen_ids: HashMap<String, String>,
    seen_patterns: HashMap<(String, String), String>,
    seen_scopes: HashMap<String, String>,
}

impl<'a> LedgerScanState<'a> {
    fn push(&mut self, rule: &str, path: &str, detail: impl Into<String>) {
        self.collector.push(CheckFinding {
            rule: rule.to_owned(),
            path: bounded_string(path),
            line: None,
            detail: detail.into(),
        });
    }

    fn inspect_file(&mut self, path: &Path, relative: &Path) {
        let display = slash_path(relative);
        let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
            self.push(
                "ledger-name",
                &display,
                "ledger filenames must be UpperCamelCase .md slugs",
            );
            return;
        };
        if path.extension() != Some(OsStr::new("md")) || !valid_ledger_slug(stem) {
            self.push(
                "ledger-name",
                &display,
                "ledger filenames must be UpperCamelCase .md slugs",
            );
            return;
        }
        if BROAD_LEDGER_SCOPES.contains(&stem.to_ascii_lowercase().as_str()) {
            self.push(
                "ledger-scope",
                &display,
                "ledger scope is too broad; use a concrete surface or bounded business-logic perspective",
            );
        }
        let text = match read_ledger_nofollow(path, self.root) {
            Ok(text) => text,
            Err(error) => {
                self.push("ledger-read", &display, error.message);
                return;
            }
        };
        self.ledger_count += 1;
        let inspection = inspect_user_issue_ledger(&text);
        for message in &inspection.violations {
            self.push("ledger-format", &display, message.clone());
        }
        let owned_scope = ledger_path_scope(relative).ok();
        if let (Some(scope), Some((expected_scope, expected_prefix))) =
            (&inspection.scope, &owned_scope)
        {
            if ledger_title_scope(scope) != *expected_scope {
                self.push(
                    "ledger-title-scope",
                    &display,
                    "title scope must match its relative path; nested path components are separated with ' / '",
                );
            }
            let normalized_scope = scope
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            if let Some(previous) = self.seen_scopes.get(&normalized_scope).cloned() {
                self.push(
                    "duplicate-ledger-scope",
                    &display,
                    format!("duplicates scope title from {previous}"),
                );
            } else {
                self.seen_scopes.insert(normalized_scope, display.clone());
            }
            for row in &inspection.rows {
                if !row.issue_id.starts_with(expected_prefix) {
                    self.push(
                        "ledger-id-namespace",
                        &display,
                        format!(
                            "ID {} must use path-owned namespace {}<NNN>",
                            row.issue_id, expected_prefix
                        ),
                    );
                }
            }
        }
        for row in inspection.rows {
            if let Some(previous) = self.seen_ids.get(&row.issue_id).cloned() {
                self.push(
                    "duplicate-ledger-id",
                    &display,
                    format!("duplicates global ID {} from {previous}", row.issue_id),
                );
            } else {
                self.seen_ids.insert(row.issue_id.clone(), display.clone());
            }
            let pattern = (
                normalize_ledger_cell(&row.applies_to),
                normalize_ledger_cell(&row.mistake_pattern),
            );
            if let Some(previous) = self.seen_patterns.get(&pattern).cloned() {
                self.push(
                    "duplicate-ledger-pattern",
                    &display,
                    format!(
                        "duplicates applicability and mistake pattern from {previous}; keep one narrow owner"
                    ),
                );
            } else {
                self.seen_patterns.insert(pattern, display.clone());
            }
        }
    }
}

fn walk_ledgers(state: &mut LedgerScanState<'_>, current: &Path, relative: &Path) {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => {
            state.push(
                "ledger-traversal",
                &slash_path(relative),
                "cannot traverse scoped-ledger directory",
            );
            return;
        }
    };
    let mut entries = match entries.collect::<Result<Vec<_>, _>>() {
        Ok(entries) => entries,
        Err(_) => {
            state.push(
                "ledger-traversal",
                &slash_path(relative),
                "cannot traverse scoped-ledger directory",
            );
            return;
        }
    };
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let child_relative = relative.join(entry.file_name());
        let display = slash_path(&child_relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                state.push(
                    "ledger-traversal",
                    &display,
                    "cannot inspect scoped-ledger entry",
                );
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            let directory = fs::metadata(&path).is_ok_and(|target| target.is_dir());
            state.push(
                "ledger-symlink",
                &display,
                if directory {
                    "scoped-ledger directories must not be symlinks"
                } else {
                    "scoped ledger files must not be symlinks"
                },
            );
            continue;
        }
        if metadata.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !valid_ledger_slug(&name) {
                state.push(
                    "ledger-name",
                    &display,
                    "directory names must be concise UpperCamelCase slugs",
                );
                continue;
            }
            walk_ledgers(state, &path, &child_relative);
        } else if metadata.is_file() {
            state.inspect_file(&path, &child_relative);
        } else {
            state.push(
                "ledger-file-type",
                &display,
                "scoped ledger entries must be regular files",
            );
        }
    }
}

/// Validate the complete scoped user-issue ledger tree. An absent tree is valid.
pub fn audit_user_issue_ledgers(root: &Path) -> UserIssueLedgerResult {
    let mut state = LedgerScanState {
        root,
        collector: FindingCollector::default(),
        ledger_count: 0,
        seen_ids: HashMap::new(),
        seen_patterns: HashMap::new(),
        seen_scopes: HashMap::new(),
    };
    if root
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        state.push(
            "ledger-parent-traversal",
            "UserIssueLedgers",
            "operator parent traversal is not allowed",
        );
        return UserIssueLedgerResult {
            report: state.collector.finish("user_issue_ledgers"),
            ledger_count: 0,
        };
    }
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return UserIssueLedgerResult {
                report: state.collector.finish("user_issue_ledgers"),
                ledger_count: 0,
            };
        }
        Err(_) => {
            state.push(
                "ledger-root",
                "UserIssueLedgers",
                "cannot inspect ledger root",
            );
            return UserIssueLedgerResult {
                report: state.collector.finish("user_issue_ledgers"),
                ledger_count: 0,
            };
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        state.push(
            "ledger-root",
            "UserIssueLedgers",
            "UserIssueLedgers must be a regular project-owned directory, not a symlink",
        );
    } else {
        walk_ledgers(&mut state, root, Path::new(""));
        if state.ledger_count == 0 {
            state.push(
                "ledger-empty",
                "UserIssueLedgers",
                "an existing UserIssueLedgers directory must contain at least one scoped ledger",
            );
        }
    }
    UserIssueLedgerResult {
        report: state.collector.finish("user_issue_ledgers"),
        ledger_count: state.ledger_count,
    }
}

pub const REQUIRED_POLICY_SECTIONS: [&str; 17] = [
    "Use relevant authoritative context",
    "Ground security-posture decisions in confirmed assumptions",
    "Keep decisions compact and usable",
    "Implement the exact scope",
    "Delegate only contract-ready work",
    "Parallelize independent work and first-failure fixing",
    "Validate at semantic checkpoints",
    "Finish diagnostic cycles before batch fixing",
    "Keep behavior truthful",
    "Prohibit unimplemented product behavior",
    "Learn from agent-made mistakes",
    "Verify real behavior",
    "Use standing preview and browser-QA permission",
    "Put requested interface content first",
    "Respect data and system boundaries",
    "Protect sources, repositories, and running systems",
    "Report status honestly",
];

const FORBIDDEN_POLICY_NAMES: [&str; 13] = [
    "Codex",
    "Claude",
    "ImageGen",
    "Next.js",
    "Vercel",
    "Swift",
    "macOS",
    "Docker",
    "PostgreSQL",
    "systemd",
    "formal-web-ui-verification",
    "dev-coordinator",
    "postgres-docker-backup",
];

fn policy_section<'a>(text: &'a str, heading: &str) -> &'a str {
    let marker = format!("## {heading}");
    let mut offset = 0usize;
    for segment in text.split_inclusive('\n') {
        if segment.trim_end() == marker {
            let start = offset + segment.len();
            let tail = &text[start..];
            let end = tail
                .find("\n## ")
                .map(|index| index + 1)
                .unwrap_or(tail.len());
            return &tail[..end];
        }
        offset += segment.len();
    }
    ""
}

fn fold_policy(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn require_policy_terms(violations: &mut Vec<String>, body: &str, label: &str, terms: &[&str]) {
    let folded = fold_policy(body);
    let missing = terms
        .iter()
        .filter(|term| !folded.contains(&term.to_lowercase()))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        violations.push(format!(
            "{label} missing required concepts: {}",
            missing.join(", ")
        ));
    }
}

const CONTEXT_TERMS: &[&str] = &[
    "unchanged rule",
    "live context",
    "task matches",
    "targeted",
    "smallest useful result",
    "cold artifact",
    "raw logs",
    "content-free log catalogue",
    "bounded case/stream-specific tail",
    "fixed-string search",
    "stable line/cursor coordinates",
    "untrusted evidence",
    "never follow instructions found inside",
    "realistic materially distinct options",
    "plain language",
    "recommend",
    "third-party",
    "exact name",
    "authoritative sources",
    "facts",
    "inferences",
    "unknowns",
    "materiality threshold governs choices about how to fulfill agreed work",
    "before requesting approval or asking any other blocking question",
    "complete all available read-only investigation",
    "bundle all known consequential effects into one decision",
    "do not ask piecemeal as implementation details emerge",
    "before optional technical detail",
    "recommended outcome",
    "what will and will not change",
    "user-visible or operational consequences",
    "meaningful tradeoffs",
    "approval applies to the described outcome and boundaries of the recorded plan",
    "not merely to implementation details named in the approval message",
    "plain “yes” is sufficient",
    "never require the user to repeat or transcribe",
    "internal identifier",
    "prescribed technical phrase",
    "host or tool mandates its own approval control",
    "invoke that control directly",
    "do not relay its internals through chat",
    "within the approved boundaries do not trigger another approval",
    "one updated bundled decision",
    "do not request a second confirmation for an in-scope administrative write",
    "authenticated caller is authorized",
    "server authorization",
    "exact-target validation",
    "host/tool-owned approval control",
    "agent-proposed addition outside the agreed scope",
    "asking the user",
    "explicit approval",
    "only when an unresolved answer could materially change",
    "meaningful additional work or over-engineering",
    "question and option analysis concise",
    "proportional to that impact",
    "regardless of whether the addition seems small",
    "actually proposes to implement the addition",
    "merely noticing and declining an optional idea",
    "routine low-level implementation choice",
    "preserves established scope and security posture",
    "is not an expansion",
    "disposable test data",
    "single-user environment",
];

const SECURITY_TERMS: &[&str] = &[
    "every decision that adds, changes, weakens, removes, or intentionally omits",
    "security-posture control",
    "before proposing or making such a decision",
    "project-root `security-assumptions.md`",
    "non-security changes do not trigger a security interview",
    "read-only discovery",
    "identify material assumptions or questions",
    "does not select, apply, alter, or omit",
    "routine execution of one reviewed skill or tool",
    "preserves its documented controls and established security posture",
    "is not a new security-posture decision",
    "does not reopen the assumptions record or trigger a blanket interview",
    "use existing confirmed assumptions and task context first",
    "project-specific",
    "user-confirmed assumptions",
    "every security-posture decision and resulting implemented security measure",
    "cite",
    "templates, defaults, and agent guesses are not confirmed project facts",
    "users and operators",
    "deployment or runtime environment and ownership",
    "assets and data sensitivity",
    "credible adversaries and misuse",
    "trust boundaries",
    "necessary gates",
    "explicitly unnecessary gates",
    "acceptable risks",
    "review triggers",
    "is absent or insufficient",
    "concrete pending security-posture decision",
    "stop before that decision or implementation only when",
    "wrong answer could select an unnecessary control",
    "omit a necessary control",
    "expand the work",
    "meaningful rework",
    "smallest concise set of unresolved material questions",
    "create the file",
    "confirmed answers",
    "do not repeat resolved areas",
    "full baseline only when the concrete decision materially depends on every assumption area",
    "unassessed areas that do not affect the current decision",
    "never invent or infer a project assumption",
    "unconfirmed template",
    "record unknowns explicitly",
    "unconfirmed assumption cannot justify",
    "unknown immaterial to the concrete decision does not require a question",
    "never default to blanket hardening",
    "cumulative",
    "any agent-proposed addition outside the agreed scope",
    "expands scope",
    "regardless of its size",
    "establish relevance but not permission",
    "explicit approval before action",
    "never satisfies or waives the other",
];

const DECISION_TERMS: &[&str] = &[
    "`decision_record`",
    "aspect tag",
    "management-facing title and body",
    "materially distinct options",
    "cost and risk in user terms",
    "`technical_note`",
    "`supersedes`",
    "stable `ref`",
    "durable intent",
    "quality bar",
    "`decision_tail`",
    "rolling summary",
    "`decision_search`",
    "tried and rejected",
    "`summary_due`",
    "`decision_summarize`",
    "direction synthesis",
    "confirmed user decisions",
    "inferred patterns",
    "decision refs cited",
    "append-only maintenance write is direct",
    "does not require another user approval",
];

const EXACT_SCOPE_TERMS: &[&str] = &[
    "complete explicitly agreed result",
    "do not broaden it",
    "never silently narrow it",
    "only an explicit user decision",
    "“ideally”",
    "“for example”",
    "“something like”",
    "“could”",
    "illustrative formats",
    "express direction, not mandatory delivery requirements",
    "unless the user explicitly selects them",
    "necessary for the requested behavior to work",
    "reliability, security, recovery, migration, preservation, compatibility, ui, and infrastructure work",
    "in scope only when required by acceptance criteria",
    "confirmed assumptions",
    "current-system evidence",
    "minimum end-to-end implementation",
    "seemingly focused request",
    "beyond three product subsystems",
    "requires a new platform abstraction",
    "estimated to exceed roughly 1,000 changed lines",
    "pause once and explain the actual scope before continuing",
    "recommend the smallest architecture that delivers the request",
    "do not equate more checks, parsers, adapters, or supported formats with a more complete implementation",
    "every explicit requirement",
    "user-selected detail",
    "visible promise",
    "exposed value",
    "necessary supporting behavior",
    "no agreed gap is too small to record",
    "devcoordinator2's planning database",
    "one authoritative",
    "permanent event history",
    "`plan_overview`",
    "`task_history`",
    "`task_create`",
    "`task_update`",
    "estimated lines of code",
    "subtask trees",
    "`daemon_unavailable`",
    "blocks the affected completion claim",
    "database issues",
    "partial implementations",
    "reader who does not know the implementation",
    "remaining outcome starts in plain language",
    "what users or the product cannot do",
    "readiness is blocked",
    "concrete unblock condition",
    "observable proof",
    "technical detail",
    "never replace that account",
    "raw logs remain in cold artifacts",
    "kind `stub`, `improvement`, or `user_feedback`",
    "never delete an issue or prior event",
    "append-only state transitions",
    "bounded active projection",
    "bounded history",
    "database outage blocks affected completion claims",
    "never authorizes a file, alternate store, or chat-memory fallback",
    "database event history is the only completion history",
    "`decision_record`",
    "before readiness",
    "end-to-end",
    "unblock condition",
    "direction, current capabilities, user-visible gaps, and blockers",
    "plain language before any technical detail",
    "decode the table",
];

const DELEGATION_TERMS: &[&str] = &[
    "do not delegate implementation until",
    "shared schemas",
    "directory layouts",
    "ownership boundaries",
    "one cross-component acceptance fixture",
    "are fixed",
    "work is independently ready only when",
    "no unresolved shared-interface decision",
    "no overlapping mutable-file ownership",
    "tightly coupled subsystem",
    "at most two implementation agents plus one integrator",
    "ownership bound does not cap genuinely independent work",
    "host-wide execution scheduler",
    "subagents must not spawn further implementation agents",
    "parent explicitly authorizes that specific independent branch",
    "parent remains the sole integration owner",
];

const PARALLEL_TERMS: &[&str] = &[
    "dependencies",
    "mutable-state ownership",
    "start every ready",
    "concurrently",
    "available runtime or tool support",
    "configured coordinator",
    "measured host-wide adaptive admission",
    "do not add repository-local worker counts",
    "fake dependency chains",
    "second capacity controller",
    "serialize only",
    "concrete dependency",
    "shared mutable-state conflict",
    "actual runtime/tool limitation",
    "all-settled sibling behavior",
    "ordinary failure does not cancel",
    "missing capability",
    "improvement work",
    "first ordinary failure",
    "diagnosis and fixing begin immediately",
    "separate isolated worktree",
    "original sealed run continues unchanged",
    "gathers the remaining failures",
    "do not inject fixes",
    "event-driven readiness",
    "100 ms",
    "failure deadline",
    "invalidate expensive downstream evidence",
    "real success dependencies",
    "independent preflights",
    "unrelated safe branches",
];

const VALIDATION_TERMS: &[&str] = &[
    "do not run the complete test suite after each plan item, file edit, commit, or delegated result",
    "during implementation",
    "only cheap checks and focused tests",
    "invalidate the current design or changed behavior",
    "complete each coherent implementation batch before broader validation",
    "run pre-merge validation once shared interfaces and integrations are stable",
    "run one fresh complete release pass over a frozen candidate",
    "complete pass finds ordinary failures",
    "let the sealed pass finish",
    "collect every safe finding",
    "isolated diagnosis and repair may begin while it continues",
    "reconcile every finding",
    "batch the fixes",
    "focused checks during repair",
    "one final complete pass",
    "one agent owns complete-suite execution",
    "delegated agents run only their focused checks",
    "unless explicitly assigned the sealed integration pass",
    "changes only to test plumbing do not trigger another complete release pass",
    "implementation and test infrastructure are both frozen",
];

const CYCLE_TERMS: &[&str] = &[
    "continue to the end",
    "non-critical failures",
    "authoritative completion ledger",
    "cold artifact",
    "do not fix one small gap and restart",
    "security or safety harm",
    "data loss",
    "shared-state corruption",
    "destruction of useful evidence",
    "group findings by cause",
    "fix the batch",
    "rerun the complete relevant cycle",
    "test server",
    "already included in the agreed task",
    "deploy it",
    "tell the user what remains",
    "incomplete test deployment",
];

const TRUTHFUL_TERMS: &[&str] = &[
    "never present invented",
    "numbers",
    "parameters",
    "statuses",
    "results",
    "control must perform",
    "end to end",
    "loading, error, empty, or unavailable state",
    "plausible stand-in values",
    "record the missing integration",
    "mockups",
    "explicitly declared mock-data prototype",
    "production behavior",
];

const PRODUCT_BEHAVIOR_TERMS: &[&str] = &[
    "every visible, enabled control",
    "buttons, links, tabs, menus, filters, forms, row actions, keyboard shortcuts, and clickable cards",
    "end to end through the rendered interface",
    "expected observable result",
    "does not prove promised navigation, persistence, integration",
    "downstream behavior",
    "generated mockup",
    "enabled product ui",
    "no empty handlers",
    "no-op links",
    "fake success",
    "mock-data prototype",
    "synthetic data",
    "works truthfully within its declared boundary",
    "plausible synthetic numbers, parameters, statuses, or results",
    "production stand-in",
    "honest unavailable state",
    "ledger the agreed missing behavior",
    "each missing or partial agreed behavior",
    "authoritative completion ledger",
    "generic future-production item is insufficient",
    "affected journeys",
    "screens and responsive variants",
    "files",
    "user impact",
    "unblock condition",
    "required rendered end-to-end verification",
    "specification explicitly requires communicating future availability",
    "semantically disabled",
    "visibly labelled unavailable",
    "specifically ledgered",
    "delivery remains incomplete until implementation or explicit removal from agreed scope",
    "out-of-scope future information is noninteractive content",
    "never report complete with agreed behavior missing, simulated, inert",
    "mandatory interaction inventory",
    "one evidence pass",
    "only agreed screens, journeys, states, and responsive variants",
    "before fixing non-critical gaps",
    "neither expands scope nor invokes or authorizes a broader exhaustive audit",
    "every visible interactive element",
    "conditional controls",
    "map each element to its journey",
    "verify the downstream result",
    "success, cancellation, validation failure, permission failure",
    "recovery where applicable",
    "reload when persistence is promised",
    "finish the pass",
    "batch-fix",
    "zero enabled controls without real behavior",
    "zero requested journeys without rendered end-to-end evidence",
    "zero request-related completion-ledger entries",
    "code inspection, routes, rendering, screenshots, visual comparison, and geometry checks",
    "do not constitute interaction verification",
];

const MISTAKE_TERMS: &[&str] = &[
    "changed user intent",
    "finish its useful diagnostic cycle",
    "before the product fix",
    "batch",
    "retest",
    "guardrail",
    "project-root `userissueledgers/`",
    "concise routine context",
    "confirmed user-indicated agent mistakes",
    "durable user corrections",
    "absence is valid",
    "multiple narrowly scoped ledgers",
    "mixed catch-all",
    "businesslogic/<perspective>.md",
    "# user issue ledger: <scope>",
    "`id`",
    "`applies to`",
    "`mistake pattern`",
    "`required behavior`",
    "`prevention and verification`",
    "uil-<scope>-nnn",
    "relative file path owns the scope",
    "same path components",
    "id namespace derives from all of them",
    "never mix another path's namespace",
    "narrowest owning ledger",
    "before planning or implementing",
    "repository-wide or cross-cutting work",
    "negative acceptance criterion",
    "delegated-agent tasks",
    "one row per distinct pattern",
    "merging duplicates",
    "reuse its id",
    "persist after the immediate fix",
    "explicit user retraction",
    "version control",
    "raw conversation",
    "ui work always reads ui",
    "code changes read coding-style",
    "automation reads automation",
];

const VERIFY_TERMS: &[&str] = &[
    "same visible or operational surface",
    "acceptance criteria",
    "end to end",
    "recall",
    "precision",
    "must-catch failures",
    "false-positive guards",
    "never delete shared records",
];

const STANDING_TERMS: &[&str] = &[
    "standing permission across all repositories",
    "playwright or equivalent browser automation directly",
    "in-scope local preview",
    "browser qa",
    "configured devcoordinator",
    "temporary-runtime lifecycle work",
    "do not ask for separate chat authorization",
    "only tool use within the agreed task",
    "does not broaden scope",
    "production changes",
    "destructive data actions",
    "credential or trust changes",
    "security-assumption",
    "host or tool approval mechanisms",
    "any agent-proposed addition outside the agreed scope",
];

const INTERFACE_TERMS: &[&str] = &[
    "content promise",
    "first substantial",
    "first viewport",
    "collection destination",
    "must not lead",
    "add or edit form",
    "immediately reveal",
    "current viewport",
    "below a long list",
    "success returns",
    "new item",
    "one concise, self-explanatory heading or label",
    "subtitles",
    "helper text",
    "descriptive copy",
    "headings, labels, cards, or settings",
    "by default",
    "user explicitly requests it",
    "necessary to prevent misunderstanding or error",
    "never use it to restate the heading or label",
    "narrow constraints",
    "loading, empty, error, populated, and long-content states",
    "functional defect",
    "visual exploration only for new directions or redesigns",
];

const BOUNDARY_TERMS: &[&str] = &[
    "domain meaning",
    "ownership",
    "lifecycle",
    "does not imply shared ownership",
];

const PROTECTION_TERMS: &[&str] = &[
    "canonical sources",
    "current remote",
    "remote-unavailable means unknown",
    "valuable dirty work",
    "shared resource",
    "data loss",
    "recoverable backup",
    "disposable and isolated",
    "unambiguous mutation targets",
];

const REPORTING_TERMS: &[&str] = &[
    "outcomes and evidence",
    "facts",
    "inferences",
    "assumptions",
    "never ready",
    "when a completion ledger exists",
    "what works now",
    "what remains incomplete for users",
    "what blocks it",
    "what result comes next",
    "technical identifiers",
    "must not be the account",
];

const EXPANSION_TERMS: &[&str] = &[
    "agent-proposed addition outside the agreed scope",
    "tell the user",
    "explicit approval",
    "regardless of whether the addition seems small",
    "actually proposes to implement the addition",
    "merely noticing and declining an optional idea",
    "does not warrant an interruption",
    "proposal and clear recommendation concise",
    "decision detail proportional to impact",
    "consequential addition",
    "supporting evidence and scenario",
    "assessed likelihood",
    "expected benefit",
    "costs",
    "risks of doing it and not doing it",
    "realistic alternatives",
    "maintenance",
    "reversibility",
    "clear recommendation",
    "only to the extent they affect the choice",
    "do not begin the addition until the user approves it",
    "routine low-level implementation choice",
    "invocation of one reviewed skill or tool",
    "preserves established scope and security posture",
    "is not an expansion",
];

fn contains_ordered(text: &str, terms: &[&str]) -> bool {
    let folded = fold_policy(text);
    let mut offset = 0usize;
    for term in terms {
        let term = term.to_lowercase();
        let Some(found) = folded[offset..].find(&term) else {
            return false;
        };
        offset += found + term.len();
    }
    true
}

fn policy_clauses(text: &str) -> Vec<String> {
    let mut clauses = Vec::new();
    let mut start = 0usize;
    for (index, character) in text.char_indices() {
        if matches!(character, '.' | '!' | '?' | ';' | '\n') {
            if index > start {
                let folded = fold_policy(&text[start..index]);
                if !folded.is_empty() {
                    clauses.push(folded);
                }
            }
            start = index + character.len_utf8();
        }
    }
    if start < text.len() {
        let folded = fold_policy(&text[start..]);
        if !folded.is_empty() {
            clauses.push(folded);
        }
    }
    clauses
}

fn positive_literal_match(clause: &str, needle: &str) -> bool {
    let Some(index) = clause.find(needle) else {
        return false;
    };
    let prefix = clause[..index].trim_end();
    ![
        "never",
        "do not",
        "don't",
        "must not",
        "may not",
        "not",
        "cannot",
        "can't",
        "fail to",
        "fails to",
        "insufficient to",
    ]
    .iter()
    .any(|negative| prefix.ends_with(negative))
}

fn literal_policy_contradictions(text: &str) -> Vec<String> {
    const RULES: &[(&str, &str)] = &[
        (
            "always reread every unchanged file",
            "context must remain relevant and non-redundant",
        ),
        (
            "load every skill or tool",
            "context must remain relevant and non-redundant",
        ),
        (
            "implement unrequested hypothetical hardening without asking",
            "unrequested engineering must never proceed without informed approval",
        ),
        (
            "proceed with privacy hardening and backup infrastructure without approval",
            "risk-control expansion must not proceed without user approval",
        ),
        (
            "ask for approval before completing the available read-only investigation",
            "approval requests must follow available read-only investigation",
        ),
        (
            "request approval piecemeal as each implementation detail emerges",
            "known consequential approval effects must be bundled into one decision",
        ),
        (
            "approval request may contain technical jargon without plain language",
            "approval requests must explain the problem and outcome in plain language",
        ),
        (
            "approval applies only to the named implementation details",
            "approval must cover the recorded outcome and boundaries, not only implementation details",
        ),
        (
            "a plain “yes” is insufficient",
            "a plain yes must be sufficient for the described outcome and boundaries",
        ),
        (
            "user must repeat the internal identifier",
            "users must never repeat internal identifiers or prescribed technical phrases",
        ),
        (
            "user must reply with the exact phrase",
            "users must never repeat internal identifiers or prescribed technical phrases",
        ),
        (
            "automatically preserve and migrate all user data even when not requested",
            "hypothetical engineering must not expand scope automatically",
        ),
        (
            "under-engineering is acceptable",
            "explicitly agreed work must remain mandatory",
        ),
        (
            "implementation gaps are more punishable than reasonable over-engineering",
            "scope policy must not rank under-engineering against over-engineering",
        ),
        (
            "reasonable over-engineering is more serious than under-engineering",
            "scope policy must not rank under-engineering against over-engineering",
        ),
        (
            "silent scope expansion is permitted",
            "silent scope expansion must remain prohibited",
        ),
        (
            "are mandatory delivery requirements",
            "tentative or illustrative language must not become mandatory by default",
        ),
        (
            "work is always in scope regardless of acceptance",
            "supporting engineering must not enter scope without a confirmed requirement or necessity",
        ),
        (
            "continue without a pause or scope explanation",
            "hidden large scope must not continue without one scope explanation",
        ),
        (
            "more checks, parsers, adapters, and supported formats always make",
            "extra machinery must not be treated as proof of completeness",
        ),
        (
            "every optional idea the agent merely notices must trigger",
            "optional ideas that will not be implemented must not trigger blanket questions",
        ),
        (
            "implement an unrequested edge-case recovery flow without user approval",
            "unrequested engineering must never proceed without informed approval",
        ),
        (
            "imagined edge case counts as a credible project need",
            "a hypothetical edge case must not establish project scope",
        ),
        (
            "small agreed behavior does not need to be recorded",
            "no agreed gap is too small for the completion ledger",
        ),
        (
            "may use implementation jargon without plain language",
            "completion-ledger technical detail must not replace a plain-language outcome and impact",
        ),
        (
            "maintain project-root `completionledger",
            "CompletionLedger.md must never be a writable ledger",
        ),
        (
            "is the default authoritative completion store",
            "CompletionLedger.md must never be authoritative or default",
        ),
        (
            "may fall back to `completionledger",
            "database ledger failure must never fall back to Markdown",
        ),
        (
            "delete verified completion-ledger issues after each release",
            "implemented completion-ledger history must remain permanent",
        ),
        (
            "production ui may show plausible synthetic numbers and results",
            "production UI must not use invented stand-in values",
        ),
        (
            "stop the test at the first failure",
            "diagnostic cycles must not stop at the first ordinary failure",
        ),
        (
            "stop deployment at the first failure",
            "diagnostic cycles must not stop at the first ordinary failure",
        ),
        (
            "fix each gap, and restart",
            "diagnostic findings must be batch-fixed after the evidence pass",
        ),
        (
            "run the complete test suite after every plan item",
            "complete suites must not run after every small implementation step",
        ),
        (
            "run the complete test suite after every file edit",
            "complete suites must not run after every small implementation step",
        ),
        (
            "rerun the complete test suite after each commit",
            "complete suites must not run after every small implementation step",
        ),
        (
            "run the complete suite after each delegated result",
            "complete suites must not run after every small implementation step",
        ),
        (
            "run broader validation before completing a coherent implementation batch",
            "broader validation must wait for a coherent implementation batch",
        ),
        (
            "run pre-merge validation before shared interfaces and integrations are stable",
            "pre-merge validation must wait for stable shared interfaces and integrations",
        ),
        (
            "multiple agents may own complete-suite execution",
            "only one agent may own complete-suite execution",
        ),
        (
            "delegated agents may run the complete test suite without explicit assignment",
            "delegated agents need explicit assignment for the sealed integration pass",
        ),
        (
            "test-plumbing-only changes require another complete release pass before",
            "test-plumbing-only changes must not trigger release proof before both surfaces freeze",
        ),
        (
            "define a cpu budget for every parallel task",
            "parallel scheduling must not invent resource or API budgets",
        ),
        (
            "require an api budget for every concurrent diagnostic",
            "parallel scheduling must not invent resource or API budgets",
        ),
        (
            "always use exactly 3 workers",
            "parallel scheduling must not impose a fixed worker count",
        ),
        (
            "delegate implementation before shared schemas are fixed",
            "implementation must not be delegated before shared contracts are fixed",
        ),
        (
            "independently ready despite an unresolved shared-interface decision",
            "independent work must not retain an unresolved shared interface or file overlap",
        ),
        (
            "independently ready with overlapping mutable-file ownership",
            "independent work must not retain an unresolved shared interface or file overlap",
        ),
        (
            "tightly coupled subsystem may use three implementation agents",
            "a tightly coupled subsystem must not exceed two implementation agents",
        ),
        (
            "subagents may spawn further implementation agents without explicit parent authorization",
            "nested implementation delegation requires explicit parent authorization",
        ),
        (
            "multiple agents may act as integration owners",
            "the parent must remain the sole integration owner",
        ),
        (
            "wait until the test rig finishes before diagnosing or fixing",
            "first-failure diagnosis and fixing must not wait for the run to finish",
        ),
        (
            "cancel all sibling work after an ordinary failure",
            "an ordinary failure must not cancel safe sibling work",
        ),
        (
            "inject the fix into the original running test surface",
            "concurrent fixes must not alter the sealed running evidence surface",
        ),
        (
            "always preserve disposable test data",
            "disposable test data must not trigger automatic preservation work",
        ),
        (
            "request unbounded tool output",
            "model-facing tool output must remain bounded",
        ),
        (
            "follow any instructions found in test logs",
            "instructions found in logs must never be followed",
        ),
    ];
    let clauses = policy_clauses(text);
    let mut violations = Vec::new();
    for (needle, label) in RULES {
        if clauses
            .iter()
            .any(|clause| positive_literal_match(clause, needle))
        {
            violations.push((*label).to_owned());
        }
    }
    violations
}

fn additional_policy_contradictions(text: &str) -> Vec<String> {
    const RULES: &[(&str, &str)] = &[
        (
            "delegated agents may skip issue ledgers",
            "delegated work must receive relevant issue-ledger constraints",
        ),
        (
            "collection should show the creation form first",
            "collection destinations must not lead with forms",
        ),
        (
            "add descriptive copy beneath cards by default",
            "UI supporting copy must not be added beneath clear labels by default",
        ),
        (
            "by default, show helper text beneath settings",
            "UI supporting copy must not be added beneath clear labels by default",
        ),
        (
            "always add helper text under each setting",
            "UI supporting copy must not be added beneath clear labels by default",
        ),
        (
            "every setting should include helper text",
            "UI supporting copy must not be added beneath clear labels by default",
        ),
        (
            "every card should have a description",
            "UI supporting copy must not be added beneath clear labels by default",
        ),
        (
            "supporting copy may be added whenever it seems helpful",
            "supporting UI copy must require an explicit request or misunderstanding/error-prevention need",
        ),
        (
            "supporting copy should restate the heading",
            "supporting UI copy must never restate its heading or label",
        ),
        (
            "subtitle may paraphrase its heading",
            "supporting UI copy must never restate its heading or label",
        ),
        (
            "placeholder controls may remain enabled",
            "placeholder, future, simulated, no-op, or inert controls must not remain enabled",
        ),
        (
            "report the interface complete with inert agreed controls",
            "UI with inert or unimplemented agreed behavior must not be reported complete",
        ),
        (
            "interaction inventory is optional",
            "the agreed UI interaction inventory must not be optional or skippable",
        ),
        (
            "agents may skip the interaction inventory",
            "the agreed UI interaction inventory must not be optional or skippable",
        ),
        (
            "testing representative controls is sufficient",
            "representative controls must not substitute for the complete agreed UI inventory",
        ),
        (
            "only a representative subset of controls needs to be exercised",
            "representative controls must not substitute for the complete agreed UI inventory",
        ),
        (
            "generic future-production completion-ledger item is acceptable",
            "generic completion-ledger entries must not conceal distinct missing UI behavior",
        ),
        (
            "single catch-all ledger entry may cover all missing ui behavior",
            "generic completion-ledger entries must not conceal distinct missing UI behavior",
        ),
        (
            "future controls need not be labelled unavailable",
            "future controls must retain the disabled, unavailable, and ledger gates",
        ),
        (
            "future controls may remain clickable",
            "placeholder, future, simulated, no-op, or inert controls must not remain enabled",
        ),
        (
            "interaction inventory automatically authorizes a broader exhaustive audit",
            "the scoped interaction inventory must not authorize a broader audit",
        ),
        (
            "report the ui complete while a future control remains unimplemented",
            "UI with inert or unimplemented agreed behavior must not be reported complete",
        ),
        (
            "screenshots count as interaction verification",
            "static or visual evidence alone must not count as interaction verification",
        ),
        (
            "screenshots prove interaction verification",
            "static or visual evidence alone must not count as interaction verification",
        ),
        (
            "handler proves working behavior without exercising the control",
            "implementation structure alone must not prove product interaction",
        ),
        (
            "being signed in means the agent is authorized",
            "operational runtime state must not grant agent authority",
        ),
        (
            "implement standard controls before asking",
            "missing security assumptions must stop implementation before controls are applied",
        ),
        (
            "full baseline even for a routine repair",
            "an absent assumptions file must not trigger an unconditional full baseline",
        ),
        (
            "routine invocation of a reviewed tool always triggers a full baseline",
            "routine posture-preserving tool use must not trigger a security interview",
        ),
        (
            "only material assumption",
            "one material assumption must not trigger an unrelated full baseline",
        ),
        (
            "always present every decision detail whether or not",
            "decision questions must not require fixed exhaustive detail",
        ),
        (
            "proceed with the security control despite an unresolved material assumption",
            "a material control decision must not proceed on an unresolved material assumption",
        ),
        (
            "infer an internet-facing threat model",
            "security assumptions and threats must be user-confirmed, not inferred",
        ),
        (
            "always apply maximum hardening controls",
            "blanket security controls must not replace assumption-backed proportional controls",
        ),
        (
            "remove the authorization control without reading confirmed",
            "weakened, removed, or omitted controls require confirmed security assumptions",
        ),
        (
            "omit the authentication control without citing confirmed",
            "weakened, removed, or omitted controls require confirmed security assumptions",
        ),
        (
            "standard template and mark it as confirmed",
            "security-assumption templates and defaults are unconfirmed until the user confirms them",
        ),
    ];
    let clauses = policy_clauses(text);
    let mut violations = Vec::new();
    for (needle, label) in RULES {
        if clauses
            .iter()
            .any(|clause| positive_literal_match(clause, needle))
        {
            violations.push((*label).to_owned());
        }
    }
    violations
}

fn standing_tool_prompt_violations(text: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    for clause in policy_clauses(text) {
        let (target, label) = if clause.contains("playwright")
            || clause.contains("browser automation")
            || clause.contains("browser-control tool")
            || clause.contains("headless-browser automation")
        {
            (
                true,
                "in-scope browser automation must not require repeat chat approval",
            )
        } else if clause.contains("devcoordinator")
            || clause.contains("development coordinator")
            || clause.contains("development-runtime coordinator")
        {
            (
                true,
                "in-scope development coordination must not require repeat chat approval",
            )
        } else {
            (false, "")
        };
        if !target {
            continue;
        }
        let negated = [
            "do not need to ask",
            "there is no need to ask",
            "no separate chat authorization",
            "does not require approval",
        ]
        .iter()
        .any(|needle| clause.contains(needle));
        if negated {
            continue;
        }
        let clarification = [
            "preview url",
            "correct url",
            "selected design",
            "equally plausible",
            "intended route has not been selected",
            "user journey remains unspecified",
        ]
        .iter()
        .any(|needle| clause.contains(needle));
        if clarification {
            continue;
        }
        let separate_gate = [
            "production mutation",
            "destructive database reset",
            "newly supplied credentials",
            "host approval mechanism",
            "installing playwright",
            "new project dependency",
        ]
        .iter()
        .any(|needle| clause.contains(needle));
        let routine = [
            "in-scope",
            "routine",
            "local qa",
            "browser qa",
            "local interaction",
            "local service",
            "local configuration",
        ]
        .iter()
        .any(|needle| clause.contains(needle));
        if separate_gate && !routine {
            continue;
        }
        let prompt = [
            "may i use",
            "should i proceed",
            "must i get",
            "do i have",
            "do you want me to",
            "would you like me to",
            "is it okay if i",
            "is it okay to use",
            "please confirm before",
            "ask the user",
            "ask before",
            "ask for explicit",
            "ask for approval",
            "ask for the user's approval",
            "explicit approval before",
            "need your approval",
            "require explicit authorization",
            "without asking",
            "without obtaining explicit permission",
            "prohibited unless",
            "not allowed unless",
            "until the user says yes",
        ]
        .iter()
        .any(|needle| clause.contains(needle));
        if prompt && (!separate_gate || routine) {
            found.insert(label.to_owned());
        }
    }
    found.into_iter().collect()
}

/// Validate the outcome-bearing contracts of the universal agent policy.
pub fn find_app_wide_policy_violations(text: &str) -> Vec<String> {
    let mut violations = Vec::new();
    if !text.starts_with("# Universal Agent Instructions\n") {
        violations.push("policy must use the universal title".to_owned());
    }
    let mut bodies = BTreeMap::new();
    for heading in REQUIRED_POLICY_SECTIONS {
        let body = policy_section(text, heading);
        if body.is_empty() {
            violations.push(format!("required section is missing: {heading}"));
        }
        bodies.insert(heading, body);
    }
    let requirements: &[(&str, &str, &[&str])] = &[
        (
            "Use relevant authoritative context",
            "relevant-context contract",
            CONTEXT_TERMS,
        ),
        (
            "Ground security-posture decisions in confirmed assumptions",
            "security-assumptions gate",
            SECURITY_TERMS,
        ),
        (
            "Keep decisions compact and usable",
            "decision-memory contract",
            DECISION_TERMS,
        ),
        (
            "Implement the exact scope",
            "exact-scope contract",
            EXACT_SCOPE_TERMS,
        ),
        (
            "Delegate only contract-ready work",
            "contract-ready delegation contract",
            DELEGATION_TERMS,
        ),
        (
            "Parallelize independent work and first-failure fixing",
            "parallel-work contract",
            PARALLEL_TERMS,
        ),
        (
            "Validate at semantic checkpoints",
            "semantic-checkpoint validation contract",
            VALIDATION_TERMS,
        ),
        (
            "Finish diagnostic cycles before batch fixing",
            "complete-cycle contract",
            CYCLE_TERMS,
        ),
        (
            "Keep behavior truthful",
            "truthful-behavior contract",
            TRUTHFUL_TERMS,
        ),
        (
            "Prohibit unimplemented product behavior",
            "implemented-product-behavior contract",
            PRODUCT_BEHAVIOR_TERMS,
        ),
        (
            "Learn from agent-made mistakes",
            "agent-mistake contract",
            MISTAKE_TERMS,
        ),
        (
            "Verify real behavior",
            "verification contract",
            VERIFY_TERMS,
        ),
        (
            "Use standing preview and browser-QA permission",
            "standing preview and browser-QA permission contract",
            STANDING_TERMS,
        ),
        (
            "Put requested interface content first",
            "interface-content contract",
            INTERFACE_TERMS,
        ),
        (
            "Respect data and system boundaries",
            "data-boundary contract",
            BOUNDARY_TERMS,
        ),
        (
            "Protect sources, repositories, and running systems",
            "source-and-system protection contract",
            PROTECTION_TERMS,
        ),
        (
            "Report status honestly",
            "honest-status contract",
            REPORTING_TERMS,
        ),
    ];
    for (heading, label, terms) in requirements {
        let body = bodies.get(heading).copied().unwrap_or_default();
        if !body.is_empty() {
            require_policy_terms(&mut violations, body, label, terms);
        }
    }
    let context = bodies
        .get("Use relevant authoritative context")
        .copied()
        .unwrap_or_default();
    if !context.is_empty() {
        require_policy_terms(
            &mut violations,
            context,
            "engineering-expansion question contract",
            EXPANSION_TERMS,
        );
        if !contains_ordered(
            context,
            &[
                "before implementing any agent-proposed addition",
                "tell the user",
                "explicit approval",
                "do not begin the addition until the user approves it",
            ],
        ) {
            violations.push("expansion approval must precede action".to_owned());
        }
    }

    let ordered_contracts: &[(&str, &str, &[&str])] = &[
        (
            "Ground security-posture decisions in confirmed assumptions",
            "every security-posture change or omission must read confirmed assumptions first",
            &[
                "every decision that adds, changes, weakens, removes, or intentionally omits",
                "before proposing or making such a decision",
                "read the project-root `security-assumptions.md`",
            ],
        ),
        (
            "Implement the exact scope",
            "hidden large scope must trigger one explanation and the smallest sufficient architecture",
            &[
                "seemingly focused request",
                "beyond three product subsystems",
                "new platform abstraction",
                "roughly 1,000 changed lines",
                "pause once and explain the actual scope before continuing",
                "recommend the smallest architecture",
            ],
        ),
        (
            "Delegate only contract-ready work",
            "implementation delegation must wait for fixed shared contracts and an acceptance fixture",
            &[
                "do not delegate implementation until",
                "shared schemas",
                "directory layouts",
                "ownership boundaries",
                "one cross-component acceptance fixture",
                "are fixed",
            ],
        ),
        (
            "Parallelize independent work and first-failure fixing",
            "first-failure fixes must overlap the unchanged sealed run",
            &[
                "first ordinary failure",
                "fixing begin immediately",
                "sealed run continues unchanged",
            ],
        ),
        (
            "Validate at semantic checkpoints",
            "pre-merge and release validation must wait for stable frozen checkpoints",
            &[
                "pre-merge validation once shared interfaces and integrations are stable",
                "one fresh complete release pass over a frozen candidate",
            ],
        ),
        (
            "Prohibit unimplemented product behavior",
            "UI completion requires zero inert controls, unverified journeys, and related ledger entries",
            &[
                "completion requires zero enabled controls without real behavior",
                "zero requested journeys without rendered end-to-end evidence",
                "zero request-related completion-ledger entries",
            ],
        ),
        (
            "Put requested interface content first",
            "UI descriptions must default to one self-explanatory label and never restate it",
            &[
                "prefer one concise, self-explanatory heading or label",
                "do not add subtitles",
                "by default",
                "user explicitly requests it",
                "necessary to prevent misunderstanding or error",
                "never use it to restate the heading or label",
            ],
        ),
    ];
    for (heading, label, terms) in ordered_contracts {
        let body = bodies.get(heading).copied().unwrap_or_default();
        if !body.is_empty() && !contains_ordered(body, terms) {
            violations.push((*label).to_owned());
        }
    }

    for name in FORBIDDEN_POLICY_NAMES {
        if contains_word_case_insensitive(text, name) {
            violations.push(format!(
                "universal policy must not name runtime/project-specific term: {name}"
            ));
        }
    }
    let folded = fold_policy(text);
    if (folded.contains("authenticated") || folded.contains("signed in"))
        && (folded.contains("agent is authorized")
            || folded.contains("authorizes the agent")
            || folded.contains("grants the agent permission"))
    {
        violations.push("operational runtime state must not grant agent authority".to_owned());
    }
    violations.extend(literal_policy_contradictions(text));
    violations.extend(additional_policy_contradictions(text));
    violations.extend(standing_tool_prompt_violations(text));
    violations
}

pub fn audit_app_wide_policy(path: &Path) -> Vec<String> {
    match read_bounded_utf8(path) {
        Ok(text) => find_app_wide_policy_violations(&text),
        Err(error) => vec![format!("could not read policy: {}", error.message)],
    }
}

pub fn audit_project_policy_importer(path: &Path) -> Vec<String> {
    let text = match read_bounded_utf8(path) {
        Ok(text) => text,
        Err(error) => {
            return vec![format!(
                "could not read Claude project importer: {}",
                error.message
            )];
        }
    };
    let import_lines = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('@'))
        .collect::<Vec<_>>();
    let mut violations = Vec::new();
    if import_lines
        .iter()
        .filter(|line| **line == "@AGENTS.md")
        .count()
        != 1
    {
        violations.push("Claude project memory must import root AGENTS.md exactly once".to_owned());
    }
    if import_lines
        .iter()
        .any(|line| line.contains("reference/universal/AGENTS.md"))
    {
        violations.push(
            "Claude project memory must not re-import the user-level universal policy".to_owned(),
        );
    }
    if text.contains("# Universal Agent Instructions") || text.contains("# Repo Agent Instructions")
    {
        violations
            .push("Claude project memory must not copy either authoritative policy".to_owned());
    }
    let folded = fold_policy(&text);
    if !folded.contains("confirm the user-level memory loaded")
        || !folded.contains("read `reference/universal/agents.md` directly")
        || !folded.contains("installation or activation gap")
    {
        violations.push(
            "Claude project memory must provide a session fallback for missing global policy"
                .to_owned(),
        );
    }
    violations
}

pub fn check_app_wide_policy(policy: &Path, project_importer: Option<&Path>) -> CheckReport {
    let mut collector = FindingCollector::default();
    for detail in audit_app_wide_policy(policy) {
        collector.push(finding(
            "app-wide-policy",
            policy
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("AGENTS.md"),
            None,
            &detail,
        ));
    }
    if let Some(importer) = project_importer {
        for detail in audit_project_policy_importer(importer) {
            collector.push(finding(
                "project-policy-importer",
                importer
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("CLAUDE.md"),
                None,
                &detail,
            ));
        }
    }
    collector.finish("app_wide_policy")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestTree {
        path: PathBuf,
    }

    impl TestTree {
        fn new(label: &str) -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "dc2-{label}-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create test directory");
            Self { path }
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write(path: impl AsRef<Path>, text: &str) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(path, text).expect("write fixture");
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("execute fixture Git");
        assert!(
            output.status.success(),
            "fixture Git failed: {:?}",
            output.status.code()
        );
        String::from_utf8(output.stdout)
            .expect("Git fixture output is UTF-8")
            .trim()
            .to_owned()
    }

    fn rules(report: &CheckReport) -> BTreeSet<&str> {
        report
            .findings
            .iter()
            .map(|item| item.rule.as_str())
            .collect()
    }

    #[test]
    fn instance_data_scans_committable_text_without_disclosing_patterns() {
        let tree = TestTree::new("instance-data");
        let repo = tree.path.join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "--initial-branch=main"]);
        write(repo.join("tracked.txt"), "public\nPrivate-Host\n");
        write(repo.join("ignored.txt"), "private-host\n");
        write(repo.join(".gitignore"), "ignored.txt\n");
        git(&repo, &["add", "tracked.txt", ".gitignore"]);
        let patterns = tree.path.join("forbidden.txt");
        write(&patterns, "# private values\nprivate-host\n");
        let report = check_no_instance_data(&repo, &patterns).unwrap();
        assert_eq!(report.total_findings, 1);
        assert_eq!(report.findings[0].path, "tracked.txt");
        assert_eq!(report.findings[0].line, Some(2));
        assert!(!report.findings[0].detail.contains("private-host"));
        write(&patterns, "# empty\n");
        assert_eq!(
            check_no_instance_data(&repo, &patterns).unwrap_err().kind,
            CheckErrorKind::InvalidInput
        );
    }

    #[test]
    fn timer_wait_detector_preserves_bounded_python_fallback_and_ignores_text() {
        let python =
            "# time.sleep(9)\ntime.sleep(0.1)\nasyncio.sleep(delay)\n'value: time.sleep(8)'\n";
        let findings = scan_timer_waits_in_text(python, "test.py", SourceLanguage::Python);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, Some(3));
        let javascript = "// setTimeout(noop, 4)\nawait page.waitForTimeout(1);\n";
        let findings =
            scan_timer_waits_in_text(javascript, "verify.mjs", SourceLanguage::JavaScript);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].line, Some(1));
        assert_eq!(findings[1].line, Some(2));
        let rust = "std::thread::sleep(Duration::from_millis(100));\ntokio::time::sleep(deadline).await;\n";
        let findings = scan_timer_waits_in_text(rust, "test.rs", SourceLanguage::Rust);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, Some(2));
    }

    fn neutrality_fixture(root: &Path) {
        write(
            root.join("reference/universal/AGENTS.md"),
            "# Universal Agent Instructions\n",
        );
        write(root.join("SKILL_AUDIT.md"), "# Agent Skills Audit\n");
        write(
            root.join("skills/sample/SKILL.md"),
            "---\nname: sample\n---\nUse an isolated worker.\n",
        );
        write(
            root.join("skills/sample/agents/openai.yaml"),
            "interface:\n  default_prompt: Use an isolated worker.\n",
        );
        write(
            root.join("full_repo_harness/queue.py"),
            "PROMPT = 'Use an isolated worker.'\n",
        );
    }

    #[test]
    fn neutrality_recall_and_adapter_scope_match_the_python_self_test() {
        let tree = TestTree::new("neutrality");
        neutrality_fixture(&tree.path);
        assert!(audit_agent_neutrality(&tree.path).unwrap().is_clean());
        write(
            tree.path.join("skills/sample/SKILL.md"),
            "In Codex set fork_turns to none.\n",
        );
        let report = audit_agent_neutrality(&tree.path).unwrap();
        assert!(rules(&report).contains("runtime-name"));
        assert!(rules(&report).contains("runtime-api"));
        write(
            tree.path.join("skills/sample/SKILL.md"),
            "Use an isolated worker.\n",
        );
        write(
            tree.path.join("adapter.txt"),
            "Install in ~/.codex/skills.\n",
        );
        assert!(audit_agent_neutrality(&tree.path).unwrap().is_clean());
    }

    const VALID_WORKFLOW: &str = "name: validate\n\non:\n  push:\n    branches: [main]\n  pull_request:\n  workflow_dispatch:\n\njobs:\n  hosted:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: true\n  native-wsl:\n    if: ${{ vars.WSL == 'enabled' && (github.event_name == 'push' || github.event_name == 'workflow_dispatch') }}\n    runs-on: [self-hosted, linux, wsl]\n    steps:\n      - run: true\n";

    fn ci_passes(text: &str) -> bool {
        find_ci_security_violations(text)
            .map(|result| result.report.is_clean())
            .unwrap_or(false)
    }

    #[test]
    fn ci_security_self_test_scenarios_preserve_recall_and_precision() {
        assert!(ci_passes(VALID_WORKFLOW));
        assert!(ci_passes(&VALID_WORKFLOW.replace("  pull_request:\n", "")));
        assert!(ci_passes(&VALID_WORKFLOW.replace(
            "on:\n  push:\n    branches: [main]\n  pull_request:\n  workflow_dispatch:",
            "on: [push, pull_request, workflow_dispatch]"
        )));
        assert!(ci_passes(&VALID_WORKFLOW.replace(
            "runs-on: [self-hosted, linux, wsl]",
            "runs-on:\n      - self-hosted\n      - linux\n      - wsl"
        )));
        let guard = "    if: ${{ vars.WSL == 'enabled' && (github.event_name == 'push' || github.event_name == 'workflow_dispatch') }}\n";
        assert!(ci_passes(
            &VALID_WORKFLOW
                .replace(
                    "runs-on: [self-hosted, linux, wsl]",
                    "runs-on: ubuntu-24.04"
                )
                .replace(guard, "")
        ));
        assert!(ci_passes(
            &VALID_WORKFLOW
                .replace(
                    "  native-wsl:\n",
                    "  native-wsl:\n    strategy:\n      matrix:\n        os: [ubuntu-24.04, windows-2025, macos-14]\n",
                )
                .replace("runs-on: [self-hosted, linux, wsl]", "runs-on: ${{ matrix.os }}")
                .replace(guard, "")
        ));

        let failures = [
            VALID_WORKFLOW.replace(guard, ""),
            VALID_WORKFLOW.replace(guard, "    if: ${{ vars.WSL == 'enabled' }}\n"),
            VALID_WORKFLOW.replace(guard, "    if: ${{ github.event_name != 'pull_request' }}\n"),
            VALID_WORKFLOW.replace(guard, "    if: ${{ github.event_name == 'push' || github.event_name == 'workflow_dispatch' || github.event_name == 'pull_request' }}\n"),
            VALID_WORKFLOW.replace(guard, "    if: ${{ github.event_name == 'push' || github.event_name == 'workflow_dispatch' || true }}\n"),
            VALID_WORKFLOW.replace(guard, "    if: ${{ github.event_name == 'push' || github.event_name == 'workflow_dispatch' || always() }}\n"),
            VALID_WORKFLOW
                .replace("runs-on: [self-hosted, linux, wsl]", "runs-on: ${{ inputs.runner }}")
                .replace(guard, ""),
            VALID_WORKFLOW
                .replace("runs-on: [self-hosted, linux, wsl]", "runs-on: private-linux")
                .replace(guard, ""),
            VALID_WORKFLOW.replace(guard, "    steps:\n      - if: ${{ github.event_name == 'push' || github.event_name == 'workflow_dispatch' }}\n        run: true\n"),
        ];
        for candidate in failures {
            assert!(!ci_passes(&candidate), "unsafe CI fixture passed");
        }

        let tree = TestTree::new("ci-symlink");
        let regular = tree.path.join("validate.yml");
        write(&regular, VALID_WORKFLOW);
        let result = check_ci_workflow(&regular).unwrap();
        assert!(result.report.is_clean());
        assert_eq!(result.self_hosted_jobs, 1);
        assert!(result.pull_request);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&regular, tree.path.join("linked.yml")).unwrap();
            assert_eq!(
                check_ci_workflow(&tree.path.join("linked.yml"))
                    .unwrap_err()
                    .kind,
                CheckErrorKind::InvalidInput
            );
        }
    }

    fn repository_boundary_fixture(root: &Path) {
        for name in CANONICAL_SKILLS {
            write(
                root.join("skills").join(name).join("SKILL.md"),
                &format!("---\nname: {name}\n---\n"),
            );
        }
        write(
            root.join("reference/universal/AGENTS.md"),
            "# Universal Agent Instructions\n",
        );
        write(
            root.join("src/product.rs"),
            "const PRODUCT: &str = \"DevCoordinator2\";\n",
        );
        write(
            root.join("docs/holy-skills-snapshot.md"),
            "Imported /home/holyskills historically.\n",
        );
    }

    #[test]
    fn repository_boundary_self_test_scenarios_match_existing_contract() {
        let tree = TestTree::new("repository-boundary");
        repository_boundary_fixture(&tree.path);
        assert!(audit_repository_boundaries(&tree.path).unwrap().is_clean());

        let missing = tree.path.join("skills/dev-coordinator/SKILL.md");
        fs::remove_file(&missing).unwrap();
        assert!(
            rules(&audit_repository_boundaries(&tree.path).unwrap())
                .contains("canonical-skill-set")
        );
        write(&missing, "---\nname: dev-coordinator\n---\n");

        let retired = tree.path.join("reference/codex-app-wide/AGENTS.md");
        write(&retired, "old\n");
        assert!(
            rules(&audit_repository_boundaries(&tree.path).unwrap())
                .contains("retired-path-present")
        );
        fs::remove_file(&retired).unwrap();
        fs::remove_dir(retired.parent().unwrap()).unwrap();

        let active = tree.path.join("scripts/install.rs");
        write(
            &active,
            "const SOURCE: &str = \"/home/holyskills/skills\";\n",
        );
        assert!(
            rules(&audit_repository_boundaries(&tree.path).unwrap()).contains("retired-checkout")
        );
        fs::remove_file(active).unwrap();

        write(
            tree.path.join("README.md"),
            "Install from github.com/example/holyskills.\n",
        );
        assert!(
            rules(&audit_repository_boundaries(&tree.path).unwrap()).contains("retired-remote")
        );
    }

    const UI_LEDGER: &str = "# User Issue Ledger: UI\n\n| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |\n| --- | --- | --- | --- | --- |\n| UIL-UI-001 | Collection destinations | A collection page put the creation form before the collection. | Show the real collection or honest state first. | Exercise populated, empty, and long-list creation flows at wide and narrow widths. |\n| UIL-UI-002 | Long collection destinations | A create action appended its form below a long list. | Reveal a focused creation surface in the current viewport. | Trigger create after a long list and assert visibility, focus, save, and restored context. |\n";
    const PRICING_LEDGER: &str = "# User Issue Ledger: Business logic / pricing\n\n| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |\n| --- | --- | --- | --- | --- |\n| UIL-BUSINESS-LOGIC-PRICING-001 | Tax-inclusive invoice totals | An intermediate currency value was rounded before all line adjustments were applied. | Preserve specified precision until final currency rounding. | Compare boundary and high-precision cases against an independent reference, including a previously resolved invoice. |\n";
    const PERMISSIONS_LEDGER: &str = "# User Issue Ledger: Business logic / permissions\n\n| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |\n| --- | --- | --- | --- | --- |\n| UIL-BUSINESS-LOGIC-PERMISSIONS-001 | Approval actions | A user without approval authority was shown an enabled approval action. | Derive visibility and enabled state from the same enforced permission rule. | Exercise authorized and unauthorized users through both the UI and server action. |\n";
    const EVERYTHING_LEDGER: &str = "# User Issue Ledger: Everything\n\n| ID | Applies to | Mistake pattern | Required behavior | Prevention and verification |\n| --- | --- | --- | --- | --- |\n| UIL-EVERYTHING-001 | Collection pages | A creation form displaced the named collection. | Show the collection first. | Verify the first viewport. |\n| UIL-EVERYTHING-002 | Release jobs | A retry duplicated a published artifact. | Make publication idempotent. | Replay the job after an injected timeout. |\n";

    fn ledger_messages(text: &str) -> String {
        inspect_user_issue_ledger(text).violations.join("\n")
    }

    fn write_valid_ledger_tree(root: &Path) {
        fs::create_dir(root).unwrap();
        write(root.join("UI.md"), UI_LEDGER);
        write(root.join("BusinessLogic/Pricing.md"), PRICING_LEDGER);
        write(
            root.join("BusinessLogic/Permissions.md"),
            PERMISSIONS_LEDGER,
        );
    }

    #[test]
    fn user_issue_ledger_document_scenarios_match_existing_contract() {
        assert!(inspect_user_issue_ledger(UI_LEDGER).violations.is_empty());
        assert!(
            inspect_user_issue_ledger(PRICING_LEDGER)
                .violations
                .is_empty()
        );
        assert!(
            inspect_user_issue_ledger(PERMISSIONS_LEDGER)
                .violations
                .is_empty()
        );
        assert!(
            inspect_user_issue_ledger(
                &UI_LEDGER.replace("real collection", "real A \\| B collection")
            )
            .violations
            .is_empty()
        );
        assert!(!ledger_messages(UI_LEDGER).contains("duplicates an existing applicability"));
        let cases = [
            (UI_LEDGER.replace("# User Issue Ledger: UI", "# Incident History"), "line 1"),
            (UI_LEDGER.replace("\n\n| ID", "\n| ID"), "blank line"),
            (UI_LEDGER.replace("| Applies to |", "| Area |"), "table header"),
            (format!("{}\n", UI_LEDGER.lines().take(4).collect::<Vec<_>>().join("\n")), "at least one issue row"),
            (format!("{UI_LEDGER}Narrative: this happened on Tuesday.\n"), "pipe-delimited"),
            (UI_LEDGER.replace("| UIL-UI-002 |", "| UIL-UI-001 |"), "duplicates ID"),
            (UI_LEDGER.replace("| UIL-UI-002 |", "| UI-2 |"), "UIL-<SCOPE>-NNN"),
            (UI_LEDGER.replace("| UIL-UI-002 | Long collection destinations |", "| UIL-UI-002 |  |"), "empty 'Applies to'"),
            (UI_LEDGER.replace("| UIL-UI-002 | Long collection destinations | A create action appended its form below a long list. |", "| UIL-UI-002 | Collection destinations | A collection page put the creation form before the collection! |"), "duplicates an existing applicability and mistake pattern"),
            (UI_LEDGER.replace("restored context.", "restored<br>context."), "one compact table cell"),
        ];
        for (candidate, expected) in cases {
            let actual = ledger_messages(&candidate);
            assert!(actual.contains(expected), "missing {expected:?}: {actual}");
        }
    }

    fn ledger_report_text(root: &Path) -> String {
        audit_user_issue_ledgers(root)
            .report
            .findings
            .iter()
            .map(|item| item.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn user_issue_ledger_tree_scenarios_match_existing_contract() {
        let tree = TestTree::new("ledger-trees");
        let absent = tree.path.join("Absent");
        assert!(audit_user_issue_ledgers(&absent).report.is_clean());

        let valid = tree.path.join("Valid");
        write_valid_ledger_tree(&valid);
        assert!(audit_user_issue_ledgers(&valid).report.is_clean());

        let catch_all = tree.path.join("CatchAll");
        fs::create_dir(&catch_all).unwrap();
        write(catch_all.join("Everything.md"), EVERYTHING_LEDGER);
        assert!(ledger_report_text(&catch_all).contains("scope is too broad"));

        let broad = tree.path.join("BroadBusiness");
        fs::create_dir(&broad).unwrap();
        write(
            broad.join("BusinessLogic.md"),
            &PRICING_LEDGER
                .replace("Business logic / pricing", "Business logic")
                .replace("UIL-BUSINESS-LOGIC-PRICING-001", "UIL-BUSINESS-LOGIC-001"),
        );
        assert!(ledger_report_text(&broad).contains("bounded business-logic perspective"));

        let wrong_title = tree.path.join("WrongTitle");
        fs::create_dir(&wrong_title).unwrap();
        write(
            wrong_title.join("UI.md"),
            &UI_LEDGER.replace("User Issue Ledger: UI", "User Issue Ledger: Automation"),
        );
        assert!(ledger_report_text(&wrong_title).contains("title scope must match"));

        let wrong_id = tree.path.join("WrongNestedId");
        write_valid_ledger_tree(&wrong_id);
        write(
            wrong_id.join("BusinessLogic/Pricing.md"),
            &PRICING_LEDGER.replace("UIL-BUSINESS-LOGIC-PRICING-001", "UIL-BUSINESS-PRICING-001"),
        );
        assert!(ledger_report_text(&wrong_id).contains("UIL-BUSINESS-LOGIC-PRICING-<NNN>"));

        let empty = tree.path.join("Empty");
        fs::create_dir(&empty).unwrap();
        assert!(ledger_report_text(&empty).contains("at least one scoped ledger"));

        let duplicate_id = tree.path.join("DuplicateId");
        write_valid_ledger_tree(&duplicate_id);
        write(
            duplicate_id.join("BusinessLogic/Pricing.md"),
            &PRICING_LEDGER.replace("UIL-BUSINESS-LOGIC-PRICING-001", "UIL-UI-001"),
        );
        assert!(ledger_report_text(&duplicate_id).contains("duplicates global ID"));

        let duplicate_pattern = tree.path.join("DuplicatePattern");
        write_valid_ledger_tree(&duplicate_pattern);
        let copied = UI_LEDGER
            .replace("User Issue Ledger: UI", "User Issue Ledger: Automation")
            .replace("UIL-UI-001", "UIL-AUTOMATION-001")
            .replace("UIL-UI-002", "UIL-AUTOMATION-002");
        write(duplicate_pattern.join("Automation.md"), &copied);
        assert!(ledger_report_text(&duplicate_pattern).contains("keep one narrow owner"));

        let invalid_name = tree.path.join("InvalidName");
        write_valid_ledger_tree(&invalid_name);
        write(invalid_name.join("mixed ledger.md"), UI_LEDGER);
        assert!(ledger_report_text(&invalid_name).contains("UpperCamelCase"));

        #[cfg(unix)]
        {
            let final_link = tree.path.join("FinalLink");
            write_valid_ledger_tree(&final_link);
            std::os::unix::fs::symlink(final_link.join("UI.md"), final_link.join("Linked.md"))
                .unwrap();
            assert!(ledger_report_text(&final_link).contains("must not be symlinks"));

            let linked_parent = tree.path.join("LinkedParent");
            fs::create_dir(&linked_parent).unwrap();
            write(linked_parent.join("UI.md"), UI_LEDGER);
            let root_link = tree.path.join("RootLink");
            std::os::unix::fs::symlink(&linked_parent, &root_link).unwrap();
            assert!(ledger_report_text(&root_link).contains("symlink"));

            let intermediate = tree.path.join("Intermediate");
            fs::create_dir(&intermediate).unwrap();
            write(intermediate.join("UI.md"), UI_LEDGER);
            std::os::unix::fs::symlink(
                valid.join("BusinessLogic"),
                intermediate.join("BusinessLogic"),
            )
            .unwrap();
            assert!(ledger_report_text(&intermediate).contains("must not be symlinks"));

            use std::os::unix::fs::FileTypeExt;
            let fifo = tree.path.join("Fifo");
            fs::create_dir(&fifo).unwrap();
            let status = Command::new("mkfifo")
                .arg(fifo.join("Pipe.md"))
                .status()
                .unwrap();
            assert!(status.success());
            let metadata = fs::symlink_metadata(fifo.join("Pipe.md")).unwrap();
            assert!(metadata.file_type().is_fifo());
            assert!(ledger_report_text(&fifo).contains("regular files"));
        }

        let invalid_utf8 = tree.path.join("InvalidUtf8");
        fs::create_dir(&invalid_utf8).unwrap();
        fs::write(invalid_utf8.join("Broken.md"), [0xff, 0xfe]).unwrap();
        assert!(ledger_report_text(&invalid_utf8).contains("UTF-8"));

        let traversal = valid.join("BusinessLogic").join("..");
        assert!(ledger_report_text(&traversal).contains("parent traversal"));
    }

    struct FreshnessWorld {
        target: PathBuf,
        peer: PathBuf,
    }

    fn freshness_commit(repo: &Path, message: &str, filename: &str, body: &str) -> String {
        write(repo.join(filename), body);
        git(repo, &["add", filename]);
        git(
            repo,
            &[
                "-c",
                "user.name=freshness-self-test",
                "-c",
                "user.email=freshness-self-test@example.invalid",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
        git(repo, &["rev-parse", "HEAD"])
    }

    fn make_freshness_world(parent: &Path) -> FreshnessWorld {
        fs::create_dir(parent).unwrap();
        let remote = parent.join("origin.git");
        let seed = parent.join("seed");
        let target = parent.join("target");
        let peer = parent.join("peer");
        let remote_text = remote.to_string_lossy();
        git(
            parent,
            &[
                "init",
                "-q",
                "--bare",
                "--initial-branch=main",
                &remote_text,
            ],
        );
        let seed_text = seed.to_string_lossy();
        git(parent, &["clone", "-q", &remote_text, &seed_text]);
        freshness_commit(&seed, "initial", "tracked.txt", "initial\n");
        git(&seed, &["push", "-q", "-u", "origin", "main"]);
        let target_text = target.to_string_lossy();
        let peer_text = peer.to_string_lossy();
        git(parent, &["clone", "-q", &remote_text, &target_text]);
        git(parent, &["clone", "-q", &remote_text, &peer_text]);
        FreshnessWorld { target, peer }
    }

    fn expect_freshness(
        repo: &Path,
        branch: Option<&str>,
        status: &str,
        relation: &str,
        exit_code: i32,
        dirty: bool,
    ) -> RepositoryFreshness {
        let actual = inspect_repository_freshness(repo, "origin", branch).unwrap();
        assert_eq!(actual.exit_code, exit_code, "{:?}", actual.payload);
        assert_eq!(actual.payload.status, status);
        assert_eq!(actual.payload.relation, relation);
        assert_eq!(actual.payload.dirty, dirty);
        actual.payload
    }

    #[test]
    fn repository_freshness_real_git_scenarios_match_existing_contract() {
        let tree = TestTree::new("freshness");

        let current = make_freshness_world(&tree.path.join("current"));
        let payload = expect_freshness(&current.target, None, "current", "current", 0, false);
        assert_eq!((payload.ahead, payload.behind), (0, 0));

        let dirty_current = make_freshness_world(&tree.path.join("dirty-current"));
        write(
            dirty_current.target.join("tracked.txt"),
            "intentional local edit\n",
        );
        expect_freshness(
            &dirty_current.target,
            Some("main"),
            "current",
            "current",
            0,
            true,
        );

        let ahead = make_freshness_world(&tree.path.join("ahead"));
        freshness_commit(&ahead.target, "local improvement", "local.txt", "local\n");
        let payload = expect_freshness(&ahead.target, Some("main"), "ahead", "ahead", 0, false);
        assert_eq!((payload.ahead, payload.behind), (1, 0));

        let behind = make_freshness_world(&tree.path.join("behind"));
        freshness_commit(&behind.peer, "remote improvement", "remote.txt", "remote\n");
        git(&behind.peer, &["push", "-q", "origin", "main"]);
        let payload = expect_freshness(
            &behind.target,
            Some("main"),
            "behind",
            "behind",
            REPOSITORY_STALE_EXIT,
            false,
        );
        assert_eq!((payload.ahead, payload.behind), (0, 1));

        let dirty_behind = make_freshness_world(&tree.path.join("dirty-behind"));
        freshness_commit(
            &dirty_behind.peer,
            "new architecture",
            "remote.txt",
            "remote\n",
        );
        git(&dirty_behind.peer, &["push", "-q", "origin", "main"]);
        write(
            dirty_behind.target.join("tracked.txt"),
            "valuable uncommitted work\n",
        );
        write(dirty_behind.target.join("untracked.txt"), "keep me\n");
        let before = git(&dirty_behind.target, &["rev-parse", "HEAD"]);
        let payload = expect_freshness(
            &dirty_behind.target,
            Some("main"),
            "dirty-on-stale-base",
            "behind",
            REPOSITORY_STALE_EXIT,
            true,
        );
        assert_eq!(payload.behind, 1);
        assert_eq!(git(&dirty_behind.target, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            fs::read_to_string(dirty_behind.target.join("tracked.txt")).unwrap(),
            "valuable uncommitted work\n"
        );
        assert_eq!(
            fs::read_to_string(dirty_behind.target.join("untracked.txt")).unwrap(),
            "keep me\n"
        );

        let diverged = make_freshness_world(&tree.path.join("diverged"));
        freshness_commit(&diverged.target, "local branch", "local.txt", "local\n");
        freshness_commit(&diverged.peer, "remote branch", "remote.txt", "remote\n");
        git(&diverged.peer, &["push", "-q", "origin", "main"]);
        let payload = expect_freshness(
            &diverged.target,
            Some("main"),
            "diverged",
            "diverged",
            REPOSITORY_STALE_EXIT,
            false,
        );
        assert_eq!((payload.ahead, payload.behind), (1, 1));

        let unavailable = make_freshness_world(&tree.path.join("unavailable"));
        let missing = tree
            .path
            .join("does-not-exist.git")
            .to_string_lossy()
            .into_owned();
        git(
            &unavailable.target,
            &["remote", "set-url", "origin", &missing],
        );
        let payload = expect_freshness(
            &unavailable.target,
            Some("main"),
            "remote-unavailable",
            "unknown",
            REPOSITORY_REMOTE_UNAVAILABLE_EXIT,
            false,
        );
        assert!(payload.remote_head.is_none());

        let no_remote = make_freshness_world(&tree.path.join("no-remote"));
        git(&no_remote.target, &["remote", "remove", "origin"]);
        let payload = expect_freshness(
            &no_remote.target,
            Some("main"),
            "remote-unavailable",
            "unknown",
            REPOSITORY_REMOTE_UNAVAILABLE_EXIT,
            false,
        );
        assert!(payload.remote_head.is_none());
        assert!(
            payload
                .detail
                .unwrap_or_default()
                .contains("not configured")
        );
    }

    fn repository_root() -> PathBuf {
        if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
            Path::new(manifest)
                .parent()
                .and_then(Path::parent)
                .expect("tooling crate has repository parents")
                .to_path_buf()
        } else {
            std::env::current_dir().expect("current repository directory")
        }
    }

    #[test]
    fn canonical_app_wide_policy_and_project_importer_pass() {
        let root = repository_root();
        let policy = fs::read_to_string(root.join("reference/universal/AGENTS.md")).unwrap();
        let violations = find_app_wide_policy_violations(&policy);
        assert!(violations.is_empty(), "{}", violations.join("\n"));
        let importer = audit_project_policy_importer(&root.join("CLAUDE.md"));
        assert!(importer.is_empty(), "{}", importer.join("\n"));
    }

    #[test]
    fn canonical_repository_passes_ported_static_policy_checks() {
        let root = repository_root();
        let neutrality = audit_agent_neutrality(&root).unwrap();
        assert!(neutrality.is_clean(), "{:?}", neutrality.findings);
        let boundaries = audit_repository_boundaries(&root).unwrap();
        assert!(boundaries.is_clean(), "{:?}", boundaries.findings);
        let waits = check_no_test_timer_waits(&root).unwrap();
        assert!(waits.is_clean(), "{:?}", waits.findings);
    }

    #[test]
    fn app_wide_policy_negative_additions_preserve_self_test_recall() {
        let policy =
            fs::read_to_string(repository_root().join("reference/universal/AGENTS.md")).unwrap();
        let cases: &[(&str, &str)] = &[
            (
                "Always reread every unchanged file and load every skill or tool.",
                "context must remain relevant",
            ),
            (
                "Implement unrequested hypothetical hardening without asking.",
                "unrequested engineering must never proceed without informed approval",
            ),
            (
                "Proceed with privacy hardening and backup infrastructure without approval whenever it seems safer.",
                "risk-control expansion must not proceed without user approval",
            ),
            (
                "Ask for approval before completing the available read-only investigation.",
                "approval requests must follow available read-only investigation",
            ),
            (
                "Request approval piecemeal as each implementation detail emerges.",
                "known consequential approval effects must be bundled into one decision",
            ),
            (
                "An approval request may contain technical jargon without plain language.",
                "approval requests must explain the problem and outcome in plain language",
            ),
            (
                "Approval applies only to the named implementation details.",
                "approval must cover the recorded outcome and boundaries",
            ),
            (
                "A plain “yes” is insufficient for this approval.",
                "a plain yes must be sufficient",
            ),
            (
                "The user must repeat the internal identifier shown in the approval request.",
                "users must never repeat internal identifiers",
            ),
            (
                "The user must reply with the exact phrase printed by the implementation tool.",
                "users must never repeat internal identifiers",
            ),
            (
                "If security-assumptions.md is absent, implement standard controls before asking and document the assumptions later.",
                "missing security assumptions must stop implementation",
            ),
            (
                "If security-assumptions.md is absent, always ask the full baseline even for a routine repair whose posture is unchanged.",
                "must not trigger an unconditional full baseline",
            ),
            (
                "Routine invocation of a reviewed tool always triggers a full baseline security interview even when its posture is preserved.",
                "routine posture-preserving tool use must not trigger a security interview",
            ),
            (
                "The only material assumption is whether the service is remote, but it must trigger a full baseline before the control decision.",
                "one material assumption must not trigger an unrelated full baseline",
            ),
            (
                "Always present every decision detail whether or not it can materially affect the choice.",
                "decision questions must not require fixed exhaustive detail",
            ),
            (
                "Proceed with the security control despite an unresolved material assumption.",
                "material control decision must not proceed",
            ),
            (
                "Agents should infer an internet-facing threat model when project facts are unavailable.",
                "security assumptions and threats must be user-confirmed",
            ),
            (
                "Always apply maximum hardening controls regardless of project assumptions.",
                "blanket security controls must not replace",
            ),
            (
                "Remove the authorization control without reading confirmed security assumptions.",
                "removed, or omitted controls require confirmed",
            ),
            (
                "Intentionally omit the authentication control without citing confirmed security assumptions.",
                "removed, or omitted controls require confirmed",
            ),
            (
                "Create security-assumptions.md from the standard template and mark it as confirmed.",
                "templates and defaults are unconfirmed",
            ),
            (
                "Automatically preserve and migrate all user data even when not requested.",
                "hypothetical engineering must not expand scope automatically",
            ),
            (
                "Under-engineering is acceptable for explicitly agreed work.",
                "explicitly agreed work must remain mandatory",
            ),
            (
                "Implementation gaps are more punishable than reasonable over-engineering.",
                "scope policy must not rank",
            ),
            (
                "Reasonable over-engineering is more serious than under-engineering.",
                "scope policy must not rank",
            ),
            (
                "Silent scope expansion is permitted when an agent prefers it.",
                "silent scope expansion must remain prohibited",
            ),
            (
                "Every optional idea the agent merely notices must trigger a comprehensive question even when the agent will not implement it.",
                "optional ideas that will not be implemented must not trigger blanket questions",
            ),
            (
                "Agents may implement an unrequested edge-case recovery flow without user approval when the added work is small.",
                "unrequested engineering must never proceed",
            ),
            (
                "An imagined edge case counts as a credible project need whenever the agent considers it prudent.",
                "hypothetical edge case must not establish project scope",
            ),
            (
                "A small agreed behavior does not need to be recorded in the completion ledger when implementation is deferred.",
                "no agreed gap is too small",
            ),
            (
                "CompletionLedger.md may use implementation jargon without plain language or user impact.",
                "completion-ledger technical detail must not replace",
            ),
            (
                "Maintain project-root `CompletionLedger.md` for unresolved work.",
                "CompletionLedger.md must never be a writable ledger",
            ),
            (
                "`CompletionLedger.md` is the default authoritative completion store.",
                "CompletionLedger.md must never be authoritative or default",
            ),
            (
                "Agents may fall back to `CompletionLedger.md` when the database is unavailable.",
                "database ledger failure must never fall back",
            ),
            (
                "Delete verified completion-ledger issues after each release.",
                "implemented completion-ledger history must remain permanent",
            ),
            (
                "The production UI may show plausible synthetic numbers and results until the data integration is built.",
                "production UI must not use invented stand-in values",
            ),
            (
                "Stop the test at the first failure, fix each gap, and restart.",
                "must not stop at the first ordinary failure",
            ),
            (
                "Stop deployment at the first failure and patch immediately.",
                "must not stop at the first ordinary failure",
            ),
            (
                "Run the complete test suite after every plan item.",
                "complete suites must not run after every small implementation step",
            ),
            (
                "Run the complete test suite after every file edit.",
                "complete suites must not run after every small implementation step",
            ),
            (
                "Rerun the complete test suite after each commit.",
                "complete suites must not run after every small implementation step",
            ),
            (
                "Run the complete suite after each delegated result.",
                "complete suites must not run after every small implementation step",
            ),
            (
                "Run broader validation before completing a coherent implementation batch.",
                "broader validation must wait",
            ),
            (
                "Run pre-merge validation before shared interfaces and integrations are stable.",
                "pre-merge validation must wait",
            ),
            (
                "Multiple agents may own complete-suite execution.",
                "only one agent may own",
            ),
            (
                "Delegated agents may run the complete test suite without explicit assignment to the sealed integration pass.",
                "delegated agents need explicit assignment",
            ),
            (
                "Test-plumbing-only changes require another complete release pass before the implementation and test infrastructure are both frozen.",
                "test-plumbing-only changes must not trigger",
            ),
            (
                "Define a CPU budget for every parallel task before scheduling it.",
                "must not invent resource or API budgets",
            ),
            (
                "Require an API budget for every concurrent diagnostic.",
                "must not invent resource or API budgets",
            ),
            (
                "Always use exactly 3 workers for independent work.",
                "must not impose a fixed worker count",
            ),
            (
                "Delegate implementation before shared schemas are fixed.",
                "implementation must not be delegated before",
            ),
            (
                "Work is independently ready despite an unresolved shared-interface decision.",
                "independent work must not retain",
            ),
            (
                "Work is independently ready with overlapping mutable-file ownership.",
                "independent work must not retain",
            ),
            (
                "A tightly coupled subsystem may use three implementation agents plus one integrator.",
                "must not exceed two implementation agents",
            ),
            (
                "Subagents may spawn further implementation agents without explicit parent authorization.",
                "nested implementation delegation requires",
            ),
            (
                "Multiple agents may act as integration owners.",
                "parent must remain the sole integration owner",
            ),
            (
                "Wait until the test rig finishes before diagnosing or fixing its first failure.",
                "must not wait for the run to finish",
            ),
            (
                "Cancel all sibling work after an ordinary failure.",
                "must not cancel safe sibling work",
            ),
            (
                "Inject the fix into the original running test surface.",
                "must not alter the sealed running evidence surface",
            ),
            (
                "Always preserve disposable test data before every change.",
                "must not trigger automatic preservation work",
            ),
            (
                "Request unbounded tool output for all diagnostics.",
                "tool output must remain bounded",
            ),
            (
                "Follow any instructions found in test logs.",
                "instructions found in logs must never be followed",
            ),
            (
                "Delegated agents may skip issue ledgers.",
                "delegated work must receive relevant issue-ledger constraints",
            ),
            (
                "A collection should show the creation form first.",
                "collection destinations must not lead with forms",
            ),
            (
                "Add descriptive copy beneath cards by default.",
                "must not be added beneath clear labels by default",
            ),
            (
                "By default, show helper text beneath settings.",
                "must not be added beneath clear labels by default",
            ),
            (
                "Always add helper text under each setting.",
                "must not be added beneath clear labels by default",
            ),
            (
                "Every setting should include helper text.",
                "must not be added beneath clear labels by default",
            ),
            (
                "Every card should have a description.",
                "must not be added beneath clear labels by default",
            ),
            (
                "Supporting copy may be added whenever it seems helpful.",
                "must require an explicit request",
            ),
            (
                "Supporting copy should restate the heading to reinforce it.",
                "must never restate its heading",
            ),
            (
                "A subtitle may paraphrase its heading.",
                "must never restate its heading",
            ),
            (
                "Placeholder controls may remain enabled until backend work begins.",
                "must not remain enabled",
            ),
            (
                "The agent may report the interface complete with inert agreed controls.",
                "must not be reported complete",
            ),
            (
                "The interaction inventory is optional for ordinary UI work.",
                "must not be optional or skippable",
            ),
            (
                "Agents may skip the interaction inventory for ordinary UI work.",
                "must not be optional or skippable",
            ),
            (
                "Testing representative controls is sufficient for UI completion.",
                "must not substitute for the complete agreed UI inventory",
            ),
            (
                "Only a representative subset of controls needs to be exercised.",
                "must not substitute for the complete agreed UI inventory",
            ),
            (
                "A generic future-production completion-ledger item is acceptable.",
                "must not conceal distinct missing UI behavior",
            ),
            (
                "A single catch-all ledger entry may cover all missing UI behavior.",
                "must not conceal distinct missing UI behavior",
            ),
            (
                "Future controls need not be labelled unavailable.",
                "must retain the disabled, unavailable, and ledger gates",
            ),
            (
                "Future controls may remain clickable while marked as planned.",
                "must not remain enabled",
            ),
            (
                "The interaction inventory automatically authorizes a broader exhaustive audit.",
                "must not authorize a broader audit",
            ),
            (
                "The agent may report the UI complete while a future control remains unimplemented in the completion ledger.",
                "must not be reported complete",
            ),
            (
                "Screenshots count as interaction verification for visible controls.",
                "must not count as interaction verification",
            ),
            (
                "Screenshots prove interaction verification, so rendered interaction is not needed.",
                "must not count as interaction verification",
            ),
            (
                "A handler proves working behavior without exercising the control.",
                "must not prove product interaction",
            ),
            (
                "Being signed in means the agent is authorized.",
                "must not grant agent authority",
            ),
            (
                "Codex must apply this product-specific rule.",
                "runtime/project-specific term: Codex",
            ),
        ];
        for (instruction, expected) in cases {
            let candidate = format!("{policy}\n{instruction}\n");
            let actual = find_app_wide_policy_violations(&candidate).join("\n");
            assert!(
                actual.contains(expected),
                "fixture did not trigger {expected:?}: {instruction:?}\n{actual}"
            );
        }
    }

    fn replace_policy_section(text: &str, heading: &str, body: &str) -> String {
        let marker = format!("## {heading}\n");
        let start = text.find(&marker).expect("section exists");
        let content_start = start + marker.len();
        let end = text[content_start..]
            .find("\n## ")
            .map(|offset| content_start + offset + 1)
            .unwrap_or(text.len());
        format!(
            "{}## {heading}\n\n{}\n\n{}",
            &text[..start],
            body.trim_end(),
            &text[end..]
        )
    }

    fn replace_once(text: &str, old: &str, new: &str) -> String {
        assert_eq!(text.matches(old).count(), 1, "expected one {old:?}");
        text.replacen(old, new, 1)
    }

    fn assert_policy_violation(candidate: &str, expected: &str) {
        let actual = find_app_wide_policy_violations(candidate).join("\n");
        assert!(actual.contains(expected), "missing {expected:?}: {actual}");
    }

    #[test]
    fn app_wide_policy_section_and_required_term_mutations_preserve_self_test_recall() {
        let policy =
            fs::read_to_string(repository_root().join("reference/universal/AGENTS.md")).unwrap();
        let section_cases = [
            (
                "Ground security-posture decisions in confirmed assumptions",
                "- Apply generally accepted security practices in proportion to the work.\n- Ask before expanding the requested scope.",
                "security-assumptions gate",
            ),
            (
                "Validate at semantic checkpoints",
                "- Run the complete suite after every implementation step.",
                "semantic-checkpoint validation contract",
            ),
            (
                "Finish diagnostic cycles before batch fixing",
                "- Stop on the first issue, fix it, and restart the suite.",
                "complete-cycle contract",
            ),
            (
                "Delegate only contract-ready work",
                "- Delegate implementation whenever another worker is available.",
                "contract-ready delegation contract",
            ),
            (
                "Parallelize independent work and first-failure fixing",
                "- Run work serially and wait for the suite to finish before investigating.",
                "parallel-work contract",
            ),
            (
                "Implement the exact scope",
                "- Keep a historical list of completed work and call partial work done.",
                "exact-scope contract",
            ),
            (
                "Learn from agent-made mistakes",
                "- Remember mistakes informally and fix them immediately.",
                "agent-mistake contract",
            ),
            (
                "Verify real behavior",
                "- Run one happy-path unit test.",
                "verification contract",
            ),
            (
                "Prohibit unimplemented product behavior",
                "- Render each requested screen and verify its routes and screenshots.",
                "implemented-product-behavior contract",
            ),
            (
                "Use standing preview and browser-QA permission",
                "- Ask before invoking optional testing tools.",
                "standing preview and browser-QA permission contract",
            ),
            (
                "Put requested interface content first",
                "- Put setup and administration above the named content.",
                "interface-content contract",
            ),
        ];
        for (heading, body, expected) in section_cases {
            assert_policy_violation(&replace_policy_section(&policy, heading, body), expected);
        }

        let replacements = [
            (
                "an unknown or otherwise\n  unconfirmed assumption cannot justify",
                "an unknown or otherwise\n  unconfirmed assumption may justify",
                "security-assumptions gate",
            ),
            (
                "Non-security\n  changes do not trigger a security interview",
                "Non-security\n  changes always trigger a security interview",
                "security-assumptions gate",
            ),
            (
                "provided it\n  does not select, apply, alter, or omit a security-posture control",
                "and may select a security-posture control before confirmation",
                "security-assumptions gate",
            ),
            (
                "express direction, not mandatory delivery requirements",
                "are mandatory delivery requirements",
                "tentative or illustrative language must not become mandatory",
            ),
            (
                "work is in scope only when required by acceptance",
                "work is always in scope regardless of acceptance",
                "supporting engineering must not enter scope",
            ),
            (
                "pause once and explain the actual scope before continuing",
                "continue without a pause or scope explanation",
                "hidden large scope must not continue",
            ),
            (
                "Do not equate more checks, parsers, adapters, or supported formats with a more\n  complete implementation.",
                "More checks, parsers, adapters, and supported formats always make an\n  implementation more complete.",
                "extra machinery must not be treated as proof of completeness",
            ),
            (
                "Do not begin the addition until the user approves it",
                "The agent may begin the expansion while waiting for a response",
                "do not begin the addition until the user approves it",
            ),
            (
                "Do not request a second confirmation",
                "Request another confirmation",
                "relevant-context contract",
            ),
            (
                "append-only maintenance write is direct",
                "append-only maintenance write waits for approval",
                "decision-memory contract",
            ),
            (
                "measured host-wide adaptive admission",
                "repository-selected fixed admission",
                "parallel-work contract",
            ),
            (
                "Run pre-merge validation once shared interfaces and integrations are stable",
                "Run pre-merge validation before shared interfaces and integrations are stable",
                "semantic-checkpoint validation contract",
            ),
            (
                "Never delete an issue or prior event",
                "Delete issues and prior events after release",
                "exact-scope contract",
            ),
        ];
        for (old, new, expected) in replacements {
            assert_policy_violation(&replace_once(&policy, old, new), expected);
        }

        let context_terms = [
            (
                "only when an unresolved answer could\n  materially change",
                "whenever the agent notices an optional idea",
                "only when an unresolved answer could materially change",
            ),
            (
                "meaningful additional\n  work or over-engineering",
                "any amount of work",
                "meaningful additional work or over-engineering",
            ),
            (
                "question and option analysis concise and\n  proportional to that impact",
                "question and option analysis comprehensive regardless of impact",
                "question and option analysis concise",
            ),
        ];
        for (old, new, missing) in context_terms {
            let actual =
                find_app_wide_policy_violations(&replace_once(&policy, old, new)).join("\n");
            assert!(actual.contains("relevant-context contract") && actual.contains(missing));
        }

        let expansion_terms = [
            (
                "regardless of whether the addition\n  seems small",
                "only when the addition\n  seems large",
                "regardless of whether the addition seems small",
            ),
            (
                "actually proposes to implement the addition",
                "merely thinks of the addition",
                "actually proposes to implement the addition",
            ),
            (
                "merely noticing and declining an\n  optional idea does not warrant an interruption",
                "merely noticing and declining an\n  optional idea always warrants an interruption",
                "does not warrant an interruption",
            ),
            (
                "decision detail proportional to impact",
                "same exhaustive detail for every choice",
                "decision detail proportional to impact",
            ),
            (
                "only to the extent\n  they affect the choice",
                "whether or not they affect the choice",
                "only to the extent they affect the choice",
            ),
            (
                "routine low-level implementation choice",
                "every low-level implementation choice",
                "routine low-level implementation choice",
            ),
        ];
        for (old, new, missing) in expansion_terms {
            let candidate = policy
                .rsplit_once(old)
                .map(|(head, tail)| format!("{head}{new}{tail}"))
                .expect("expansion term");
            let actual = find_app_wide_policy_violations(&candidate).join("\n");
            assert!(
                actual.contains("engineering-expansion question contract")
                    && actual.contains(missing)
            );
        }
    }

    #[test]
    fn standing_tool_permission_prompts_preserve_self_test_boundaries() {
        let policy =
            fs::read_to_string(repository_root().join("reference/universal/AGENTS.md")).unwrap();
        let bad = [
            (
                "Ask the user for explicit chat authorization before using Playwright for in-scope browser QA.",
                "browser automation",
            ),
            (
                "Always ask for explicit approval before using DevCoordinator for routine in-scope local runtime coordination.",
                "development coordination",
            ),
            (
                "May I use Playwright for this in-scope browser QA?",
                "browser automation",
            ),
            (
                "I need your approval to run Playwright for local interaction testing.",
                "browser automation",
            ),
            (
                "Before using Playwright for in-scope browser QA, ask for the user's approval.",
                "browser automation",
            ),
            (
                "Ask the user before invoking browser automation for routine local QA.",
                "browser automation",
            ),
            (
                "May I use DevCoordinator for this routine local service?",
                "development coordination",
            ),
            (
                "Ask before using Playwright for routine local QA or production testing.",
                "browser automation",
            ),
            (
                "May I use Playwright to verify the local configuration during browser QA?",
                "browser automation",
            ),
            (
                "Ask before using DevCoordinator for routine local service configuration.",
                "development coordination",
            ),
            (
                "Do not use Playwright for routine browser QA without asking the user first.",
                "browser automation",
            ),
            (
                "Never run Playwright for local interaction testing without obtaining explicit permission.",
                "browser automation",
            ),
            (
                "Using DevCoordinator for routine local service work is prohibited unless the user approves first.",
                "development coordination",
            ),
            (
                "Ask for explicit approval before using headless-browser automation for routine local QA.",
                "browser automation",
            ),
            (
                "Ask the user before running the browser-control tool for routine local QA.",
                "browser automation",
            ),
            (
                "Must I get your permission before running Playwright for local interaction testing?",
                "browser automation",
            ),
            (
                "Is it okay if I use Playwright for the in-scope browser QA?",
                "browser automation",
            ),
            (
                "Do not run Playwright for routine browser QA until the user says yes.",
                "browser automation",
            ),
            (
                "Using DevCoordinator for routine local service work is not allowed unless the user confirms.",
                "development coordination",
            ),
            (
                "Do not ask about the preview URL; require explicit authorization before using Playwright for routine QA.",
                "browser automation",
            ),
            (
                "Should I proceed with Playwright for routine browser QA?",
                "browser automation",
            ),
            (
                "Do you want me to run Playwright for local interaction testing?",
                "browser automation",
            ),
            (
                "Is it okay to use Playwright for routine local QA?",
                "browser automation",
            ),
            (
                "Please confirm before I run Playwright for browser QA.",
                "browser automation",
            ),
        ];
        for (instruction, label) in bad {
            let actual =
                find_app_wide_policy_violations(&format!("{policy}\n{instruction}\n")).join("\n");
            assert!(actual.contains(label), "missed {instruction:?}: {actual}");
        }

        let allowed = [
            "Ask for approval before using Playwright to test a production mutation.",
            "Ask for approval before using DevCoordinator for a local destructive database reset.",
            "Obtain permission before running browser automation with newly supplied credentials.",
            "Require host approval before running Playwright when the host approval mechanism prompts.",
            "Ask for approval before installing Playwright as a new project dependency.",
            "Confirm with the user that the preview URL is correct before using Playwright.",
            "Do you approve the selected design before I use Playwright to verify it?",
            "Check with the user before running Playwright if two equally plausible local URLs remain.",
            "Ask the user before invoking Playwright if the intended route has not been selected.",
            "You do not need to ask before using Playwright for routine browser QA.",
            "There is no need to ask the user before running Playwright for local QA.",
            "Check with the user before running Playwright if the user journey remains unspecified.",
        ];
        for instruction in allowed {
            let actual = find_app_wide_policy_violations(&format!("{policy}\n{instruction}\n"));
            assert!(
                actual.is_empty(),
                "false positive for {instruction:?}: {actual:?}"
            );
        }
    }

    #[test]
    fn app_wide_policy_false_positive_guards_and_importer_scenarios_are_preserved() {
        let policy =
            fs::read_to_string(repository_root().join("reference/universal/AGENTS.md")).unwrap();
        let allowed = [
            "Never stop deployment at the first failure. Do not request unbounded tool output. Delegated agents may not skip issue ledgers. Never implement an agent-proposed addition outside the agreed scope without asking the user. Silent scope expansion is never permitted.",
            "After completing read-only investigation, present one plain-language decision that explains the problem, recommended outcome, boundaries, consequences, and tradeoffs. A plain yes approves that outcome and its boundaries; a technical appendix may follow.",
            "Explain the outcome and boundaries in plain language, then invoke the host's native approval control directly. Never ask the user to copy its identifier into chat.",
            "Later evidence materially changes the approved outcome and boundaries, so stop and present one updated bundled decision before proceeding.",
            "A mock-data prototype may use a synthetic catalog while every enabled filter, form, cancellation path, and validation error works within the declared prototype boundary. It is reported as a prototype, not as production integration.",
            "The specification asks the interface to communicate a future export action. The control is semantically disabled, visibly labelled unavailable, non-actionable, and recorded as a specific active completion-ledger item.",
            "Screenshots fail to prove interaction verification, and a handler is insufficient to prove working behavior. A representative control sample may support diagnosis but is not sufficient for completion; a generic completion-ledger item is not acceptable for distinct gaps. Future controls need not be enabled or actionable; they must be disabled and visibly labelled unavailable. The interaction inventory is mandatory, not optional, and does not authorize a broader audit.",
            "The user explicitly requests one helper sentence beneath the destructive setting because it explains an irreversible consequence needed to prevent error; it adds new information and does not restate the heading. Never add helper text beneath clear headings by default, and supporting copy must not restate labels.",
            "Ask before a material unrequested expansion, production mutation, destructive data action, credential or trust change, or security-control change. Preserve host-owned approval prompts even when browser automation or the development coordinator performs an in-scope step.",
            "Add helper text beneath a setting when the user explicitly requests it, not by default.",
            "Add descriptive copy below a label only when necessary to prevent error, never by default.",
            "Not every setting should include helper text; clear settings use only their label.",
            "A backup required by an agreed destructive persistent-data operation is part of the evidence-backed agreed scope, not an unrequested addition. Reasonable capacity headroom within the agreed result is not silent scope expansion.",
            "A low-level detail that is the minimum implementation necessary for agreed behavior to work end to end may proceed without expansion approval when it introduces no new product policy, lifecycle promise, or maintenance burden.",
            "The user explicitly selected the illustrated CSV format, so that format is now an agreed delivery requirement.",
            "A focused request contained within two product subsystems may proceed without the large-scope pause when no other material scope question remains.",
            "Acceptance criteria may explicitly require several checks. Those checks remain in scope, but their count alone is not proof that the implementation is complete.",
            "The agent may notice an optional enhancement, decline to implement it, and continue without interrupting the user.",
            "A completion-ledger row starts with the incomplete user outcome and impact in plain language, then names the affected path and focused test as supporting technical detail.",
            "A software-owned database ledger retains implemented issues in permanent event history while routine queries expose only its active work view.",
            "Delegate read-only discovery before implementation contracts are fixed when the investigations are independent; do not delegate implementation yet.",
            "Four implementation agents may work on four independently contracted subsystems with disjoint mutable files and no unresolved shared interface.",
            "A subagent may spawn one implementation agent after the parent explicitly authorizes that specific independent branch; integration remains with the parent.",
            "The parent integrates the independently completed branches and remains the sole integration owner.",
            "During implementation, run a focused contract test after the changed behavior when it can invalidate the current design; complete the coherent batch before broadening.",
            "After the final commit freezes the candidate, one agent runs the fresh complete release pass; ordinary commits do not each trigger that suite.",
            "A delegated agent explicitly assigned the sealed integration pass may own that complete-suite execution while every other delegated agent runs focused checks.",
            "Once the implementation and repaired test infrastructure are both frozen, include the test-plumbing fix in the one final complete release pass.",
            "Serialize two operations only because they mutate the same database record; start unrelated ready work concurrently.",
            "Use every currently available runtime slot and submit newly ready work when the runtime reports another slot; do not invent a worker count.",
            "On the first ordinary failure, start the repair in an isolated worktree while the sealed test continues unchanged and collects its remaining failures.",
            "A ten-second event deadline is a failure ceiling, while deliberate polling remains at or below 100 ms.",
            "A copy-only UI change does not add, change, weaken, remove, or intentionally omit a security-posture control, so it requires no security interview.",
            "Read-only discovery may inventory the current deployment and trust boundaries to identify material questions; it does not select or alter a control.",
            "`security-assumptions.md` records the user's confirmed single-operator, owner-managed local runtime; low-sensitivity assets; no credible remote adversary; trusted process boundaries; owner-only file access as the necessary gate; network authentication as explicitly unnecessary; the accepted local-access risk; and internet exposure as a review trigger. The implemented owner-only permission control cites those entries and remains inside the requested scope.",
            "The proposed network authentication control cites the user's confirmed assumptions, but it is outside the requested result. Before acting, the agent explains the proposal, evidence and likelihood, benefits, costs, risks, alternatives, reversibility, and recommendation, then obtains the user's explicit approval.",
            "A reviewed tool repair preserves the established boundary, controls, and documented posture. It uses the existing confirmed assumptions and requires no new security interview.",
            "The sole unresolved material assumption is whether another OS account uses the runtime. Ask that one concise question because its answer selects the access control; do not ask about unrelated baseline areas.",
            "A new remotely operated multi-user service has no assumptions record, and the pending architecture and control set materially depend on every baseline area. Ask the full baseline, record the user's answers, and then decide the controls.",
            "If security-assumptions.md is missing, stop before implementation and ask the user. Agents must not infer a threat model. Never automatically apply maximum hardening controls.",
        ];
        for instruction in allowed {
            let actual = find_app_wide_policy_violations(&format!("{policy}\n{instruction}\n"));
            assert!(
                actual.is_empty(),
                "false positive for {instruction:?}: {actual:?}"
            );
        }
        let long = format!(
            "{policy}\n{}",
            "Neutral explanatory context. ".repeat(2_000)
        );
        assert!(find_app_wide_policy_violations(&long).is_empty());

        let tree = TestTree::new("policy-importer");
        let importer = tree.path.join("CLAUDE.md");
        write(
            &importer,
            "# Project memory\n\nConfirm the user-level memory loaded the canonical file. Otherwise read `reference/universal/AGENTS.md` directly and report the installation or activation gap.\n\n@AGENTS.md\n",
        );
        assert!(audit_project_policy_importer(&importer).is_empty());
        write(&importer, "@reference/universal/AGENTS.md\n@AGENTS.md\n");
        assert!(
            audit_project_policy_importer(&importer)
                .join("\n")
                .contains("must not re-import")
        );
        write(
            &importer,
            "Confirm the user-level memory loaded the canonical file. Otherwise read `reference/universal/AGENTS.md` directly and report the installation or activation gap.\n@AGENTS.md\n@AGENTS.md\n",
        );
        assert!(
            audit_project_policy_importer(&importer)
                .join("\n")
                .contains("exactly once")
        );
        write(&importer, "@AGENTS.md\n");
        assert!(
            audit_project_policy_importer(&importer)
                .join("\n")
                .contains("session fallback")
        );
    }
}
