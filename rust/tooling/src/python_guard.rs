//! Repository-wide guard for the Rust control-plane's Python-free boundary.
//!
//! The guard inventories Git-tracked files instead of walking the working tree,
//! so ignored build output cannot affect the result. It reports only stable
//! finding metadata (kind, repository-relative path, and line number); matched
//! source text is deliberately never retained or returned.

use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tree_sitter::Parser;

/// The deterministic default artifact used by `tooling check python-free`.
pub const DEFAULT_REPORT_RELATIVE_PATH: &str =
    ".devcoordinator/agent-validation/python-free/report.json";

/// Result payloads stay bounded even for a deliberately hostile repository.
pub const MAX_REPORTED_FINDINGS: usize = 256;
const MAX_REPORTED_PATH_BYTES: usize = 1_024;
const MAX_INSPECTED_TEXT_BYTES: u64 = 16 * 1024 * 1024;

/// Python files retained solely as marker-free, cross-language audit inputs.
///
/// These paths are an exact allowlist. They are parsed as Python source by
/// Tree-sitter but are never executed by this tool.
pub const ALLOWED_INERT_PYTHON_FIXTURES: [&str; 7] = [
    "skills/full-repo-audit/evals/marker-free/cases/false-persistence-success/repo/contacts.py",
    "skills/full-repo-audit/evals/marker-free/cases/ignored-input-config/repo/candidates.py",
    "skills/full-repo-audit/evals/marker-free/cases/missing-registration-lifecycle/repo/maintenance.py",
    "skills/full-repo-audit/evals/marker-free/cases/partial-plumbing-no-dependency-call/repo/delivery.py",
    "skills/full-repo-audit/evals/marker-free/cases/production-fixture-mock/repo/catalog.py",
    "skills/full-repo-audit/evals/marker-free/cases/shallow-outcome-tests/repo/invoice.py",
    "skills/full-repo-audit/evals/marker-free/cases/shallow-outcome-tests/repo/tests/test_invoice.py",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PythonGuardStatus {
    Clean,
    Findings,
}

impl PythonGuardStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Findings => "findings",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PythonGuardFindingKind {
    UnexpectedPythonFile,
    PythonShebang,
    ActivePythonInvocation,
    PythonProjectManifest,
    CiPythonSetup,
    InvalidInertPythonFixture,
    InertPythonFixtureNotRegular,
    TrackedFileUnavailable,
    TrackedFileTooLarge,
    UnsafeTrackedPath,
}

impl PythonGuardFindingKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnexpectedPythonFile => "unexpected_python_file",
            Self::PythonShebang => "python_shebang",
            Self::ActivePythonInvocation => "active_python_invocation",
            Self::PythonProjectManifest => "python_project_manifest",
            Self::CiPythonSetup => "ci_python_setup",
            Self::InvalidInertPythonFixture => "invalid_inert_python_fixture",
            Self::InertPythonFixtureNotRegular => "inert_python_fixture_not_regular",
            Self::TrackedFileUnavailable => "tracked_file_unavailable",
            Self::TrackedFileTooLarge => "tracked_file_too_large",
            Self::UnsafeTrackedPath => "unsafe_tracked_path",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonGuardFinding {
    pub kind: PythonGuardFindingKind,
    pub path: String,
    pub line: Option<u64>,
}

impl PythonGuardFinding {
    fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "path": self.path,
            "line": self.line,
        })
    }
}

/// Bounded, source-free result of one tracked-file inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonGuardReport {
    pub status: PythonGuardStatus,
    pub tracked_files: usize,
    pub inspected_text_files: usize,
    pub inert_python_fixtures: usize,
    pub total_findings: usize,
    pub findings_truncated: bool,
    pub findings: Vec<PythonGuardFinding>,
}

impl PythonGuardReport {
    pub fn is_clean(&self) -> bool {
        self.status == PythonGuardStatus::Clean
    }

    pub fn to_json(&self) -> Value {
        json!({
            "schema": 1,
            "check": "python_free",
            "status": self.status.as_str(),
            "tracked_files": self.tracked_files,
            "inspected_text_files": self.inspected_text_files,
            "inert_python_fixtures": self.inert_python_fixtures,
            "total_findings": self.total_findings,
            "findings_truncated": self.findings_truncated,
            "findings": self.findings.iter().map(PythonGuardFinding::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Small stdout-safe receipt for an artifact-backed guard run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonGuardReceipt {
    pub status: PythonGuardStatus,
    pub report_file: String,
    pub tracked_files: usize,
    pub inert_python_fixtures: usize,
    pub total_findings: usize,
    pub findings_truncated: bool,
}

impl PythonGuardReceipt {
    pub fn is_clean(&self) -> bool {
        self.status == PythonGuardStatus::Clean
    }

    pub fn to_json(&self) -> Value {
        json!({
            "schema": 1,
            "check": "python_free",
            "status": self.status.as_str(),
            "report_file": self.report_file,
            "tracked_files": self.tracked_files,
            "inert_python_fixtures": self.inert_python_fixtures,
            "total_findings": self.total_findings,
            "findings_truncated": self.findings_truncated,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PythonGuardErrorKind {
    RepositoryUnavailable,
    GitUnavailable,
    GitInventoryFailed,
    GitInventoryInvalid,
    PythonParserUnavailable,
    ReportWriteFailed,
}

impl PythonGuardErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RepositoryUnavailable => "repository_unavailable",
            Self::GitUnavailable => "git_unavailable",
            Self::GitInventoryFailed => "git_inventory_failed",
            Self::GitInventoryInvalid => "git_inventory_invalid",
            Self::PythonParserUnavailable => "python_parser_unavailable",
            Self::ReportWriteFailed => "report_write_failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonGuardError {
    pub kind: PythonGuardErrorKind,
    pub message: &'static str,
}

impl PythonGuardError {
    const fn new(kind: PythonGuardErrorKind, message: &'static str) -> Self {
        Self { kind, message }
    }
}

impl fmt::Display for PythonGuardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for PythonGuardError {}

/// Inventory and inspect the repository's tracked working-tree files.
pub fn inspect_repository(repository_root: &Path) -> Result<PythonGuardReport, PythonGuardError> {
    let repository_root = canonical_repository_root(repository_root)?;
    let tracked_paths = tracked_files(&repository_root)?;
    let mut collector = FindingCollector::default();
    let mut inspected_text_files = 0usize;
    let mut inert_python_fixtures = 0usize;

    let mut python_parser = Parser::new();
    python_parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|_| {
            PythonGuardError::new(
                PythonGuardErrorKind::PythonParserUnavailable,
                "could not initialize the inert Python fixture parser",
            )
        })?;

    for relative in &tracked_paths {
        let display_path = bounded_path(relative);
        if !is_safe_relative_path(relative) {
            collector.push(PythonGuardFinding {
                kind: PythonGuardFindingKind::UnsafeTrackedPath,
                path: display_path,
                line: None,
            });
            continue;
        }

        let normalized = normalized_path(relative);
        let is_python = relative.extension() == Some(OsStr::new("py"));
        let is_inert_fixture = ALLOWED_INERT_PYTHON_FIXTURES.contains(&normalized.as_str());

        if relative.file_name() == Some(OsStr::new("pyproject.toml")) {
            collector.push(PythonGuardFinding {
                kind: PythonGuardFindingKind::PythonProjectManifest,
                path: display_path.clone(),
                line: None,
            });
        }

        if is_python && !is_inert_fixture {
            collector.push(PythonGuardFinding {
                kind: PythonGuardFindingKind::UnexpectedPythonFile,
                path: display_path.clone(),
                line: None,
            });
        }

        let absolute = repository_root.join(relative);
        let metadata = match fs::symlink_metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(_) => {
                collector.push(PythonGuardFinding {
                    kind: PythonGuardFindingKind::TrackedFileUnavailable,
                    path: display_path,
                    line: None,
                });
                continue;
            }
        };

        if !metadata.file_type().is_file() {
            if is_inert_fixture {
                collector.push(PythonGuardFinding {
                    kind: PythonGuardFindingKind::InertPythonFixtureNotRegular,
                    path: display_path,
                    line: None,
                });
            }
            continue;
        }

        let first_line = match read_first_line(&absolute) {
            Ok(line) => line,
            Err(_) => {
                collector.push(PythonGuardFinding {
                    kind: PythonGuardFindingKind::TrackedFileUnavailable,
                    path: display_path,
                    line: None,
                });
                continue;
            }
        };
        if is_python_shebang(&first_line) {
            collector.push(PythonGuardFinding {
                kind: PythonGuardFindingKind::PythonShebang,
                path: display_path.clone(),
                line: Some(1),
            });
        }

        if is_inert_fixture {
            inert_python_fixtures += 1;
            if metadata.len() > MAX_INSPECTED_TEXT_BYTES {
                collector.push(PythonGuardFinding {
                    kind: PythonGuardFindingKind::TrackedFileTooLarge,
                    path: display_path,
                    line: None,
                });
                continue;
            }
            let source = match fs::read(&absolute) {
                Ok(source) => source,
                Err(_) => {
                    collector.push(PythonGuardFinding {
                        kind: PythonGuardFindingKind::TrackedFileUnavailable,
                        path: display_path,
                        line: None,
                    });
                    continue;
                }
            };
            inspected_text_files += 1;
            if !is_valid_python_source(&mut python_parser, &source) {
                collector.push(PythonGuardFinding {
                    kind: PythonGuardFindingKind::InvalidInertPythonFixture,
                    path: display_path,
                    line: None,
                });
            }
            continue;
        }

        if !is_scannable_text_path(relative) {
            continue;
        }
        if metadata.len() > MAX_INSPECTED_TEXT_BYTES {
            collector.push(PythonGuardFinding {
                kind: PythonGuardFindingKind::TrackedFileTooLarge,
                path: display_path,
                line: None,
            });
            continue;
        }
        let source = match fs::read(&absolute) {
            Ok(source) => source,
            Err(_) => {
                collector.push(PythonGuardFinding {
                    kind: PythonGuardFindingKind::TrackedFileUnavailable,
                    path: display_path,
                    line: None,
                });
                continue;
            }
        };
        if source.contains(&0) {
            continue;
        }
        inspected_text_files += 1;
        inspect_text(relative, &source, &display_path, &mut collector);
    }

    let total_findings = collector.total;
    Ok(PythonGuardReport {
        status: if total_findings == 0 {
            PythonGuardStatus::Clean
        } else {
            PythonGuardStatus::Findings
        },
        tracked_files: tracked_paths.len(),
        inspected_text_files,
        inert_python_fixtures,
        total_findings,
        findings_truncated: total_findings > collector.findings.len(),
        findings: collector.findings,
    })
}

/// Run the guard, atomically persist its bounded report, and return a small
/// receipt suitable for default command output.
pub fn inspect_repository_to_report(
    repository_root: &Path,
    report_file: &Path,
) -> Result<PythonGuardReceipt, PythonGuardError> {
    let report = inspect_repository(repository_root)?;
    write_report(report_file, &report)?;
    Ok(PythonGuardReceipt {
        status: report.status,
        report_file: bounded_display_path(report_file),
        tracked_files: report.tracked_files,
        inert_python_fixtures: report.inert_python_fixtures,
        total_findings: report.total_findings,
        findings_truncated: report.findings_truncated,
    })
}

fn canonical_repository_root(root: &Path) -> Result<PathBuf, PythonGuardError> {
    let canonical = root.canonicalize().map_err(|_| {
        PythonGuardError::new(
            PythonGuardErrorKind::RepositoryUnavailable,
            "repository root is unavailable",
        )
    })?;
    if !canonical.is_dir() {
        return Err(PythonGuardError::new(
            PythonGuardErrorKind::RepositoryUnavailable,
            "repository root is not a directory",
        ));
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(&canonical)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            PythonGuardError::new(
                PythonGuardErrorKind::GitUnavailable,
                "could not invoke Git for the repository inventory",
            )
        })?;
    if !output.status.success() {
        return Err(PythonGuardError::new(
            PythonGuardErrorKind::GitInventoryFailed,
            "Git could not resolve the repository root",
        ));
    }
    let top = std::str::from_utf8(trim_ascii(&output.stdout)).map_err(|_| {
        PythonGuardError::new(
            PythonGuardErrorKind::GitInventoryInvalid,
            "Git returned a non-UTF-8 repository root",
        )
    })?;
    let top = Path::new(top).canonicalize().map_err(|_| {
        PythonGuardError::new(
            PythonGuardErrorKind::GitInventoryInvalid,
            "Git returned an unavailable repository root",
        )
    })?;
    if top != canonical {
        return Err(PythonGuardError::new(
            PythonGuardErrorKind::RepositoryUnavailable,
            "the supplied path is not the repository root",
        ));
    }
    Ok(canonical)
}

fn tracked_files(root: &Path) -> Result<Vec<PathBuf>, PythonGuardError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--cached", "--"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            PythonGuardError::new(
                PythonGuardErrorKind::GitUnavailable,
                "could not invoke Git for the tracked-file inventory",
            )
        })?;
    if !output.status.success() {
        return Err(PythonGuardError::new(
            PythonGuardErrorKind::GitInventoryFailed,
            "Git could not inventory tracked files",
        ));
    }

    let mut paths = Vec::new();
    for raw in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let path = std::str::from_utf8(raw).map_err(|_| {
            PythonGuardError::new(
                PythonGuardErrorKind::GitInventoryInvalid,
                "Git returned a non-UTF-8 tracked path",
            )
        })?;
        paths.push(PathBuf::from(path));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn write_report(path: &Path, report: &PythonGuardReport) -> Result<(), PythonGuardError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent).map_err(|_| report_write_error())?;
    }
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(report_write_error)?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|_| report_write_error())?;
    let write_result = (|| -> Result<(), io::Error> {
        serde_json::to_writer_pretty(&mut file, &report.to_json())?;
        file.write_all(b"\n")?;
        file.sync_all()
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(report_write_error());
    }
    if fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(report_write_error());
    }
    Ok(())
}

const fn report_write_error() -> PythonGuardError {
    PythonGuardError::new(
        PythonGuardErrorKind::ReportWriteFailed,
        "could not write the Python-free guard report",
    )
}

#[derive(Default)]
struct FindingCollector {
    total: usize,
    findings: Vec<PythonGuardFinding>,
}

impl FindingCollector {
    fn push(&mut self, finding: PythonGuardFinding) {
        self.total = self.total.saturating_add(1);
        if self.findings.len() < MAX_REPORTED_FINDINGS {
            self.findings.push(finding);
        }
    }
}

fn read_first_line(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut buffer = [0u8; 512];
    let count = file.read(&mut buffer)?;
    let end = buffer[..count]
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(count);
    Ok(buffer[..end].to_vec())
}

fn is_python_shebang(line: &[u8]) -> bool {
    let Ok(line) = std::str::from_utf8(line) else {
        return false;
    };
    let Some(command) = line.strip_prefix("#!") else {
        return false;
    };
    let words = shell_words(command);
    if words.is_empty() {
        return false;
    }
    if executable_is_python(&words[0]) {
        return true;
    }
    executable_basename(&words[0]) == "env"
        && words
            .iter()
            .skip(1)
            .find(|word| !word.starts_with('-') && !is_assignment(word))
            .is_some_and(|word| executable_is_python(word))
}

fn is_valid_python_source(parser: &mut Parser, source: &[u8]) -> bool {
    parser
        .parse(source, None)
        .is_some_and(|tree| !tree.root_node().has_error())
}

fn inspect_text(path: &Path, source: &[u8], display_path: &str, collector: &mut FindingCollector) {
    let kind = text_kind(path);
    let mut markdown = MarkdownState::default();
    let mut rust = RustScanState::default();
    let mut configuration = ConfigurationState::default();
    for (offset, raw_line) in source.split(|byte| *byte == b'\n').enumerate() {
        let line_number = (offset + 1) as u64;
        let line = String::from_utf8_lossy(raw_line);

        if is_ci_path(path) && is_ci_python_setup(&line) {
            collector.push(PythonGuardFinding {
                kind: PythonGuardFindingKind::CiPythonSetup,
                path: display_path.to_owned(),
                line: Some(line_number),
            });
        }

        let active_invocation = match kind {
            TextKind::Rust => rust.invokes_python(&line),
            TextKind::JavaScript => javascript_invokes_python(&line),
            TextKind::Shell => shell_line_invokes_python(&line),
            TextKind::Configuration => configuration.invokes_python(&line),
            TextKind::Markdown => markdown.invokes_python(path, &line),
            TextKind::Other => false,
        };
        if active_invocation {
            collector.push(PythonGuardFinding {
                kind: PythonGuardFindingKind::ActivePythonInvocation,
                path: display_path.to_owned(),
                line: Some(line_number),
            });
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextKind {
    Rust,
    JavaScript,
    Shell,
    Configuration,
    Markdown,
    Other,
}

fn text_kind(path: &Path) -> TextKind {
    let extension = path.extension().and_then(OsStr::to_str).unwrap_or_default();
    match extension {
        "rs" => TextKind::Rust,
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => TextKind::JavaScript,
        "sh" | "bash" | "zsh" | "fish" | "service" => TextKind::Shell,
        "toml" | "yaml" | "yml" | "json" | "jsonc" | "ini" | "cfg" => TextKind::Configuration,
        "md" | "mdx" => TextKind::Markdown,
        _ if is_shell_filename(path.file_name().and_then(OsStr::to_str).unwrap_or_default()) => {
            TextKind::Shell
        }
        _ => TextKind::Other,
    }
}

fn is_scannable_text_path(path: &Path) -> bool {
    text_kind(path) != TextKind::Other
        || path
            .components()
            .any(|component| component.as_os_str() == OsStr::new("scripts"))
}

fn is_shell_filename(name: &str) -> bool {
    matches!(
        name,
        "Makefile"
            | "GNUmakefile"
            | "Justfile"
            | "justfile"
            | "Dockerfile"
            | "Containerfile"
            | "Taskfile"
    )
}

#[derive(Default)]
struct RustScanState {
    lexical: RustLexicalState,
}

impl RustScanState {
    fn invokes_python(&mut self, line: &str) -> bool {
        let code = self.lexical.code_mask(line);
        find_code_pattern(line, &code, "Command::new")
            .and_then(|start| first_literal_argument_after(&line[start..], "Command::new"))
            .is_some_and(executable_is_python)
            || rust_argv_invokes_python(line, &code)
    }
}

#[derive(Default)]
enum RustLexicalState {
    #[default]
    Code,
    BlockComment(usize),
    Quoted {
        quote: u8,
        escaped: bool,
    },
    RawString {
        hashes: usize,
    },
}

impl RustLexicalState {
    fn code_mask(&mut self, line: &str) -> Vec<bool> {
        let bytes = line.as_bytes();
        let mut mask = vec![false; bytes.len()];
        let mut index = 0usize;
        while index < bytes.len() {
            match self {
                Self::Code => {
                    if bytes[index..].starts_with(b"//") {
                        break;
                    }
                    if bytes[index..].starts_with(b"/*") {
                        *self = Self::BlockComment(1);
                        index += 2;
                        continue;
                    }
                    if let Some((length, hashes)) = rust_raw_string_opener(bytes, index) {
                        *self = Self::RawString { hashes };
                        index += length;
                        continue;
                    }
                    if matches!(bytes[index], b'b' | b'c') && bytes.get(index + 1) == Some(&b'"') {
                        *self = Self::Quoted {
                            quote: b'"',
                            escaped: false,
                        };
                        index += 2;
                        continue;
                    }
                    if bytes[index] == b'"' {
                        *self = Self::Quoted {
                            quote: b'"',
                            escaped: false,
                        };
                        index += 1;
                        continue;
                    }
                    mask[index] = true;
                    index += 1;
                }
                Self::BlockComment(depth) => {
                    if bytes[index..].starts_with(b"/*") {
                        *depth += 1;
                        index += 2;
                    } else if bytes[index..].starts_with(b"*/") {
                        *depth -= 1;
                        index += 2;
                        if *depth == 0 {
                            *self = Self::Code;
                        }
                    } else {
                        index += 1;
                    }
                }
                Self::Quoted { quote, escaped } => {
                    if *escaped {
                        *escaped = false;
                    } else if bytes[index] == b'\\' {
                        *escaped = true;
                    } else if bytes[index] == *quote {
                        *self = Self::Code;
                    }
                    index += 1;
                }
                Self::RawString { hashes } => {
                    if bytes[index] == b'"'
                        && bytes
                            .get(index + 1..index + 1 + *hashes)
                            .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
                    {
                        index += 1 + *hashes;
                        *self = Self::Code;
                    } else {
                        index += 1;
                    }
                }
            }
        }
        mask
    }
}

fn rust_raw_string_opener(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut index = start;
    if bytes.get(index) == Some(&b'b') {
        index += 1;
    }
    if bytes.get(index) != Some(&b'r') {
        return None;
    }
    index += 1;
    let hash_start = index;
    while bytes.get(index) == Some(&b'#') {
        index += 1;
    }
    (bytes.get(index) == Some(&b'"')).then_some((index + 1 - start, index - hash_start))
}

fn find_code_pattern(line: &str, mask: &[bool], pattern: &str) -> Option<usize> {
    line.match_indices(pattern).find_map(|(start, _)| {
        mask.get(start..start + pattern.len())
            .is_some_and(|range| range.iter().all(|is_code| *is_code))
            .then_some(start)
    })
}

fn rust_argv_invokes_python(line: &str, code: &[bool]) -> bool {
    let Some(start) = find_code_pattern(line, code, "vec![") else {
        return false;
    };
    let rest = line[start + "vec![".len()..].trim_start();
    let Some(quote) = rest.as_bytes().first().copied() else {
        return false;
    };
    if !matches!(quote, b'\'' | b'"') {
        return false;
    }
    let body = &rest[1..];
    let Some(end) = body.as_bytes().iter().position(|byte| *byte == quote) else {
        return false;
    };
    executable_is_python(&body[..end])
}

fn javascript_invokes_python(line: &str) -> bool {
    [
        "spawn(",
        "spawnSync(",
        "execFile(",
        "execFileSync(",
        "execa(",
        "Bun.spawn(",
    ]
    .iter()
    .filter_map(|call| first_literal_argument_after(line, call.trim_end_matches('(')))
    .any(executable_is_python)
}

fn first_literal_argument_after<'a>(line: &'a str, call: &str) -> Option<&'a str> {
    let start = line.find(call)? + call.len();
    let rest = line[start..].trim_start();
    let rest = rest.strip_prefix('(')?.trim_start();
    let quote = rest.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let body = &rest[1..];
    let end = body.as_bytes().iter().position(|byte| *byte == quote)?;
    Some(&body[..end])
}

fn shell_line_invokes_python(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
        return false;
    }
    shell_command_invokes_python(trimmed)
}

#[derive(Default)]
struct ConfigurationState {
    block: Option<ConfigurationBlock>,
}

enum ConfigurationBlock {
    Indented { parent_indent: usize },
    Array,
}

impl ConfigurationState {
    fn invokes_python(&mut self, line: &str) -> bool {
        let trimmed = line.trim_start();
        let indentation = line.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            return false;
        }

        if let Some(block) = &self.block {
            let remains_in_block = match block {
                ConfigurationBlock::Indented { parent_indent } => indentation > *parent_indent,
                ConfigurationBlock::Array => true,
            };
            if remains_in_block {
                let candidate = trimmed.strip_prefix("- ").unwrap_or(trimmed).trim_start();
                let invoked = shell_command_invokes_python(candidate);
                if matches!(block, ConfigurationBlock::Array) && trimmed.contains(']') {
                    self.block = None;
                }
                return invoked;
            }
            self.block = None;
        }

        let command_line = trimmed.strip_prefix("- ").unwrap_or(trimmed).trim_start();
        let lowered = trimmed.to_ascii_lowercase();
        let command_value = [
            "command",
            "run",
            "script",
            "exec",
            "entrypoint",
            "args",
            "cmd",
        ]
        .iter()
        .find_map(|key| value_after_config_key(command_line, key));
        let Some(command_value) = command_value else {
            return lowered.starts_with("execstart=")
                && shell_command_invokes_python(trimmed.split_once('=').map_or("", |row| row.1));
        };
        let value = command_value.trim_start();
        if matches!(value, "" | "|" | ">" | "|-" | ">-" | "|+" | ">+") {
            self.block = Some(ConfigurationBlock::Indented {
                parent_indent: indentation,
            });
            return false;
        }
        if value.starts_with('[') && !value.contains(']') {
            self.block = Some(ConfigurationBlock::Array);
        }
        shell_command_invokes_python(value)
    }
}

fn value_after_config_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    let quoted_double = format!("\"{key}\"");
    let quoted_single = format!("'{key}'");
    let rest = trimmed
        .strip_prefix(key)
        .or_else(|| trimmed.strip_prefix(&quoted_double))
        .or_else(|| trimmed.strip_prefix(&quoted_single))?;
    if rest.starts_with(|character: char| character.is_ascii_alphanumeric() || character == '_') {
        return None;
    }
    let rest = rest.trim_start();
    rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))
}

fn is_ci_path(path: &Path) -> bool {
    let normalized = normalized_path(path);
    normalized.starts_with(".github/workflows/")
        || normalized.starts_with(".github/actions/")
        || normalized == ".gitlab-ci.yml"
        || normalized == ".gitlab-ci.yaml"
        || normalized == "azure-pipelines.yml"
        || normalized == "azure-pipelines.yaml"
        || normalized == "bitbucket-pipelines.yml"
        || normalized == ".circleci/config.yml"
        || normalized.starts_with(".buildkite/")
}

fn is_ci_python_setup(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with("//") {
        return false;
    }
    let lowered = trimmed.to_ascii_lowercase();
    lowered.contains("actions/setup-python")
        || lowered.contains("setup-python@")
        || lowered.starts_with("image: python:")
        || lowered.contains(" image: python:")
        || lowered.contains("uv python install")
}

#[derive(Default)]
struct MarkdownState {
    fence: Option<Fence>,
    external_repository_section: bool,
}

struct Fence {
    marker: char,
    length: usize,
    shell_like: bool,
}

impl MarkdownState {
    fn invokes_python(&mut self, path: &Path, line: &str) -> bool {
        let trimmed = line.trim_start();
        if let Some((marker, length, info)) = fence_start(trimmed) {
            if let Some(fence) = &self.fence {
                if fence.marker == marker && length >= fence.length {
                    self.fence = None;
                    return false;
                }
            } else {
                let language = info
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                self.fence = Some(Fence {
                    marker,
                    length,
                    shell_like: language.is_empty()
                        || matches!(
                            language.as_str(),
                            "bash"
                                | "sh"
                                | "shell"
                                | "console"
                                | "zsh"
                                | "fish"
                                | "toml"
                                | "yaml"
                                | "yml"
                        ),
                });
                return false;
            }
        }

        if trimmed.starts_with('#') {
            let heading = trimmed.trim_start_matches('#').trim().to_ascii_lowercase();
            self.external_repository_section = heading.contains("external repository")
                || heading.contains("other repository")
                || heading.contains("governed repository example");
            return false;
        }

        if is_historical_document(path)
            || is_external_repository_example_document(path)
            || self.external_repository_section
        {
            return false;
        }
        if self.fence.as_ref().is_some_and(|fence| fence.shell_like) {
            return shell_line_invokes_python(trimmed);
        }

        markdown_inline_active_command(trimmed)
    }
}

fn fence_start(line: &str) -> Option<(char, usize, &str)> {
    let marker = line.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let length = line
        .chars()
        .take_while(|character| *character == marker)
        .count();
    if length < 3 {
        return None;
    }
    Some((marker, length, &line[length..]))
}

fn is_historical_document(path: &Path) -> bool {
    let normalized = normalized_path(path);
    normalized == "DecisionHistory.md"
        || normalized == "SKILL_AUDIT.md"
        || normalized.starts_with("UserIssueLedgers/")
        || normalized.starts_with("docs/acceptance-")
        || normalized == "docs/holy-skills-snapshot.md"
        || normalized == "docs/legacy-deletion-map.md"
}

fn is_external_repository_example_document(path: &Path) -> bool {
    normalized_path(path) == "docs/repository-config.md"
}

fn markdown_inline_active_command(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    let imperative = lowered.starts_with("run ")
        || lowered.starts_with("execute ")
        || lowered.starts_with("invoke ")
        || lowered.starts_with("use ")
        || lowered.starts_with("- run ")
        || lowered.starts_with("- execute ")
        || lowered.starts_with("- invoke ")
        || lowered.starts_with("- use ")
        || lowered.contains(" then run ")
        || lowered.contains(" must run ")
        || lowered.contains(" should run ");
    if !imperative {
        return false;
    }
    inline_code_spans(line).any(shell_command_invokes_python)
}

fn inline_code_spans(line: &str) -> impl Iterator<Item = &str> {
    let mut spans = Vec::new();
    let mut remaining = line;
    while let Some(start) = remaining.find('`') {
        remaining = &remaining[start + 1..];
        let Some(end) = remaining.find('`') else {
            break;
        };
        spans.push(&remaining[..end]);
        remaining = &remaining[end + 1..];
    }
    spans.into_iter()
}

fn shell_command_invokes_python(line: &str) -> bool {
    if command_substitutions(line).any(shell_command_invokes_python) {
        return true;
    }
    let tokens = shell_tokens(line);
    let mut command_position = true;
    let mut wrapper = WrapperState::None;
    for token in tokens {
        match token {
            ShellToken::Operator => {
                command_position = true;
                wrapper = WrapperState::None;
            }
            ShellToken::Word(word) if command_position => {
                let word = trim_command_punctuation(&word);
                if word.is_empty() || is_assignment(word) || is_shell_control_word(word) {
                    continue;
                }
                if executable_is_python(word) {
                    return true;
                }
                let basename = executable_basename(word).to_ascii_lowercase();
                match wrapper {
                    WrapperState::None if is_transparent_wrapper(&basename) => continue,
                    WrapperState::None
                        if matches!(basename.as_str(), "uv" | "poetry" | "pipenv") =>
                    {
                        wrapper = WrapperState::AwaitRun;
                        continue;
                    }
                    WrapperState::AwaitRun if basename == "run" => {
                        wrapper = WrapperState::Running;
                        continue;
                    }
                    WrapperState::AwaitRun if word.starts_with('-') => continue,
                    WrapperState::Running if word.starts_with('-') => continue,
                    WrapperState::Running => {
                        if executable_is_python(word) {
                            return true;
                        }
                    }
                    _ => {}
                }
                command_position = false;
            }
            ShellToken::Word(_) => {}
        }
    }
    false
}

fn command_substitutions(line: &str) -> impl Iterator<Item = &str> {
    let mut commands = Vec::new();
    let mut remaining = line;
    while let Some(start) = remaining.find("$(") {
        remaining = &remaining[start + 2..];
        let end = remaining.find(')').unwrap_or(remaining.len());
        commands.push(&remaining[..end]);
        remaining = &remaining[end..];
    }
    commands.into_iter()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WrapperState {
    None,
    AwaitRun,
    Running,
}

#[derive(Debug, Eq, PartialEq)]
enum ShellToken {
    Word(String),
    Operator,
}

fn shell_tokens(line: &str) -> Vec<ShellToken> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;

    let push_word = |tokens: &mut Vec<ShellToken>, word: &mut String| {
        if !word.is_empty() {
            tokens.push(ShellToken::Word(std::mem::take(word)));
        }
    };

    for character in line.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(current) = quote {
            if character == current {
                quote = None;
            } else {
                word.push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
        } else if character.is_whitespace() || matches!(character, '[' | ']' | ',' | '(' | ')') {
            push_word(&mut tokens, &mut word);
        } else if matches!(character, ';' | '|' | '&') {
            push_word(&mut tokens, &mut word);
            if !matches!(tokens.last(), Some(ShellToken::Operator)) {
                tokens.push(ShellToken::Operator);
            }
        } else if matches!(character, ':' | '=') && word.is_empty() {
            // YAML/TOML command values and shell labels introduce a command position.
            push_word(&mut tokens, &mut word);
        } else {
            word.push(character);
        }
    }
    push_word(&mut tokens, &mut word);
    tokens
}

fn shell_words(line: &str) -> Vec<String> {
    shell_tokens(line)
        .into_iter()
        .filter_map(|token| match token {
            ShellToken::Word(word) => Some(word),
            ShellToken::Operator => None,
        })
        .collect()
}

fn trim_command_punctuation(word: &str) -> &str {
    word.trim_matches(|character: char| {
        matches!(character, '@' | '!' | '$' | '{' | '}' | ':' | '=')
    })
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn is_shell_control_word(word: &str) -> bool {
    matches!(
        word,
        "if" | "then" | "elif" | "else" | "while" | "until" | "do" | "time"
    )
}

fn is_transparent_wrapper(word: &str) -> bool {
    matches!(word, "env" | "sudo" | "command" | "exec" | "nohup") || word.starts_with('-')
}

fn executable_is_python(word: &str) -> bool {
    let executable = executable_basename(word).to_ascii_lowercase();
    executable == "python"
        || executable == "python3"
        || executable.strip_prefix("python3.").is_some_and(|version| {
            !version.is_empty() && version.chars().all(|c| c.is_ascii_digit())
        })
        || executable == "pytest"
        || executable == "ruff"
        || executable == "compileall"
}

fn executable_basename(word: &str) -> &str {
    word.trim_end_matches(['\r', '\n'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(word)
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn normalized_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn bounded_path(path: &Path) -> String {
    truncate_utf8(&normalized_path(path), MAX_REPORTED_PATH_BYTES)
}

fn bounded_display_path(path: &Path) -> String {
    truncate_utf8(&path.to_string_lossy(), MAX_REPORTED_PATH_BYTES)
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

fn trim_ascii(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |position| position + 1);
    &value[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_REPOSITORY: AtomicU64 = AtomicU64::new(0);

    struct TestRepository {
        root: PathBuf,
    }

    impl TestRepository {
        fn new() -> Self {
            let nonce = NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "devcoordinator2-python-guard-{}-{now}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create test repository");
            let status = Command::new("git")
                .arg("init")
                .arg("--quiet")
                .arg(&root)
                .status()
                .expect("invoke git init");
            assert!(status.success());
            Self { root }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create fixture parent");
            }
            fs::write(path, contents).expect("write fixture");
        }

        fn track_all(&self) {
            let status = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["add", "--all"])
                .status()
                .expect("invoke git add");
            assert!(status.success());
        }

        fn inspect(&self) -> PythonGuardReport {
            self.track_all();
            inspect_repository(&self.root).expect("guard succeeds")
        }
    }

    impl Drop for TestRepository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn allows_exact_fixture_inventory_and_parses_all_seven() {
        let repository = TestRepository::new();
        repository.write("README.md", "# Rust-only repository\n");
        for fixture in ALLOWED_INERT_PYTHON_FIXTURES {
            repository.write(fixture, "def example(value):\n    return value + 1\n");
        }

        let report = repository.inspect();

        assert!(report.is_clean(), "{:#?}", report.findings);
        assert_eq!(report.inert_python_fixtures, 7);
        assert_eq!(report.tracked_files, 8);
    }

    #[test]
    fn ignored_and_untracked_files_do_not_enter_the_git_inventory() {
        let repository = TestRepository::new();
        repository.write("README.md", "# Rust-only repository\n");
        repository.track_all();
        repository.write("scratch.py", "print('untracked scratch file')\n");

        let report = inspect_repository(&repository.root).expect("guard succeeds");

        assert!(report.is_clean(), "{:#?}", report.findings);
        assert_eq!(report.tracked_files, 1);
    }

    #[test]
    fn rejects_every_non_allowlisted_python_file_and_invalid_allowed_source() {
        let repository = TestRepository::new();
        repository.write("scripts/tool.py", "print('active')\n");
        repository.write(
            "skills/full-repo-audit/evals/marker-free/score.py",
            "print('active scorer')\n",
        );
        repository.write(
            "skills/full-repo-audit/evals/marker-free/self_test.py",
            "print('active self-test')\n",
        );
        repository.write(
            ALLOWED_INERT_PYTHON_FIXTURES[0],
            "def syntactically_broken(:\n    pass\n",
        );

        let report = repository.inspect();

        assert_eq!(report.total_findings, 4);
        assert!(report.findings.iter().any(|finding| {
            finding.kind == PythonGuardFindingKind::UnexpectedPythonFile
                && finding.path == "scripts/tool.py"
        }));
        assert!(report.findings.iter().any(|finding| {
            finding.kind == PythonGuardFindingKind::InvalidInertPythonFixture
                && finding.path == ALLOWED_INERT_PYTHON_FIXTURES[0]
        }));
        for active_tool in [
            "skills/full-repo-audit/evals/marker-free/score.py",
            "skills/full-repo-audit/evals/marker-free/self_test.py",
        ] {
            assert!(report.findings.iter().any(|finding| {
                finding.kind == PythonGuardFindingKind::UnexpectedPythonFile
                    && finding.path == active_tool
            }));
        }
    }

    #[test]
    fn catches_shebangs_commands_manifests_and_ci_setup() {
        let repository = TestRepository::new();
        repository.write("bin/review", "#!/usr/bin/env python3\nprint('review')\n");
        repository.write("scripts/check.sh", "#!/bin/sh\nuv run --locked pytest -q\n");
        repository.write(
            "config/tool.toml",
            "command = [\n  \".venv/bin/ruff\",\n  \"check\",\n]\n",
        );
        repository.write("pyproject.toml", "[project]\nname = \"old-tool\"\n");
        repository.write(
            ".github/workflows/check.yml",
            "steps:\n  - uses: actions/setup-python@v5\n  - run: |\n      python3 scripts/check.py\n",
        );
        repository.write(
            "rust/active.rs",
            "let child = std::process::Command::new(\"compileall\").spawn()?;\n",
        );
        repository.write(
            "rust/argv.rs",
            "fn command() -> Vec<String> { vec![\"python3\".into(), \"-c\".into()] }\n",
        );

        let report = repository.inspect();

        for kind in [
            PythonGuardFindingKind::PythonShebang,
            PythonGuardFindingKind::ActivePythonInvocation,
            PythonGuardFindingKind::PythonProjectManifest,
            PythonGuardFindingKind::CiPythonSetup,
        ] {
            assert!(
                report.findings.iter().any(|finding| finding.kind == kind),
                "missing {kind:?}: {:#?}",
                report.findings
            );
        }
        assert_eq!(
            report
                .findings
                .iter()
                .filter(|finding| finding.kind == PythonGuardFindingKind::ActivePythonInvocation)
                .count(),
            5
        );
    }

    #[test]
    fn permits_terminology_history_fixture_strings_and_external_examples() {
        let repository = TestRepository::new();
        repository.write(
            "skills/coverage/SKILL.md",
            "Accept LCOV, Cobertura, and coverage.py terminology.\n",
        );
        repository.write(
            "DecisionHistory.md",
            "The former release ran `python3 scripts/install.py` and pytest.\n",
        );
        repository.write(
            "rust/fixture.rs",
            "const CROSS_LANGUAGE: &str = r###\"\nCommand::new(\"python3\");\nvec![\"pytest\", \"-q\"];\ncommand = [\"python3\", \"-m\", \"pytest\"]\n\"###;\n",
        );
        repository.write(
            "docs/repository-config.md",
            "# Repository Configuration\n\n```toml\ncommand = [\"python3\", \"-m\", \"pytest\"]\n```\n",
        );

        let report = repository.inspect();

        assert!(report.is_clean(), "{:#?}", report.findings);
    }

    #[test]
    fn active_markdown_instructions_are_not_treated_as_history() {
        let repository = TestRepository::new();
        repository.write(
            "README.md",
            "Run the verifier:\n\n```bash\npython3 scripts/verify.py\n```\n",
        );
        repository.write(
            "skills/check/SKILL.md",
            "- Run `pytest -q` before reporting success.\n",
        );

        let report = repository.inspect();

        assert_eq!(
            report
                .findings
                .iter()
                .filter(|finding| finding.kind == PythonGuardFindingKind::ActivePythonInvocation)
                .count(),
            2,
            "{:#?}",
            report.findings
        );
    }

    #[test]
    fn report_api_writes_source_free_artifact_and_bounded_receipt() {
        let repository = TestRepository::new();
        repository.write(
            "scripts/secret-tool.py",
            "SECRET_SOURCE_VALUE = 'never-return-this'\n",
        );
        repository.track_all();
        let report_file = repository.root.join("artifacts/python-free.json");

        let receipt = inspect_repository_to_report(&repository.root, &report_file)
            .expect("write guard report");
        let artifact = fs::read_to_string(&report_file).expect("read report artifact");
        let receipt_json = receipt.to_json().to_string();

        assert_eq!(receipt.status, PythonGuardStatus::Findings);
        assert!(artifact.contains("secret-tool.py"));
        assert!(!artifact.contains("SECRET_SOURCE_VALUE"));
        assert!(!receipt_json.contains("SECRET_SOURCE_VALUE"));
        assert_eq!(receipt.total_findings, 1);
    }

    #[test]
    fn command_parser_distinguishes_arguments_from_commands() {
        assert!(shell_command_invokes_python("CI=1 python3 -m pytest"));
        assert!(shell_command_invokes_python(
            "cargo check && .venv/bin/ruff check"
        ));
        assert!(shell_command_invokes_python("uv run --locked pytest -q"));
        assert!(shell_command_invokes_python("exec compileall src"));
        assert!(shell_command_invokes_python("value=$(python3 tool.py)"));
        assert!(!shell_command_invokes_python(
            "echo python3 scripts/example.py"
        ));
        assert!(!shell_command_invokes_python("echo coverage.py"));
        assert!(!shell_command_invokes_python("node verify.mjs"));
    }

    #[test]
    fn findings_are_bounded_while_the_total_remains_truthful() {
        let repository = TestRepository::new();
        for index in 0..(MAX_REPORTED_FINDINGS + 5) {
            repository.write(&format!("legacy/tool-{index}.py"), "pass\n");
        }

        let report = repository.inspect();

        assert_eq!(report.total_findings, MAX_REPORTED_FINDINGS + 5);
        assert_eq!(report.findings.len(), MAX_REPORTED_FINDINGS);
        assert!(report.findings_truncated);
    }
}
