//! Deterministic repository inventory and batching for all Rust audit builders.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use globset::GlobBuilder;
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::audit_ledger::read_bytes_nofollow;
use crate::audit_ledger::{validate_directory_nofollow, write_bytes_nofollow};

pub const DEFAULT_MAX_BATCH_BYTES: usize = 60_000;
pub const DIR_EXCLUSION_SAMPLE_LIMIT: usize = 20;
pub const DIR_EXCLUSION_COUNT_LIMIT: usize = 100;
pub const ARTIFACT_MARKER: &str = ".full-repo-audit-artifacts.json";
pub const ARTIFACT_OWNER: &str = "full-repo-audit";
pub const VERIFICATION_RECEIPT_NAME: &str = "verification_receipt.json";
const KNOWN_GENERATED_ARTIFACTS: &[&str] = &[
    "audit_complete.json",
    "audit_complete.json.tmp",
    "audit_index.md",
    "completion-ledger-database-import.json",
    "completion-ledger-plan.json",
    "completion_ledger_projection.json",
    "consolidated-findings.json",
    "consolidated-findings.md",
    "effort_ledger.json",
    "excluded_files.json",
    "journey_audit.md",
    "lead_reconciliation.md",
    "final-report.md",
    "logs",
    "manifest.json",
    "queue_complete.json",
    "queue_complete.json.tmp",
    "test_evidence_index.json",
    VERIFICATION_RECEIPT_NAME,
    "visual_evidence.json",
    "visual_journey_audit.md",
];

pub const UI_ASSET_EXTENSIONS: &[&str] = &[
    ".avif", ".bmp", ".eot", ".gif", ".ico", ".jpeg", ".jpg", ".png", ".ttf", ".webp", ".woff",
    ".woff2",
];
pub const UI_ASSET_DIRS: &[&str] = &[
    "appiconset",
    "asset",
    "assets",
    "brand",
    "branding",
    "font",
    "fonts",
    "icon",
    "icons",
    "image",
    "images",
    "img",
    "media",
    "public",
    "screenshot",
    "screenshots",
    "static",
];
pub const UI_ASSET_NAME_TOKENS: &[&str] = &[
    "appicon",
    "avatar",
    "background",
    "banner",
    "brand",
    "favicon",
    "hero",
    "icon",
    "logo",
    "screenshot",
    "sprite",
];

const SOURCE_EXTENSIONS: &[&str] = &[
    ".axaml",
    ".astro",
    ".bash",
    ".c",
    ".cc",
    ".cljs",
    ".clj",
    ".cpp",
    ".cs",
    ".cshtml",
    ".cts",
    ".css",
    ".cxx",
    ".dart",
    ".ejs",
    ".erl",
    ".ex",
    ".exs",
    ".fish",
    ".fs",
    ".fsx",
    ".go",
    ".gradle",
    ".gql",
    ".graphql",
    ".groovy",
    ".h",
    ".handlebars",
    ".hbs",
    ".hh",
    ".hpp",
    ".hrl",
    ".html",
    ".j2",
    ".java",
    ".jinja",
    ".jinja2",
    ".js",
    ".jsx",
    ".kt",
    ".kts",
    ".less",
    ".liquid",
    ".lua",
    ".m",
    ".mdx",
    ".mjs",
    ".mm",
    ".mts",
    ".mustache",
    ".njk",
    ".php",
    ".pl",
    ".pm",
    ".prisma",
    ".proto",
    ".ps1",
    ".pug",
    ".py",
    ".r",
    ".razor",
    ".rb",
    ".rs",
    ".sass",
    ".scala",
    ".scss",
    ".sh",
    ".sql",
    ".storyboard",
    ".svelte",
    ".svg",
    ".swift",
    ".tf",
    ".tfvars",
    ".tpl",
    ".ts",
    ".tsx",
    ".twig",
    ".vb",
    ".vue",
    ".xaml",
    ".xib",
    ".zsh",
];
const CONFIG_EXTENSIONS: &[&str] = &[
    ".cjs",
    ".conf",
    ".csproj",
    ".editorconfig",
    ".fsproj",
    ".ini",
    ".json",
    ".jsonc",
    ".pbxproj",
    ".props",
    ".sln",
    ".targets",
    ".toml",
    ".vbproj",
    ".xcconfig",
    ".xaml",
    ".xml",
    ".yaml",
    ".yml",
];
const SOURCE_FILENAMES: &[&str] = &[
    ".babelrc",
    ".dockerignore",
    ".editorconfig",
    ".env.example",
    ".env.local.example",
    ".eslintrc",
    ".eslintrc.cjs",
    ".eslintrc.js",
    ".eslintrc.json",
    ".gitattributes",
    ".gitignore",
    ".gitmodules",
    ".node-version",
    ".npmignore",
    ".npmrc",
    ".nvmrc",
    ".prettierrc",
    ".prettierrc.cjs",
    ".prettierrc.js",
    ".prettierrc.json",
    ".python-version",
    ".ruby-version",
    ".tool-versions",
    "Brewfile",
    "Capfile",
    "Cargo.toml",
    "CMakeLists.txt",
    "compose.yaml",
    "compose.yml",
    "deno.json",
    "deno.jsonc",
    "Directory.Build.props",
    "Directory.Build.targets",
    "Directory.Packages.props",
    "Dockerfile",
    "docker-compose.yaml",
    "docker-compose.yml",
    "Gemfile",
    "global.json",
    "go.mod",
    "go.sum",
    "Guardfile",
    "justfile",
    "Justfile",
    "Makefile",
    "mix.exs",
    "package.json",
    "Pipfile",
    "pom.xml",
    "Procfile",
    "pyproject.toml",
    "Rakefile",
    "requirements.txt",
    "setup.cfg",
    "setup.py",
    "tsconfig.json",
    "turbo.json",
    "Vagrantfile",
    "vite.config.js",
    "vite.config.mjs",
    "vite.config.ts",
    "webpack.config.js",
];
const LOCK_FILENAMES: &[&str] = &[
    "bun.lock",
    "bun.lockb",
    "Cargo.lock",
    "composer.lock",
    "flake.lock",
    "Gemfile.lock",
    "package-lock.json",
    "Pipfile.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "uv.lock",
    "yarn.lock",
];
const SOURCE_MARKDOWN_FILENAMES: &[&str] = &[
    "AGENTS.md",
    "API.md",
    "ARCHITECTURE.md",
    "CLAUDE.md",
    "CONTRIBUTING.md",
    "DESIGN.md",
    "GEMINI.md",
    "PRD.md",
    "PRODUCT.md",
    "README.md",
    "REQUIREMENTS.md",
    "ROADMAP.md",
    "RUNBOOK.md",
    "SECURITY.md",
    "SKILL.md",
    "SPEC.md",
    "USER_STORIES.md",
    "UX.md",
];
const SOURCE_MARKDOWN_DIRS: &[&str] = &["docs", "documentation", "guides", "prompts", "references"];
const SOURCE_SCRIPT_DIRS: &[&str] = &[".husky", "hooks", "script", "scripts", "tools"];
const SOURCE_SUFFIXES: &[&str] = &[
    ".config.cjs",
    ".config.js",
    ".config.mjs",
    ".config.ts",
    ".d.ts",
    ".module.css",
    ".module.scss",
    ".spec.jsx",
    ".spec.tsx",
    ".stories.jsx",
    ".stories.tsx",
    ".test.jsx",
    ".test.tsx",
];
const MESSAGE_CATALOG_EXTENSIONS: &[&str] = &[
    ".arb",
    ".ftl",
    ".po",
    ".pot",
    ".properties",
    ".resx",
    ".strings",
    ".xlf",
    ".xliff",
];
const MESSAGE_CATALOG_CONFIG_EXTENSIONS: &[&str] = &[".json", ".jsonc", ".yaml", ".yml"];
const GENERATED_DIRS: &[&str] = &[
    ".build",
    ".next",
    ".nuxt",
    ".parcel-cache",
    ".svelte-kit",
    ".swiftpm",
    ".terraform",
    ".turbo",
    "bin",
    "build",
    "coverage",
    "DerivedData",
    "dist",
    "obj",
    "out",
    "target",
    "tmp",
];
const GENERATED_FILE_SUFFIXES: &[&str] = &[".tsbuildinfo"];
const VENDOR_DIRS: &[&str] = &["bower_components", "node_modules", "Pods", "vendor"];
const TOOLING_DIRS: &[&str] = &[
    ".cache",
    ".codex",
    ".dart_tool",
    ".git",
    ".gradle",
    ".idea",
    ".pytest_cache",
    ".ruff_cache",
    ".venv",
    ".vscode",
    "__pycache__",
    "venv",
];
const HIDDEN_PROJECT_DIRS: &[&str] = &[
    ".changeset",
    ".claude",
    ".devcontainer",
    ".github",
    ".gitlab",
    ".husky",
    ".storybook",
    ".well-known",
];
const FIRST_PARTY_HIDDEN_PROJECT_PARENT_DIRS: &[&str] = &[
    "app", "apps", "backend", "client", "engine", "frontend", "lib", "libs", "package", "packages",
    "server", "service", "services", "src",
];
const EXCLUDED_FILENAMES: &[&str] = &[".DS_Store"];
const ENV_EXAMPLE_MARKERS: &[&str] = &[
    ".dist",
    ".dist.json",
    ".example",
    ".example.json",
    ".sample",
    ".sample.json",
    ".schema",
    ".schema.json",
    ".template",
    ".template.json",
];
const ENV_EXAMPLE_BASENAMES: &[&str] = &["example", "sample", "template"];
const ENV_EXAMPLE_TOKEN_MARKERS: &[&str] = &["dist", "example", "sample", "schema", "template"];
const BINARY_EXTENSIONS: &[&str] = &[
    ".a", ".avif", ".bmp", ".class", ".dll", ".dmg", ".eot", ".exe", ".gif", ".gz", ".ico", ".jar",
    ".jpeg", ".jpg", ".mov", ".mp3", ".mp4", ".o", ".pdf", ".png", ".so", ".sqlite", ".ttf",
    ".wasm", ".webm", ".webp", ".woff", ".woff2", ".zip",
];
const INTERFACE_EXTENSIONS: &[&str] = &[
    ".axaml",
    ".astro",
    ".cshtml",
    ".css",
    ".ejs",
    ".handlebars",
    ".hbs",
    ".html",
    ".j2",
    ".jinja",
    ".jinja2",
    ".jsx",
    ".less",
    ".liquid",
    ".mdx",
    ".mustache",
    ".njk",
    ".pug",
    ".razor",
    ".sass",
    ".scss",
    ".storyboard",
    ".svelte",
    ".svg",
    ".tpl",
    ".tsx",
    ".twig",
    ".vue",
    ".xaml",
    ".xib",
];
const ANDROID_INTERFACE_DIRS: &[&str] = &["layout", "menu", "navigation"];
const INTERFACE_PATH_PARTS: &[&str] = &[
    "client",
    "components",
    "frontend",
    "layouts",
    "pages",
    "screens",
    "templates",
    "ui",
    "views",
    "web",
    "widgets",
];
const NON_INTERFACE_SOURCE_PARTS: &[&str] = &[
    "__tests__",
    "e2e",
    "fixtures",
    "scripts",
    "test",
    "tests",
    "tools",
];
const INTERFACE_TEXT_PARTS: &[&str] = &[
    "i18n",
    "lang",
    "locale",
    "locales",
    "messages",
    "translations",
];
const INTERFACE_NAME_TOKENS: &[&str] = &[
    "button", "checkbox", "command", "dialog", "drawer", "dropdown", "field", "form", "input",
    "menu", "modal", "nav", "page", "popover", "screen", "select", "sidebar", "tab", "toast",
    "toolbar", "tooltip", "view",
];
const INTERFACE_KEY_MARKERS: &[&str] = &[
    "aria-label",
    "command",
    "default_prompt",
    "description",
    "display_name",
    "empty_state",
    "error_message",
    "helper_text",
    "label",
    "menu",
    "placeholder",
    "short_description",
    "success_message",
    "title",
    "toast",
    "tooltip",
];
const HIGH_SIGNAL_SOURCE_DIRS: &[&str] = &[
    ".github",
    ".gitlab",
    "android",
    "app",
    "apps",
    "backend",
    "client",
    "config",
    "frontend",
    "ios",
    "lib",
    "mobile",
    "packages",
    "server",
    "src",
    "test",
    "tests",
    "templates",
];

static RUN_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{7,127}$").expect("constant run-id regex")
});
static STATIC_CONFIG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[a-z0-9_-]+\.)?config\.(?:[cm]?[jt]s|json|jsonc)$")
        .expect("constant static-config regex")
});
static CAMEL_BOUNDARY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([a-z0-9])([A-Z])").expect("constant camel regex"));
static WORD_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^A-Za-z0-9]+").expect("constant word regex"));
const HIGH_RISK_PATH_TOKENS: &[&str] = &[
    "auth",
    "authorization",
    "backup",
    "billing",
    "coordinator",
    "crypto",
    "database",
    "deploy",
    "docker",
    "migration",
    "payment",
    "permission",
    "restore",
    "security",
    "server",
    "worker",
];
const HIGH_RISK_CODE_SUFFIXES: &[&str] = &[
    "", ".c", ".cc", ".cpp", ".cs", ".go", ".java", ".js", ".jsx", ".kt", ".kts", ".m", ".mm",
    ".php", ".py", ".rb", ".rs", ".sh", ".swift", ".ts", ".tsx",
];
const TEST_PATH_PARTS: &[&str] = &["__tests__", "spec", "specs", "test", "tests"];
static HIGH_RISK_SOURCE_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            r"\b(?:subprocess\.(?:Popen|run|call)|child_process|Runtime\.getRuntime\(\)\.exec|ProcessBuilder)\b",
            "process execution",
        ),
        (
            r"\bshell\s*=\s*True\b|\beval\s*\(|\bexec\s*\(",
            "dynamic or shell execution",
        ),
        (
            r"(?i)\b(?:DROP|TRUNCATE|DELETE\s+FROM|pg_restore|pg_dump|pg_dumpall)\b",
            "destructive or backup database operation",
        ),
        (
            r"(?i)\b(?:password|secret|access[_-]?token|private[_-]?key)\b",
            "credential or secret handling",
        ),
    ]
    .into_iter()
    .map(|(expression, reason)| {
        (
            Regex::new(expression).expect("constant high-risk regex"),
            reason,
        )
    })
    .collect()
});
static TEST_NAME_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r#"\b(?:async\s+)?def\s+(test_[A-Za-z0-9_]+)\s*\("#,
        r#"\bfunc\s+(test[A-Za-z0-9_]+)\s*\("#,
        r#"\b(?:function\s+)?(test[A-Za-z0-9_]+)\s*\("#,
        r#"\b(?:describe|it|test)\s*\(\s*['\"]([^'\"]{1,160})['\"]"#,
    ]
    .into_iter()
    .map(|expression| Regex::new(expression).expect("constant test-name regex"))
    .collect()
});

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileEntry {
    pub rel_path: String,
    pub size_bytes: usize,
    pub kind: String,
    pub interface_relevant: bool,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuditUnit {
    pub unit_id: String,
    pub rel_path: String,
    pub size_bytes: usize,
    pub kind: String,
    pub interface_relevant: bool,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_byte: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_byte: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct CollectOptions {
    pub include_config: bool,
    pub include_env: bool,
    pub include_generated: bool,
    pub include_vendor: bool,
    pub include_assets: bool,
    pub exclude_globs: Vec<String>,
    pub include_files: BTreeSet<String>,
    pub include_globs: Vec<String>,
    pub output_rel_dirs: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct FileCollection {
    pub entries: Vec<FileEntry>,
    pub excluded: Vec<Value>,
    pub tracked_deletions: Vec<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactOwnership {
    pub marker_name: String,
    pub owner: String,
    pub known_generated_artifacts: BTreeSet<String>,
}

impl Default for ArtifactOwnership {
    fn default() -> Self {
        Self {
            marker_name: ARTIFACT_MARKER.to_owned(),
            owner: ARTIFACT_OWNER.to_owned(),
            known_generated_artifacts: KNOWN_GENERATED_ARTIFACTS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        }
    }
}

fn suffix(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_lowercase()))
        .unwrap_or_default()
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

pub fn run_id_token(value: &str) -> Result<String, String> {
    if RUN_ID_RE.is_match(value) {
        Ok(value.to_owned())
    } else {
        Err("must be 8-128 characters using letters, numbers, '.', '_', or '-'".to_owned())
    }
}

pub fn validate_markdown_safe_token(value: &str, field_name: &str) -> Result<(), String> {
    if value != value.trim() {
        return Err(format!(
            "{field_name} must not have leading or trailing whitespace."
        ));
    }
    if value.chars().any(|character| {
        let code = u32::from(character);
        code < 32 || code == 127
    }) {
        return Err(format!(
            "{field_name} must not contain ASCII control characters."
        ));
    }
    let unsafe_characters = ['|', '`']
        .into_iter()
        .filter(|character| value.contains(*character))
        .collect::<Vec<_>>();
    if !unsafe_characters.is_empty() {
        return Err(format!(
            "{field_name} must not contain Markdown table/code delimiters: {unsafe_characters:?}."
        ));
    }
    Ok(())
}

pub fn validate_repo_relative_path_token(value: &str, field_name: &str) -> Result<(), String> {
    validate_markdown_safe_token(value, field_name)?;
    if value.contains('\\') {
        return Err(format!("{field_name} must use POSIX repo-relative paths."));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || value.is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "{field_name} must be a repo-relative path without '.' or '..' segments."
        ));
    }
    Ok(())
}

pub fn artifact_delivery_contract(report_path: &Path) -> Result<String, String> {
    let report = report_path
        .canonicalize()
        .unwrap_or_else(|_| report_path.to_path_buf());
    let report_text = report.to_string_lossy();
    validate_markdown_safe_token(&report_text, "worker report path")?;
    let report_name = report_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "worker report filename is not UTF-8".to_owned())?;
    validate_markdown_safe_token(report_name, "worker report filename")?;
    Ok(format!(
        "## Artifact Delivery Contract\n\nWrite the complete required Markdown report directly to `{report_text}`. The\nreport file is the result of record; do not paste or repeat its body in the\nsubagent response. Writing this audit artifact does not authorize editing the\naudited repository.\n\nAfter the file is durably written, compute its SHA-256 and byte length, then\nrespond with only this compact receipt:\n\n`REPORT_SAVED filename={report_name}; status=<complete|blocked> sha256=<64-hex> bytes=<integer> findings=<integer-or-unknown>`\n\nKeep the receipt at or below 80 tokens. If the report cannot be written, return\nonly `REPORT_NOT_SAVED path=<exact-report-path>; reason=<short-reason>`, still\nat or below 80 tokens. Never substitute the full report body for a failed\nartifact write.\n"
    ))
}

pub fn isolated_light_worker_contract() -> &'static str {
    "## Dispatch Contract\n\nThe lead must pass this entire generated prompt plus applicable project-ledger\nrequirements. Spawn this worker in a fresh isolated context with the\nruntime/user-selected effort. Never rely on an inherited lead transcript.\n"
}

pub fn lead_artifact_delivery_contract(report_path: &Path) -> Result<String, String> {
    let report = report_path
        .canonicalize()
        .unwrap_or_else(|_| report_path.to_path_buf());
    let report_text = report.to_string_lossy();
    validate_markdown_safe_token(&report_text, "lead report path")?;
    Ok(format!(
        "## Artifact Delivery Contract\n\nWrite the complete lead-owned reconciliation directly to `{report_text}`.\nKeep this detailed artifact out of routine chat context; inspect it through\ntargeted sections and let the verifier validate it. Writing this artifact does\nnot authorize editing the audited repository.\n"
    ))
}

pub fn is_local_tooling_file(rel_path: &str) -> bool {
    let parts = Path::new(rel_path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    parts.len() >= 2
        && parts[..parts.len() - 1].contains(&".claude")
        && parts.last() == Some(&"settings.local.json")
}

pub fn is_env_example_file(name: &str) -> bool {
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if extension == "env" && ENV_EXAMPLE_BASENAMES.contains(&stem.to_lowercase().as_str()) {
        return true;
    }
    if name.contains(".env.") {
        let tokens = name
            .split('.')
            .filter(|part| !part.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        if tokens
            .iter()
            .any(|token| ENV_EXAMPLE_TOKEN_MARKERS.contains(&token.as_str()))
        {
            return true;
        }
    }
    name.contains(".env.")
        && ENV_EXAMPLE_MARKERS
            .iter()
            .any(|marker| name.ends_with(marker))
}

pub fn is_secret_env_file(name: &str) -> bool {
    !is_env_example_file(name)
        && (name == ".envrc"
            || name == ".env"
            || name.ends_with(".env")
            || (name.contains(".env.")
                && !ENV_EXAMPLE_MARKERS
                    .iter()
                    .any(|marker| name.ends_with(marker))))
}

pub fn is_source_markdown(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    if suffix(path) != ".md" {
        return false;
    }
    let parts = path.components().collect::<Vec<_>>();
    if parts.len() == 1 {
        return true;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if SOURCE_MARKDOWN_FILENAMES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
    {
        return true;
    }
    path.parent().is_some_and(|parent| {
        parent.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|part| SOURCE_MARKDOWN_DIRS.contains(&part.to_lowercase().as_str()))
        })
    })
}

fn path_parent_parts(rel_path: &str) -> Vec<String> {
    Path::new(rel_path)
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| component.as_os_str().to_str())
        .map(str::to_lowercase)
        .collect()
}

fn is_message_catalog_path(rel_path: &str, file_suffix: &str) -> bool {
    if MESSAGE_CATALOG_EXTENSIONS.contains(&file_suffix) {
        return true;
    }
    MESSAGE_CATALOG_CONFIG_EXTENSIONS.contains(&file_suffix)
        && path_parent_parts(rel_path)
            .iter()
            .any(|part| INTERFACE_TEXT_PARTS.contains(&part.as_str()))
}

pub fn classify(rel_path: &str, include_config: bool, include_env: bool) -> Option<&'static str> {
    let path = Path::new(rel_path);
    let name = path.file_name()?.to_str()?;
    let lower_name = name.to_lowercase();
    let file_suffix = suffix(path);
    if !include_env && is_secret_env_file(name) {
        None
    } else if is_source_markdown(rel_path) {
        Some("source/contract")
    } else if is_env_example_file(name) {
        Some("source/config")
    } else if include_env && is_secret_env_file(name) {
        Some("config")
    } else if LOCK_FILENAMES.contains(&name)
        || SOURCE_FILENAMES.contains(&name)
        || lower_name == "dockerfile"
        || lower_name.starts_with("dockerfile.")
    {
        Some("source/config")
    } else if is_message_catalog_path(rel_path, &file_suffix) {
        Some("source/message-catalog")
    } else if SOURCE_EXTENSIONS.contains(&file_suffix.as_str())
        || SOURCE_SUFFIXES
            .iter()
            .any(|item| lower_name.ends_with(item))
    {
        Some("source")
    } else if include_config
        && (CONFIG_EXTENSIONS.contains(&file_suffix.as_str())
            || SOURCE_FILENAMES.contains(&name)
            || name.contains(".env.")
            || name.ends_with(".schema.json"))
    {
        Some("config")
    } else {
        None
    }
}

pub fn filename_words(name: &str) -> BTreeSet<String> {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let spaced = CAMEL_BOUNDARY_RE.replace_all(stem, "$1 $2");
    WORD_SPLIT_RE
        .split(&spaced)
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub fn is_ui_asset_path(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    if !UI_ASSET_EXTENSIONS.contains(&suffix(path).as_str()) {
        return false;
    }
    path_parent_parts(rel_path)
        .iter()
        .any(|part| UI_ASSET_DIRS.contains(&part.as_str()))
        || filename_words(
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(""),
        )
        .iter()
        .any(|word| UI_ASSET_NAME_TOKENS.contains(&word.as_str()))
}

fn read_initial_bytes(path: &Path, limit: usize) -> Vec<u8> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    let Ok(file) = file else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    let _ = file.take(limit as u64).read_to_end(&mut bytes);
    bytes
}

pub fn is_text_like_sample(sample: &[u8]) -> bool {
    if sample.contains(&0) {
        return false;
    }
    if sample.is_empty() {
        return true;
    }
    if std::str::from_utf8(sample).is_err() {
        return false;
    }
    let controls = sample
        .iter()
        .filter(|byte| **byte < 32 && !b"\n\r\t\x0c\x08".contains(byte))
        .count();
    (controls as f64 / sample.len() as f64) < 0.01
}

pub fn is_extensionless_script_candidate(rel_path: &str, path: &Path) -> bool {
    let rel = Path::new(rel_path);
    if rel.extension().is_some() {
        return false;
    }
    let sample = read_initial_bytes(path, 8192);
    is_text_like_sample(&sample)
        && (sample.starts_with(b"#!")
            || path_parent_parts(rel_path)
                .iter()
                .any(|part| SOURCE_SCRIPT_DIRS.contains(&part.as_str())))
}

pub fn is_high_signal_unknown(rel_path: &str, path: &Path) -> bool {
    let rel = Path::new(rel_path);
    let parts = path_parent_parts(rel_path);
    let name = rel
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let lower_name = name.to_lowercase();
    let top_level_operational = rel.components().count() == 1
        && (name.starts_with('.')
            || (rel.extension().is_none()
                && (lower_name.ends_with("file") || lower_name.ends_with("rc"))));
    if !top_level_operational
        && !parts.iter().any(|part| {
            HIGH_SIGNAL_SOURCE_DIRS.contains(&part.as_str())
                || SOURCE_SCRIPT_DIRS.contains(&part.as_str())
                || HIDDEN_PROJECT_DIRS.contains(&part.as_str())
        })
    {
        return false;
    }
    is_text_like_sample(&read_initial_bytes(path, 8192))
}

fn has_interface_key_markers(path: &Path) -> bool {
    let file_suffix = suffix(path);
    if !CONFIG_EXTENSIONS.contains(&file_suffix.as_str()) && file_suffix != ".md" {
        return false;
    }
    let text = String::from_utf8_lossy(&read_initial_bytes(path, 1_000_000)).into_owned();
    INTERFACE_KEY_MARKERS.iter().any(|key| {
        Regex::new(&format!(
            r#"(?i)(^|[\"'\s_-]){}[\"'\s_-]*[:=]"#,
            regex::escape(key)
        ))
        .expect("escaped interface-key regex")
        .is_match(&text)
    })
}

pub fn is_interface_file(rel_path: &str, fs_path: &Path) -> bool {
    let rel = Path::new(rel_path);
    let parts = path_parent_parts(rel_path);
    let file_suffix = suffix(rel);
    if is_ui_asset_path(rel_path) || INTERFACE_EXTENSIONS.contains(&file_suffix.as_str()) {
        return true;
    }
    if file_suffix == ".xml"
        && parts.contains(&"res".to_owned())
        && parts
            .iter()
            .any(|part| ANDROID_INTERFACE_DIRS.contains(&part.as_str()))
    {
        return true;
    }
    if is_message_catalog_path(rel_path, &file_suffix) {
        return true;
    }
    let name = rel
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if LOCK_FILENAMES.contains(&name) || SOURCE_FILENAMES.contains(&name) {
        return false;
    }
    if STATIC_CONFIG_RE.is_match(&name.to_lowercase()) {
        return false;
    }
    if parts
        .iter()
        .any(|part| NON_INTERFACE_SOURCE_PARTS.contains(&part.as_str()))
    {
        return has_interface_key_markers(fs_path);
    }
    let in_interface_text = parts
        .iter()
        .any(|part| INTERFACE_TEXT_PARTS.contains(&part.as_str()));
    if in_interface_text
        && (CONFIG_EXTENSIONS.contains(&file_suffix.as_str())
            || SOURCE_EXTENSIONS.contains(&file_suffix.as_str())
            || MESSAGE_CATALOG_EXTENSIONS.contains(&file_suffix.as_str()))
    {
        return true;
    }
    if parts
        .iter()
        .any(|part| INTERFACE_PATH_PARTS.contains(&part.as_str()))
        && (SOURCE_EXTENSIONS.contains(&file_suffix.as_str())
            || CONFIG_EXTENSIONS.contains(&file_suffix.as_str())
            || MESSAGE_CATALOG_EXTENSIONS.contains(&file_suffix.as_str()))
    {
        return true;
    }
    if filename_words(name)
        .iter()
        .any(|word| INTERFACE_NAME_TOKENS.contains(&word.as_str()))
    {
        return true;
    }
    has_interface_key_markers(fs_path)
}

pub fn is_first_party_hidden_project_dir(rel_dir: &str) -> bool {
    let parts = Path::new(rel_dir)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    let Some(last) = parts.last() else {
        return false;
    };
    HIDDEN_PROJECT_DIRS.contains(last)
        && (parts.len() == 1
            || parts[..parts.len() - 1]
                .iter()
                .any(|part| FIRST_PARTY_HIDDEN_PROJECT_PARENT_DIRS.contains(part)))
}

pub fn is_excluded_dir(
    part: &str,
    include_generated: bool,
    include_vendor: bool,
    rel_dir: Option<&str>,
) -> bool {
    if HIDDEN_PROJECT_DIRS.contains(&part)
        && is_first_party_hidden_project_dir(rel_dir.unwrap_or(part))
    {
        return false;
    }
    TOOLING_DIRS.contains(&part)
        || HIDDEN_PROJECT_DIRS.contains(&part)
        || (!include_generated && GENERATED_DIRS.contains(&part))
        || (!include_vendor && VENDOR_DIRS.contains(&part))
}

pub fn excluded_dir_reason(
    part: &str,
    include_generated: bool,
    include_vendor: bool,
    rel_dir: Option<&str>,
) -> Option<String> {
    if TOOLING_DIRS.contains(&part) {
        Some(format!("excluded tooling directory: {part}"))
    } else if HIDDEN_PROJECT_DIRS.contains(&part)
        && !is_first_party_hidden_project_dir(rel_dir.unwrap_or(part))
    {
        Some(format!(
            "excluded nested hidden project directory: {}",
            rel_dir.unwrap_or(part)
        ))
    } else if GENERATED_DIRS.contains(&part) && !include_generated {
        Some(format!(
            "excluded generated/build directory: {part}; pass --include-generated to audit"
        ))
    } else if VENDOR_DIRS.contains(&part) && !include_vendor {
        Some(format!(
            "excluded vendor directory: {part}; pass --include-vendor to audit"
        ))
    } else {
        None
    }
}

fn posix_relative(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

fn walk_with_filter<F>(repo: &Path, descend: F) -> Vec<String>
where
    F: Fn(&str, &str) -> bool + Send + Sync + 'static,
{
    let root = repo.to_path_buf();
    let filter_root = root.clone();
    let mut builder = WalkBuilder::new(repo);
    builder
        .standard_filters(false)
        .follow_links(false)
        .sort_by_file_path(|left, right| left.cmp(right));
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
            return true;
        }
        let Some(rel_dir) = posix_relative(&filter_root, entry.path()) else {
            return false;
        };
        let Some(name) = entry.file_name().to_str() else {
            return false;
        };
        descend(name, &rel_dir)
    });
    builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() > 0 && entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| posix_relative(&root, entry.path()))
        .collect()
}

pub fn walk_files(repo: &Path, include_generated: bool, include_vendor: bool) -> Vec<String> {
    walk_with_filter(repo, move |name, rel_dir| {
        !is_excluded_dir(name, include_generated, include_vendor, Some(rel_dir))
    })
}

fn is_under_rel_dir(rel_path: &str, rel_dir: &str) -> bool {
    let normalized = rel_dir.trim_end_matches('/');
    !normalized.is_empty()
        && (rel_path == normalized
            || rel_path
                .strip_prefix(normalized)
                .is_some_and(|rest| rest.starts_with('/')))
}

pub fn excluded_by_output_dir(rel_path: &str, output_rel_dirs: &[String]) -> Option<String> {
    output_rel_dirs.iter().find_map(|output| {
        is_under_rel_dir(rel_path, output)
            .then(|| format!("audit output directory excluded: {output}"))
    })
}

pub fn excluded_by_dir(
    rel_path: &str,
    include_generated: bool,
    include_vendor: bool,
) -> Option<String> {
    Path::new(rel_path)
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| component.as_os_str().to_str())
        .find_map(|part| {
            if TOOLING_DIRS.contains(&part) {
                Some(format!("excluded tooling directory: {part}"))
            } else if GENERATED_DIRS.contains(&part) && !include_generated {
                Some(format!("excluded generated/build directory: {part}"))
            } else if VENDOR_DIRS.contains(&part) && !include_vendor {
                Some(format!("excluded vendor directory: {part}"))
            } else {
                None
            }
        })
}

fn is_generated_file(rel_path: &str) -> bool {
    GENERATED_FILE_SUFFIXES.contains(&suffix(Path::new(rel_path)).as_str())
}

fn excluded_generated_file_reason(rel_path: &str, include_generated: bool) -> Option<String> {
    (!include_generated && is_generated_file(rel_path)).then(|| {
        format!(
            "excluded generated/build file: {}; pass --include-generated to audit",
            suffix(Path::new(rel_path))
        )
    })
}

fn normalize_glob_pattern(pattern: &str) -> &str {
    let mut normalized = pattern;
    while let Some(value) = normalized.strip_prefix("./") {
        normalized = value;
    }
    normalized
}

pub fn glob_matches(rel_path: &str, pattern: &str) -> bool {
    fn matches(path: &str, pattern: &str) -> bool {
        GlobBuilder::new(pattern)
            .literal_separator(false)
            .backslash_escape(false)
            .build()
            .ok()
            .is_some_and(|glob| glob.compile_matcher().is_match(path))
    }
    matches(rel_path, pattern)
        || pattern
            .strip_prefix("**/")
            .is_some_and(|pattern| matches(rel_path, pattern))
}

pub fn matches_any_glob(rel_path: &str, patterns: &[String]) -> Option<String> {
    patterns.iter().find_map(|pattern| {
        glob_matches(rel_path, normalize_glob_pattern(pattern))
            .then(|| format!("matched --exclude-glob {pattern}"))
    })
}

pub fn forced_include_reason(
    rel_path: &str,
    include_files: &BTreeSet<String>,
    include_globs: &[String],
) -> Option<String> {
    if include_files.contains(rel_path) {
        return Some("matched --include-file".to_owned());
    }
    include_globs.iter().find_map(|pattern| {
        glob_matches(rel_path, normalize_glob_pattern(pattern))
            .then(|| format!("matched --include-glob {pattern}"))
    })
}

fn include_glob_explicitly_targets_dir(rel_dir: &str, include_globs: &[String]) -> bool {
    let rel_parts = rel_dir
        .trim_end_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if rel_parts.is_empty() {
        return false;
    }
    include_globs.iter().any(|pattern| {
        let mut normalized = pattern.trim();
        if let Some(value) = normalized.strip_prefix("./") {
            normalized = value;
        }
        while let Some(value) = normalized.strip_prefix("**/") {
            normalized = value;
        }
        let wildcard_at = ['*', '?', '[']
            .into_iter()
            .filter_map(|token| normalized.find(token))
            .min()
            .unwrap_or(normalized.len());
        let literal = normalized[..wildcard_at].trim_end_matches('/');
        let prefix_parts = literal
            .split('/')
            .filter(|part| !part.is_empty() && *part != "**")
            .collect::<Vec<_>>();
        !prefix_parts.is_empty()
            && (prefix_parts == rel_parts
                || (prefix_parts.len() > rel_parts.len()
                    && prefix_parts[..rel_parts.len()] == rel_parts)
                || (prefix_parts.len() < rel_parts.len()
                    && rel_parts[..prefix_parts.len()] == prefix_parts))
    })
}

fn include_glob_may_match_excluded_dir(rel_dir: &str, include_globs: &[String]) -> bool {
    let rel_parts = rel_dir.trim_end_matches('/').split('/').collect::<Vec<_>>();
    include_globs.iter().any(|pattern| {
        let normalized = normalize_glob_pattern(pattern.trim());
        let literal = normalized.split('/').collect::<Vec<_>>();
        literal[..literal.len().saturating_sub(1)]
            .iter()
            .filter(|part| {
                **part != "**" && !part.contains('*') && !part.contains('?') && !part.contains('[')
            })
            .any(|part| rel_parts.contains(part))
    })
}

fn include_glob_explicitly_targets_excluded_path(rel_path: &str, include_globs: &[String]) -> bool {
    let parts = rel_path.split('/').collect::<Vec<_>>();
    parts[..parts.len().saturating_sub(1)]
        .iter()
        .enumerate()
        .any(|(index, part)| {
            (GENERATED_DIRS.contains(part) || VENDOR_DIRS.contains(part))
                && include_glob_explicitly_targets_dir(&parts[..=index].join("/"), include_globs)
        })
}

fn walk_for_include_globs(
    repo: &Path,
    include_generated: bool,
    include_vendor: bool,
    output_rel_dirs: &[String],
    include_globs: &[String],
) -> Vec<String> {
    let outputs = output_rel_dirs.to_vec();
    let globs = include_globs.to_vec();
    walk_with_filter(repo, move |name, rel_dir| {
        if excluded_by_output_dir(rel_dir, &outputs).is_some() {
            return false;
        }
        !is_excluded_dir(name, include_generated, include_vendor, Some(rel_dir))
            || include_glob_may_match_excluded_dir(rel_dir, &globs)
            || include_glob_explicitly_targets_dir(rel_dir, &globs)
    })
}

fn is_hidden_path(rel_path: &str, include_generated: bool) -> bool {
    let parts = rel_path.split('/').collect::<Vec<_>>();
    parts[..parts.len().saturating_sub(1)]
        .iter()
        .enumerate()
        .any(|(index, part)| {
            part.starts_with('.')
                && !(HIDDEN_PROJECT_DIRS.contains(part)
                    && is_first_party_hidden_project_dir(&parts[..=index].join("/")))
                && !(include_generated && GENERATED_DIRS.contains(part))
        })
}

fn run_git(repo: &Path, arguments: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(arguments)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn nul_paths(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .collect()
}

pub fn run_git_files(repo: &Path) -> Option<Vec<String>> {
    run_git(repo, &["rev-parse", "--is-inside-work-tree"])?;
    run_git(repo, &["ls-files", "-co", "--exclude-standard", "-z"]).map(|output| nul_paths(&output))
}

pub fn run_git_deleted_paths(repo: &Path) -> Option<Vec<String>> {
    let mut paths = BTreeSet::new();
    for arguments in [
        [
            "diff",
            "--name-only",
            "--diff-filter=D",
            "--no-renames",
            "-z",
        ]
        .as_slice(),
        [
            "diff",
            "--cached",
            "--name-only",
            "--diff-filter=D",
            "--no-renames",
            "-z",
        ]
        .as_slice(),
    ] {
        paths.extend(nul_paths(&run_git(repo, arguments)?));
    }
    Some(paths.into_iter().collect())
}

fn git_deleted_path_evidence(repo: &Path, rel_path: &str) -> Value {
    for revision in [format!(":{rel_path}"), format!("HEAD:{rel_path}")] {
        if let Some(bytes) = run_git(repo, &["show", &revision]) {
            return json!({
                "path":rel_path,"baseline_sha256":sha256_hex(&bytes),
                "baseline_size_bytes":bytes.len(),"baseline_available":true,
            });
        }
    }
    json!({
        "path":rel_path,"baseline_sha256":Value::Null,
        "baseline_size_bytes":Value::Null,"baseline_available":false,
    })
}

fn excluded_pathspecs(
    include_generated: bool,
    include_vendor: bool,
    output_rel_dirs: &[String],
) -> Vec<String> {
    let mut names = TOOLING_DIRS.iter().copied().collect::<BTreeSet<_>>();
    if !include_generated {
        names.extend(GENERATED_DIRS);
    }
    if !include_vendor {
        names.extend(VENDOR_DIRS);
    }
    let mut pathspecs = names
        .into_iter()
        .flat_map(|name| {
            [
                format!(":(exclude){name}/**"),
                format!(":(exclude)**/{name}/**"),
            ]
        })
        .collect::<Vec<_>>();
    pathspecs.extend(
        output_rel_dirs
            .iter()
            .filter(|path| !path.is_empty())
            .map(|path| format!(":(exclude){}/**", path.trim_end_matches('/'))),
    );
    pathspecs
}

pub fn run_git_ignored_files(
    repo: &Path,
    include_generated: bool,
    include_vendor: bool,
    output_rel_dirs: &[String],
) -> Vec<String> {
    let pathspecs = excluded_pathspecs(include_generated, include_vendor, output_rel_dirs);
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "-i", "-o", "--exclude-standard", "-z", "--"])
        .args(pathspecs);
    command
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| nul_paths(&output.stdout))
        .unwrap_or_default()
}

fn walk_secret_env_files(
    repo: &Path,
    include_generated: bool,
    include_vendor: bool,
) -> Vec<String> {
    walk_files(repo, include_generated, include_vendor)
        .into_iter()
        .filter(|path| {
            Path::new(path)
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(is_secret_env_file)
        })
        .collect()
}

fn has_dir_part(rel_path: &str, names: &[&str]) -> bool {
    Path::new(rel_path)
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| component.as_os_str().to_str())
        .any(|part| names.contains(&part))
}

fn walk_requested_extra_files(
    repo: &Path,
    include_env: bool,
    include_generated: bool,
    include_vendor: bool,
) -> Vec<String> {
    walk_files(repo, include_generated, include_vendor)
        .into_iter()
        .filter(|rel_path| {
            let name = Path::new(rel_path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            (include_env && is_secret_env_file(name))
                || (include_generated
                    && (has_dir_part(rel_path, GENERATED_DIRS) || is_generated_file(rel_path)))
                || (include_vendor && has_dir_part(rel_path, VENDOR_DIRS))
        })
        .collect()
}

fn is_high_signal_ignored(
    rel_path: &str,
    path: &Path,
    include_config: bool,
    include_env: bool,
) -> bool {
    if is_ui_asset_path(rel_path) {
        return true;
    }
    let parts = path_parent_parts(rel_path);
    if !parts.iter().any(|part| {
        HIGH_SIGNAL_SOURCE_DIRS.contains(&part.as_str())
            || SOURCE_SCRIPT_DIRS.contains(&part.as_str())
            || HIDDEN_PROJECT_DIRS.contains(&part.as_str())
    }) {
        return false;
    }
    classify(rel_path, include_config, include_env).is_some()
        || is_extensionless_script_candidate(rel_path, path)
        || is_high_signal_unknown(rel_path, path)
}

fn should_warn_excluded(
    rel_path: &str,
    path: &Path,
    reason: Option<&str>,
    include_config: bool,
    include_env: bool,
) -> bool {
    let Some(reason) = reason else {
        return false;
    };
    if reason == "not source-like" {
        return is_high_signal_unknown(rel_path, path);
    }
    if reason.starts_with("audit output directory excluded")
        || reason.starts_with("secret-bearing env file")
        || reason == "local Claude settings excluded"
        || reason.starts_with("excluded generated/build directory")
        || reason.starts_with("excluded generated/build file")
        || reason.starts_with("excluded vendor directory")
    {
        return false;
    }
    if matches!(
        reason,
        "binary/static asset extension" | "binary file content"
    ) {
        return is_ui_asset_path(rel_path);
    }
    classify(rel_path, include_config, include_env).is_some() || is_interface_file(rel_path, path)
}

fn directory_entries(path: &Path) -> Vec<std::fs::DirEntry> {
    let mut entries = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    entries
}

fn summarized_pruned_directory(
    repo: &Path,
    directory: &Path,
    rel_path: &str,
    reason: &str,
    options: &CollectOptions,
) -> Value {
    let mut file_count = 0usize;
    let mut capped = false;
    let mut sample_paths = Vec::new();
    let mut source_like_paths = Vec::new();
    let mut source_like_count = 0usize;
    let mut unresolved_source_like_count = 0usize;
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let mut child_directories = Vec::new();
        for entry in directory_entries(&current) {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                let child_path = entry.path();
                let Some(child_rel) = posix_relative(repo, &child_path) else {
                    continue;
                };
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !TOOLING_DIRS.contains(&name.as_str())
                    && excluded_dir_reason(
                        &name,
                        options.include_generated,
                        options.include_vendor,
                        Some(&child_rel),
                    )
                    .is_none()
                {
                    child_directories.push(child_path);
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            if file_count >= DIR_EXCLUSION_COUNT_LIMIT {
                capped = true;
                stack.clear();
                break;
            }
            file_count += 1;
            let child_path = entry.path();
            let Some(child_rel) = posix_relative(repo, &child_path) else {
                continue;
            };
            if sample_paths.len() < DIR_EXCLUSION_SAMPLE_LIMIT {
                sample_paths.push(child_rel.clone());
            }
            if reason.starts_with("excluded generated/build directory")
                || reason.starts_with("excluded vendor directory")
            {
                let source_like = classify(&child_rel, options.include_config, options.include_env)
                    .is_some()
                    || is_extensionless_script_candidate(&child_rel, &child_path)
                    || is_ui_asset_path(&child_rel);
                if source_like {
                    source_like_count += 1;
                    if forced_include_reason(
                        &child_rel,
                        &options.include_files,
                        &options.include_globs,
                    )
                    .is_none()
                    {
                        unresolved_source_like_count += 1;
                        if source_like_paths.len() < DIR_EXCLUSION_SAMPLE_LIMIT {
                            source_like_paths.push(child_rel);
                        }
                    }
                }
            }
        }
        child_directories.reverse();
        stack.extend(child_directories);
    }
    let mut summary = json!({
        "path":rel_path,"reason":reason,"size_bytes":0,"scope_warning":false,
        "entry_type":"directory","file_count":file_count,"file_count_capped":capped,
        "scan_file_limit":DIR_EXCLUSION_COUNT_LIMIT,"sample_paths":sample_paths,
    });
    let object = summary.as_object_mut().expect("JSON object");
    if source_like_count > 0 {
        if capped {
            object.insert(
                "source_like_observed_count".to_owned(),
                json!(source_like_count),
            );
            object.insert("source_like_count_capped".to_owned(), json!(true));
        } else {
            object.insert(
                "source_like_total_count".to_owned(),
                json!(source_like_count),
            );
            object.insert("source_like_count_capped".to_owned(), json!(false));
        }
    }
    if unresolved_source_like_count > 0 {
        object.extend([
            ("contains_source_like_samples".to_owned(), json!(true)),
            (
                "source_like_sample_count".to_owned(),
                json!(unresolved_source_like_count),
            ),
            (
                "source_like_sample_count_capped".to_owned(),
                json!(capped),
            ),
            (
                "source_like_sample_paths".to_owned(),
                json!(source_like_paths),
            ),
            (
                "review_hint".to_owned(),
                json!("Pruned directory contains source-like samples from a bounded scan; pass the relevant include flag or --include-file/--include-glob if these are first-party files."),
            ),
        ]);
    }
    summary
}

fn summarize_pruned_dirs(repo: &Path, options: &CollectOptions) -> Vec<Value> {
    fn visit(repo: &Path, current: &Path, options: &CollectOptions, output: &mut Vec<Value>) {
        for entry in directory_entries(current) {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            let Some(rel_path) = posix_relative(repo, &path) else {
                continue;
            };
            if excluded_by_output_dir(&rel_path, &options.output_rel_dirs).is_some() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if HIDDEN_PROJECT_DIRS.contains(&name.as_str())
                && is_first_party_hidden_project_dir(&rel_path)
            {
                visit(repo, &path, options, output);
                continue;
            }
            if let Some(reason) = excluded_dir_reason(
                &name,
                options.include_generated,
                options.include_vendor,
                Some(&rel_path),
            ) {
                output.push(summarized_pruned_directory(
                    repo, &path, &rel_path, &reason, options,
                ));
            } else {
                visit(repo, &path, options, output);
            }
        }
    }
    let mut summaries = Vec::new();
    visit(repo, repo, options, &mut summaries);
    summaries
}

fn stable_bytes(repo: &Path, path: &Path) -> Result<Vec<u8>, String> {
    read_bytes_nofollow(path, Some(repo))
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "file disappeared during scan".to_owned())
}

fn process_file(
    repo: &Path,
    rel_path: &str,
    force_reason: Option<&str>,
    mut reason: Option<String>,
    options: &CollectOptions,
) -> Result<FileEntry, Value> {
    let path = repo.join(rel_path);
    let metadata = path.symlink_metadata();
    let size = path
        .metadata()
        .map_or(0, |metadata| metadata.len() as usize);
    if reason.is_none() {
        if !path.exists() {
            reason = Some("path does not exist".to_owned());
        } else if !path.is_file() {
            reason = Some("not a regular file".to_owned());
        } else if metadata.is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            reason = Some("symlink skipped".to_owned());
        }
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if reason.is_none() && EXCLUDED_FILENAMES.contains(&name) {
        reason = Some("excluded filename".to_owned());
    }
    if reason.is_none() && is_local_tooling_file(rel_path) {
        reason = Some("local Claude settings excluded".to_owned());
    }
    if reason.is_none() && !options.include_env && is_secret_env_file(name) {
        reason = Some(
            "secret-bearing env file excluded; pass --include-env to audit intentionally"
                .to_owned(),
        );
    }
    let mut kind = None;
    if reason.is_none() && BINARY_EXTENSIONS.contains(&suffix(&path).as_str()) {
        if options.include_assets && is_ui_asset_path(rel_path) {
            kind = Some("source/ui-asset");
        } else {
            reason = Some("binary/static asset extension".to_owned());
        }
    }
    if reason.is_none()
        && is_hidden_path(rel_path, options.include_generated)
        && force_reason.is_none()
    {
        reason = Some("hidden tooling directory".to_owned());
    }
    if reason.is_none()
        && kind.is_none()
        && options.include_generated
        && is_generated_file(rel_path)
    {
        kind = Some("generated/build metadata");
    }
    if reason.is_none() && kind.is_none() {
        kind = classify(rel_path, options.include_config, options.include_env);
    }
    if reason.is_none() && kind.is_none() && is_extensionless_script_candidate(rel_path, &path) {
        kind = Some("source/script");
    }
    if reason.is_none() && kind.is_none() && force_reason.is_some() {
        kind = Some("source/manual");
    }
    if reason.is_none() && kind.is_none() {
        reason = Some("not source-like".to_owned());
    }
    let bytes = if reason.is_none() {
        match stable_bytes(repo, &path) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                reason = Some(format!("file became unreadable during scan: {error}"));
                None
            }
        }
    } else {
        None
    };
    if reason.is_none()
        && kind != Some("source/ui-asset")
        && bytes.as_ref().is_some_and(|bytes| bytes.contains(&0))
    {
        reason = Some("binary file content".to_owned());
    }
    if let (None, Some(kind), Some(bytes)) = (&reason, kind, bytes) {
        return Ok(FileEntry {
            rel_path: rel_path.to_owned(),
            size_bytes: size,
            kind: kind.to_owned(),
            interface_relevant: is_interface_file(rel_path, &path),
            sha256: sha256_hex(&bytes),
        });
    }
    let reason_text = reason.unwrap_or_else(|| "not source-like".to_owned());
    Err(json!({
        "path":rel_path,"reason":reason_text,"size_bytes":size,
        "scope_warning":should_warn_excluded(
            rel_path,&path,Some(&reason_text),options.include_config,options.include_env
        ),
    }))
}

pub fn collect_files(repo: &Path, options: &CollectOptions) -> FileCollection {
    let mut git_ignored_paths = BTreeSet::new();
    let mut deleted_tracked_paths = BTreeSet::new();
    let glob_forced_paths = if options.include_globs.is_empty() {
        BTreeSet::new()
    } else {
        walk_for_include_globs(
            repo,
            options.include_generated,
            options.include_vendor,
            &options.output_rel_dirs,
            &options.include_globs,
        )
        .into_iter()
        .filter(|rel_path| {
            forced_include_reason(rel_path, &BTreeSet::new(), &options.include_globs).is_some()
        })
        .collect()
    };
    let git_paths = run_git_files(repo);
    let mut rel_paths = if let Some(git_paths) = git_paths {
        deleted_tracked_paths.extend(run_git_deleted_paths(repo).unwrap_or_default());
        git_ignored_paths.extend(run_git_ignored_files(
            repo,
            options.include_generated,
            options.include_vendor,
            &options.output_rel_dirs,
        ));
        let mut paths = git_paths.into_iter().collect::<BTreeSet<_>>();
        paths.extend(walk_secret_env_files(
            repo,
            options.include_generated,
            options.include_vendor,
        ));
        paths.extend(options.include_files.iter().cloned());
        paths.extend(glob_forced_paths);
        if options.include_assets {
            paths.extend(
                git_ignored_paths
                    .iter()
                    .filter(|path| is_ui_asset_path(path))
                    .cloned(),
            );
        }
        if options.include_env || options.include_generated || options.include_vendor {
            paths.extend(walk_requested_extra_files(
                repo,
                options.include_env,
                options.include_generated,
                options.include_vendor,
            ));
        }
        paths
    } else {
        let mut paths = walk_files(repo, options.include_generated, options.include_vendor)
            .into_iter()
            .collect::<BTreeSet<_>>();
        paths.extend(options.include_files.iter().cloned());
        paths.extend(glob_forced_paths);
        paths
    };

    let mut collection = FileCollection {
        excluded: summarize_pruned_dirs(repo, options),
        ..Default::default()
    };
    for rel_path in &rel_paths {
        let force_reason =
            forced_include_reason(rel_path, &options.include_files, &options.include_globs);
        let mut reason = excluded_by_output_dir(rel_path, &options.output_rel_dirs)
            .or_else(|| {
                excluded_by_dir(rel_path, options.include_generated, options.include_vendor)
            })
            .or_else(|| excluded_generated_file_reason(rel_path, options.include_generated))
            .or_else(|| matches_any_glob(rel_path, &options.exclude_globs));
        if force_reason.is_some()
            && reason
                .as_deref()
                .is_some_and(|reason| !reason.starts_with("audit output directory excluded"))
            && (options.include_files.contains(rel_path)
                || !reason.as_deref().is_some_and(|reason| {
                    (reason.starts_with("excluded generated/build directory")
                        || reason.starts_with("excluded vendor directory"))
                        && !include_glob_explicitly_targets_excluded_path(
                            rel_path,
                            &options.include_globs,
                        )
                }))
        {
            reason = None;
        }
        let path = repo.join(rel_path);
        if reason.is_none() && deleted_tracked_paths.contains(rel_path) && !path.exists() {
            collection
                .tracked_deletions
                .push(git_deleted_path_evidence(repo, rel_path));
            continue;
        }
        match process_file(repo, rel_path, force_reason.as_deref(), reason, options) {
            Ok(entry) => collection.entries.push(entry),
            Err(excluded) => collection.excluded.push(excluded),
        }
    }

    for rel_path in git_ignored_paths.difference(&rel_paths) {
        let path = repo.join(rel_path);
        if excluded_by_output_dir(rel_path, &options.output_rel_dirs).is_some()
            || excluded_by_dir(rel_path, options.include_generated, options.include_vendor)
                .is_some()
            || excluded_generated_file_reason(rel_path, options.include_generated).is_some()
        {
            continue;
        }
        let Ok(metadata) = path.symlink_metadata() else {
            continue;
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| EXCLUDED_FILENAMES.contains(&name))
        {
            continue;
        }
        if is_local_tooling_file(rel_path) {
            collection.excluded.push(json!({
                "path":rel_path,"reason":"local Claude settings excluded",
                "size_bytes":metadata.len(),"scope_warning":false,
            }));
            continue;
        }
        if is_high_signal_ignored(rel_path, &path, options.include_config, options.include_env) {
            collection.excluded.push(json!({
                "path":rel_path,"reason":"gitignored source-like file excluded by gitignore",
                "size_bytes":metadata.len(),"scope_warning":true,
            }));
        }
    }
    collection.tracked_deletions.sort_by(|left, right| {
        left.get("path")
            .and_then(Value::as_str)
            .cmp(&right.get("path").and_then(Value::as_str))
    });
    rel_paths.clear();
    collection
}

fn line_range_units_for(
    repo: &Path,
    path: &Path,
    max_unit_bytes: usize,
) -> Option<Vec<(usize, usize, usize)>> {
    let data = read_bytes_nofollow(path, Some(repo)).ok().flatten()?;
    let lines = data
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    if lines.len() <= 1 || lines.iter().any(|line| line.len() > max_unit_bytes) {
        return None;
    }
    let mut chunks = Vec::new();
    let mut start_line = 1usize;
    let mut current_bytes = 0usize;
    let mut end_line = 0usize;
    for (index, line) in lines.iter().enumerate() {
        std::str::from_utf8(line).ok()?;
        let line_number = index + 1;
        if current_bytes > 0 && current_bytes + line.len() > max_unit_bytes {
            chunks.push((start_line, end_line, current_bytes));
            start_line = line_number;
            current_bytes = 0;
        }
        current_bytes += line.len();
        end_line = line_number;
    }
    if current_bytes > 0 {
        chunks.push((start_line, end_line, current_bytes));
    }
    (chunks.len() > 1).then_some(chunks)
}

pub fn audit_units_for(
    repo: &Path,
    entries: &[FileEntry],
    max_unit_bytes: usize,
) -> Vec<AuditUnit> {
    let mut units = Vec::new();
    for entry in entries {
        if entry.size_bytes <= max_unit_bytes {
            units.push(AuditUnit {
                unit_id: entry.rel_path.clone(),
                rel_path: entry.rel_path.clone(),
                size_bytes: entry.size_bytes,
                kind: entry.kind.clone(),
                interface_relevant: entry.interface_relevant,
                sha256: entry.sha256.clone(),
                start_line: None,
                end_line: None,
                start_byte: None,
                end_byte: None,
            });
            continue;
        }
        if let Some(line_units) =
            line_range_units_for(repo, &repo.join(&entry.rel_path), max_unit_bytes)
        {
            for (start, end, size) in line_units {
                units.push(AuditUnit {
                    unit_id: format!("{}#L{start}-L{end}", entry.rel_path),
                    rel_path: entry.rel_path.clone(),
                    size_bytes: size,
                    kind: entry.kind.clone(),
                    interface_relevant: entry.interface_relevant,
                    sha256: entry.sha256.clone(),
                    start_line: Some(start),
                    end_line: Some(end),
                    start_byte: None,
                    end_byte: None,
                });
            }
            continue;
        }
        let chunk_count = entry.size_bytes.div_ceil(max_unit_bytes).max(2);
        for chunk_index in 0..chunk_count {
            let start = chunk_index * max_unit_bytes + 1;
            let end = entry.size_bytes.min((chunk_index + 1) * max_unit_bytes);
            if start > end {
                continue;
            }
            units.push(AuditUnit {
                unit_id: format!("{}#B{start}-{end}", entry.rel_path),
                rel_path: entry.rel_path.clone(),
                size_bytes: end - start + 1,
                kind: entry.kind.clone(),
                interface_relevant: entry.interface_relevant,
                sha256: entry.sha256.clone(),
                start_line: None,
                end_line: None,
                start_byte: Some(start),
                end_byte: Some(end),
            });
        }
    }
    units
}

pub fn batch_files(
    entries: &[AuditUnit],
    batch_size: usize,
    max_batch_bytes: usize,
) -> Result<Vec<Vec<AuditUnit>>, String> {
    if batch_size < 1 {
        return Err("--batch-size must be at least 1".to_owned());
    }
    if max_batch_bytes < 1 {
        return Err("--max-batch-bytes must be at least 1".to_owned());
    }
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_bytes = 0usize;
    for entry in entries {
        if current.len() >= batch_size
            || (!current.is_empty() && current_bytes + entry.size_bytes > max_batch_bytes)
        {
            batches.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current.push(entry.clone());
        current_bytes += entry.size_bytes;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    Ok(batches)
}

fn language_hint(entry: &AuditUnit) -> String {
    suffix(Path::new(&entry.rel_path))
        .strip_prefix('.')
        .filter(|value| !value.is_empty())
        .unwrap_or("config")
        .to_owned()
}

fn most_common(values: impl Iterator<Item = String>, limit: usize) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for value in values {
        if let Some((_, count)) = counts.iter_mut().find(|(existing, _)| existing == &value) {
            *count += 1;
        } else {
            counts.push((value, 1));
        }
    }
    counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    counts
        .into_iter()
        .take(limit)
        .map(|(value, _)| value)
        .collect()
}

pub fn purpose_for(entries: &[AuditUnit]) -> String {
    let directories = most_common(
        entries.iter().map(|item| {
            let mut parts = item.rel_path.split('/');
            let first = parts.next().unwrap_or(".");
            if parts.next().is_some() { first } else { "." }.to_owned()
        }),
        3,
    );
    let kinds = most_common(entries.iter().map(|item| item.kind.clone()), 3);
    let languages = most_common(entries.iter().map(language_hint), 4);
    let interface_count = entries
        .iter()
        .filter(|entry| entry.interface_relevant)
        .count();
    format!(
        "Audit {} files mostly under {}; primary file types: {}.{}",
        kinds.join(", "),
        directories.join(", "),
        languages.join(", "),
        if interface_count > 0 {
            format!(" Includes {interface_count} interface-relevant file(s).")
        } else {
            String::new()
        }
    )
}

pub fn duplicate_whole_file_paths_for_batches(batches: &[Vec<AuditUnit>]) -> Vec<String> {
    let mut counts = BTreeMap::new();
    for unit in batches.iter().flatten().filter(|unit| {
        unit.start_line.is_none()
            && unit.end_line.is_none()
            && unit.start_byte.is_none()
            && unit.end_byte.is_none()
    }) {
        *counts.entry(unit.rel_path.clone()).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .filter_map(|(path, count)| (count > 1).then_some(path))
        .collect()
}

pub fn validate_generated_artifact_tokens(
    entries: &[FileEntry],
    units: &[AuditUnit],
) -> Result<(), String> {
    for (index, entry) in entries.iter().enumerate() {
        validate_repo_relative_path_token(
            &entry.rel_path,
            &format!("source_files[{index}].rel_path"),
        )?;
    }
    for (index, unit) in units.iter().enumerate() {
        validate_markdown_safe_token(&unit.unit_id, &format!("coverage_units[{index}].unit_id"))?;
        validate_repo_relative_path_token(
            &unit.rel_path,
            &format!("coverage_units[{index}].rel_path"),
        )?;
    }
    Ok(())
}

pub fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("cannot encode {}: {error}", path.display()))?;
    write_bytes_nofollow(path, &bytes, 0o600).map_err(|error| error.to_string())
}

pub fn canonical_json_sha256(value: &Value) -> Result<String, String> {
    crate::audit_findings::canonical_json_sha256(value)
}

pub fn read_ownership_marker(
    out_dir: &Path,
    ownership: &ArtifactOwnership,
) -> Option<serde_json::Map<String, Value>> {
    let path = out_dir.join(&ownership.marker_name);
    let bytes = read_bytes_nofollow(&path, Some(out_dir)).ok().flatten()?;
    crate::audit_findings::strict_json_object(&bytes, "audit ownership marker").ok()
}

pub fn ensure_output_dir_safe(
    out_dir: &Path,
    repo: &Path,
    ownership: &ArtifactOwnership,
) -> Result<Option<serde_json::Map<String, Value>>, String> {
    let metadata = match out_dir.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect output path: {error}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Output path exists but is not a non-symlinked directory: {}",
            out_dir.display()
        ));
    }
    validate_directory_nofollow(out_dir).map_err(|error| error.to_string())?;
    if directory_entries(out_dir).is_empty() {
        return Ok(None);
    }
    let marker = read_ownership_marker(out_dir, ownership);
    if let Some(marker) = marker
        && marker.get("owned_by") == Some(&json!(ownership.owner))
    {
        if marker.get("repo_root") != Some(&json!(repo.to_string_lossy())) {
            return Err(format!(
                "Output directory is marked as {}-owned for a different repo: {}",
                ownership.owner,
                out_dir.display()
            ));
        }
        return Ok(Some(marker));
    }
    Err(format!(
        "Output directory is non-empty and is not marked as {}-owned: {}. Choose a new empty directory or an existing directory created by this harness.",
        ownership.owner,
        out_dir.display()
    ))
}

pub fn write_ownership_marker(
    out_dir: &Path,
    repo: &Path,
    generated_artifacts: &[String],
    claimed_at: &str,
    ownership: &ArtifactOwnership,
) -> Result<(), String> {
    write_json(
        &out_dir.join(&ownership.marker_name),
        &json!({
            "owned_by":ownership.owner,
            "repo_root":repo.to_string_lossy(),
            "claimed_at":claimed_at,
            "generated_artifacts":generated_artifacts,
        }),
    )
}

fn previous_generated_artifacts(
    out_dir: &Path,
    marker: Option<&serde_json::Map<String, Value>>,
) -> Vec<String> {
    if let Some(items) = marker
        .and_then(|marker| marker.get("generated_artifacts"))
        .and_then(Value::as_array)
    {
        let items = items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !items.is_empty() {
            return items;
        }
    }
    let manifest_path = out_dir.join("manifest.json");
    if let Some(manifest) = read_bytes_nofollow(&manifest_path, Some(out_dir))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
    {
        let prompts = manifest
            .get("batches")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter_map(|batch| batch.get("prompt"))
            .filter_map(Value::as_str)
            .map(str::to_owned);
        let journey_prompts = manifest
            .get("journey_audit")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|journey| [journey.get("source_prompt"), journey.get("visual_prompt")])
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned);
        let lead_prompt = manifest
            .get("lead_reconciliation")
            .and_then(Value::as_object)
            .and_then(|lead| lead.get("prompt"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let archive = manifest
            .get("archived_reports_dir")
            .and_then(Value::as_str)
            .and_then(|path| relative_dir_if_child(out_dir, Path::new(path)));
        return [
            "audit_complete.json",
            "audit_index.md",
            "effort_ledger.json",
            "excluded_files.json",
            "manifest.json",
            "queue_complete.json",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain(lead_prompt)
        .chain(journey_prompts)
        .chain(archive)
        .chain(prompts)
        .collect();
    }
    let batch_re = Regex::new(r"^batch_\d{3}\.md$").expect("constant batch regex");
    [
        "audit_complete.json",
        "audit_index.md",
        "effort_ledger.json",
        "excluded_files.json",
        "manifest.json",
        "queue_complete.json",
        "journey_audit.md",
        "visual_journey_audit.md",
        "lead_reconciliation.md",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(directory_entries(out_dir).into_iter().filter_map(|entry| {
        let name = entry.file_name().to_str()?.to_owned();
        (entry.file_type().ok()?.is_file() && batch_re.is_match(&name)).then_some(name)
    }))
    .collect()
}

fn is_safe_generated_artifact_name(name: &str) -> bool {
    !name.is_empty()
        && !Path::new(name).is_absolute()
        && Path::new(name) != Path::new(".")
        && !name.contains('\\')
        && !Path::new(name).components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
}

fn has_symlinked_parent(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    let mut current = root.to_path_buf();
    for component in relative
        .components()
        .take(relative.components().count().saturating_sub(1))
    {
        current.push(component.as_os_str());
        if current
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return true;
        }
    }
    false
}

fn generated_artifact_path_is_safe(out_dir: &Path, path: &Path) -> bool {
    if has_symlinked_parent(path, out_dir) {
        return false;
    }
    let Ok(root) = validate_directory_nofollow(out_dir) else {
        return false;
    };
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .is_some_and(|parent| parent.starts_with(&root));
    }
    path.canonicalize()
        .or_else(|_| {
            path.parent()
                .unwrap_or(out_dir)
                .canonicalize()
                .map(|parent| parent.join(path.file_name().unwrap_or_default()))
        })
        .is_ok_and(|resolved| resolved != root && resolved.starts_with(&root))
}

pub fn clean_generated_artifacts(
    out_dir: &Path,
    marker: Option<&serde_json::Map<String, Value>>,
    ownership: &ArtifactOwnership,
) -> Result<Vec<PathBuf>, String> {
    let mut names = previous_generated_artifacts(out_dir, marker)
        .into_iter()
        .collect::<BTreeSet<_>>();
    names.extend(ownership.known_generated_artifacts.iter().cloned());
    let batch_re = Regex::new(r"^batch_\d{3,}\.md$").expect("constant batch regex");
    let stale_re = Regex::new(r"^reports\.stale\.\d{8}T\d{6}Z(?:\.\d+)?$")
        .expect("constant stale-report regex");
    for entry in directory_entries(out_dir) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if batch_re.is_match(&name) || stale_re.is_match(&name) {
            names.insert(name);
        }
    }
    let mut removed = Vec::new();
    for name in names {
        if !is_safe_generated_artifact_name(&name) {
            continue;
        }
        let path = out_dir.join(&name);
        if !generated_artifact_path_is_safe(out_dir, &path) {
            continue;
        }
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot inspect generated artifact: {error}")),
        };
        if metadata.file_type().is_symlink() || metadata.is_file() {
            std::fs::remove_file(&path)
                .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
        } else if metadata.is_dir() {
            validate_directory_nofollow(&path).map_err(|error| error.to_string())?;
            std::fs::remove_dir_all(&path)
                .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
        } else {
            continue;
        }
        removed.push(path);
    }
    Ok(removed)
}

pub fn relative_dir_if_child(parent: &Path, child: &Path) -> Option<String> {
    let relative = child.strip_prefix(parent).ok()?;
    if relative.as_os_str().is_empty() {
        Some(String::new())
    } else {
        Some(relative.to_string_lossy().replace('\\', "/"))
    }
}

pub fn discover_owned_output_dirs(
    repo: &Path,
    include_generated: bool,
    include_vendor: bool,
    ownership: &ArtifactOwnership,
) -> Vec<String> {
    let mut result = Vec::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(directory) = stack.pop() {
        if directory != repo
            && read_ownership_marker(&directory, ownership).is_some_and(|value| {
                value.get("owned_by") == Some(&json!(ownership.owner))
                    && value.get("repo_root") == Some(&json!(repo.to_string_lossy()))
            })
        {
            if let Some(relative) = relative_dir_if_child(repo, &directory)
                && !relative.is_empty()
            {
                result.push(relative);
            }
            continue;
        }
        for entry in directory_entries(&directory).into_iter().rev() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            let Some(relative) = posix_relative(repo, &entry.path()) else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_excluded_dir(&name, include_generated, include_vendor, Some(&relative)) {
                stack.push(entry.path());
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

pub fn validate_repo_relative_include(repo: &Path, raw_path: &str) -> Result<String, String> {
    validate_repo_relative_path_token(raw_path, "--include-file").map_err(|_| {
        format!("--include-file must be a repo-relative path without '.' or '..': {raw_path}")
    })?;
    let path = repo.join(raw_path);
    let metadata = path
        .symlink_metadata()
        .map_err(|_| format!("--include-file does not name an existing file: {raw_path}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "--include-file does not name an existing file: {raw_path}"
        ));
    }
    read_bytes_nofollow(&path, Some(repo))
        .map_err(|_| format!("--include-file does not name a safe file: {raw_path}"))?;
    Ok(raw_path.to_owned())
}

pub fn is_test_source_path(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    path_parent_parts(rel_path)
        .iter()
        .any(|part| TEST_PATH_PARTS.contains(&part.as_str()))
        || matches!(name.as_str(), "self_test.py" | "selftest.py")
        || stem.starts_with("test_")
        || stem.starts_with("spec_")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || stem.ends_with(".test")
        || stem.ends_with(".spec")
}

pub fn build_test_evidence_index(repo: &Path, entries: &[FileEntry], run_id: &str) -> Value {
    let test_files = entries
        .iter()
        .filter(|entry| is_test_source_path(&entry.rel_path))
        .map(|entry| {
            let text = String::from_utf8_lossy(&read_initial_bytes(
                &repo.join(&entry.rel_path),
                1_000_000,
            ))
            .into_owned();
            let mut names = Vec::new();
            'patterns: for pattern in TEST_NAME_PATTERNS.iter() {
                for captures in pattern.captures_iter(&text) {
                    let name = captures
                        .get(1)
                        .map(|capture| capture.as_str())
                        .unwrap_or("")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !name.is_empty() && !names.contains(&name) {
                        names.push(name);
                    }
                    if names.len() >= 200 {
                        break 'patterns;
                    }
                }
            }
            json!({
                "rel_path":entry.rel_path,"sha256":entry.sha256,
                "named_tests":names,"named_tests_truncated":names.len() >= 200,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version":1,"run_id":run_id,"test_file_count":test_files.len(),
        "test_files":test_files,
    })
}

pub fn high_risk_file_inventory(repo: &Path, entries: &[FileEntry]) -> Vec<Value> {
    let mut result = Vec::new();
    for entry in entries {
        let path = Path::new(&entry.rel_path);
        let mut tokens = path_parent_parts(&entry.rel_path)
            .into_iter()
            .collect::<BTreeSet<_>>();
        tokens.extend(filename_words(
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(""),
        ));
        let matched_tokens = tokens
            .iter()
            .filter(|token| HIGH_RISK_PATH_TOKENS.contains(&token.to_lowercase().as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let mut reasons = BTreeSet::new();
        if !matched_tokens.is_empty() {
            reasons.insert(format!(
                "high-risk path/domain tokens: {}",
                matched_tokens.join(", ")
            ));
        }
        let text =
            String::from_utf8_lossy(&read_initial_bytes(&repo.join(&entry.rel_path), 600_000))
                .into_owned();
        if HIGH_RISK_CODE_SUFFIXES.contains(&suffix(path).as_str()) {
            for (pattern, reason) in HIGH_RISK_SOURCE_PATTERNS.iter() {
                if pattern.is_match(&text) {
                    reasons.insert((*reason).to_owned());
                }
            }
        }
        if !reasons.is_empty() {
            result.push(json!({
                "rel_path":entry.rel_path,"sha256":entry.sha256,
                "risk_reasons":reasons.into_iter().collect::<Vec<_>>(),
            }));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn git(root: &Path, arguments: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn classification_preserves_source_config_secret_and_interface_boundaries() {
        assert_eq!(classify("src/main.rs", false, false), Some("source"));
        assert_eq!(
            classify("docs/design.md", false, false),
            Some("source/contract")
        );
        assert_eq!(
            classify("package.json", false, false),
            Some("source/config")
        );
        assert_eq!(classify(".env", true, false), None);
        assert_eq!(
            classify(".env.example", false, false),
            Some("source/config")
        );
        assert_eq!(classify("config/runtime.yaml", true, false), Some("config"));
        assert!(is_secret_env_file("service.env.production"));
        assert!(!is_secret_env_file("service.env.template.json"));

        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "tests/widget.ts",
            b"export const x = 1;\n",
        );
        write(
            directory.path(),
            "tests/messages.json",
            br#"{"empty_state":"Nothing here"}"#,
        );
        assert!(!is_interface_file(
            "tests/widget.ts",
            &directory.path().join("tests/widget.ts")
        ));
        assert!(is_interface_file(
            "tests/messages.json",
            &directory.path().join("tests/messages.json")
        ));
        assert!(is_interface_file(
            "src/components/SaveButton.ts",
            &directory.path().join("tests/widget.ts")
        ));
        assert!(is_ui_asset_path("public/images/logo.png"));
        assert!(!is_ui_asset_path("fixtures/random.png"));
    }

    #[test]
    fn git_collection_preserves_deletions_secret_exclusions_assets_and_ignored_warnings() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        git(repo, &["init", "-q"]);
        git(repo, &["config", "user.email", "fixture@example.test"]);
        git(repo, &["config", "user.name", "Fixture"]);
        write(
            repo,
            ".gitignore",
            b".env\npublic/logo.png\nsrc/ignored.ts\n",
        );
        write(repo, "src/main.rs", b"pub fn main_path() {}\n");
        write(repo, "src/old.rs", b"pub fn old() {}\n");
        write(
            repo,
            "src/components/SaveButton.tsx",
            b"export function SaveButton() { return <button>Save</button>; }\n",
        );
        write(repo, "docs/README.md", b"# Product\n");
        write(repo, ".env.example", b"TOKEN=example\n");
        write(repo, ".env", b"TOKEN=private\n");
        write(repo, "public/logo.png", b"\x89PNG\r\n\x1a\nfixture");
        write(repo, "src/ignored.ts", b"export function ignored() {}\n");
        write(
            repo,
            "node_modules/first_party.ts",
            b"export function vendored() {}\n",
        );
        git(repo, &["add", ".gitignore", "src", "docs", ".env.example"]);
        git(repo, &["commit", "-qm", "fixture"]);
        std::fs::remove_file(repo.join("src/old.rs")).unwrap();

        let default = collect_files(
            repo,
            &CollectOptions {
                include_config: true,
                ..Default::default()
            },
        );
        let included = default
            .entries
            .iter()
            .map(|entry| entry.rel_path.as_str())
            .collect::<BTreeSet<_>>();
        assert!(included.contains("src/main.rs"));
        assert!(included.contains("src/components/SaveButton.tsx"));
        assert!(included.contains(".env.example"));
        assert!(!included.contains(".env"));
        assert!(!included.contains("public/logo.png"));
        assert_eq!(default.tracked_deletions.len(), 1);
        assert_eq!(default.tracked_deletions[0]["path"], "src/old.rs");
        assert_eq!(default.tracked_deletions[0]["baseline_available"], true);
        assert!(default.excluded.iter().any(|entry| {
            entry.get("path") == Some(&json!("src/ignored.ts"))
                && entry.get("scope_warning") == Some(&json!(true))
        }));
        assert!(default.excluded.iter().any(|entry| {
            entry.get("path") == Some(&json!("node_modules"))
                && entry.get("entry_type") == Some(&json!("directory"))
        }));

        let with_asset = collect_files(
            repo,
            &CollectOptions {
                include_config: true,
                include_assets: true,
                ..Default::default()
            },
        );
        assert!(with_asset.entries.iter().any(|entry| {
            entry.rel_path == "public/logo.png" && entry.kind == "source/ui-asset"
        }));
    }

    #[test]
    fn explicit_globs_can_enter_selected_vendor_but_never_owned_output() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vendor/owned/widget.rs",
            b"pub fn widget() {}\n",
        );
        write(
            directory.path(),
            "vendor/other/skip.rs",
            b"pub fn skip() {}\n",
        );
        write(
            directory.path(),
            "audit-out/result.rs",
            b"pub fn result() {}\n",
        );
        let collection = collect_files(
            directory.path(),
            &CollectOptions {
                include_globs: vec![
                    "vendor/owned/**/*.rs".to_owned(),
                    "audit-out/**/*.rs".to_owned(),
                ],
                output_rel_dirs: vec!["audit-out".to_owned()],
                ..Default::default()
            },
        );
        assert!(
            collection
                .entries
                .iter()
                .any(|entry| entry.rel_path == "vendor/owned/widget.rs")
        );
        assert!(
            !collection
                .entries
                .iter()
                .any(|entry| entry.rel_path.contains("vendor/other"))
        );
        assert!(
            !collection
                .entries
                .iter()
                .any(|entry| entry.rel_path.contains("audit-out"))
        );
    }

    #[test]
    fn large_files_split_by_utf8_lines_then_bytes_and_batch_deterministically() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "lines.rs", b"1111\n2222\n3333\n");
        write(directory.path(), "binary.rs", b"0123456789\xffabcdef");
        let entries = vec![
            FileEntry {
                rel_path: "lines.rs".to_owned(),
                size_bytes: 15,
                kind: "source".to_owned(),
                interface_relevant: false,
                sha256: "a".repeat(64),
            },
            FileEntry {
                rel_path: "binary.rs".to_owned(),
                size_bytes: 17,
                kind: "source".to_owned(),
                interface_relevant: false,
                sha256: "b".repeat(64),
            },
        ];
        let units = audit_units_for(directory.path(), &entries, 10);
        assert_eq!(units[0].unit_id, "lines.rs#L1-L2");
        assert_eq!(units[1].unit_id, "lines.rs#L3-L3");
        assert_eq!(units[2].unit_id, "binary.rs#B1-10");
        assert_eq!(units[3].unit_id, "binary.rs#B11-17");
        let batches = batch_files(&units, 2, 15).unwrap();
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1, 1]);
        assert!(duplicate_whole_file_paths_for_batches(&batches).is_empty());
        assert!(purpose_for(&units).contains("primary file types: rs"));
    }

    #[test]
    fn tokens_and_globs_reject_ambiguous_generated_content() {
        assert!(run_id_token("run-1234").is_ok());
        assert!(run_id_token("short").is_err());
        assert!(validate_repo_relative_path_token("src/main.rs", "path").is_ok());
        assert!(validate_repo_relative_path_token("../main.rs", "path").is_err());
        assert!(validate_markdown_safe_token("src/a|b.rs", "path").is_err());
        assert!(glob_matches("src/a/b.rs", "**/*.rs"));
        assert!(glob_matches("main.rs", "**/*.rs"));
        assert!(!glob_matches("main.ts", "**/*.rs"));
    }

    #[test]
    fn ownership_cleanup_removes_only_declared_artifacts_and_never_follows_links() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        let output = directory.path().join("output");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        let ownership = ArtifactOwnership::default();
        assert!(
            ensure_output_dir_safe(&output, &repo, &ownership)
                .unwrap()
                .is_none()
        );
        write(&output, "unrelated.txt", b"keep");
        assert!(ensure_output_dir_safe(&output, &repo, &ownership).is_err());
        write_ownership_marker(
            &output,
            &repo,
            &["manifest.json".to_owned(), "generated/tree".to_owned()],
            "2026-09-04T00:00:00Z",
            &ownership,
        )
        .unwrap();
        let marker = ensure_output_dir_safe(&output, &repo, &ownership)
            .unwrap()
            .unwrap();
        write(&output, "manifest.json", b"{}");
        write(&output, "generated/tree/result.txt", b"remove");
        let external = directory.path().join("external.txt");
        std::fs::write(&external, b"preserve").unwrap();
        std::os::unix::fs::symlink(&external, output.join("batch_999.md")).unwrap();
        let removed = clean_generated_artifacts(&output, Some(&marker), &ownership).unwrap();
        assert!(removed.iter().any(|path| path.ends_with("manifest.json")));
        assert!(!output.join("generated/tree").exists());
        assert!(!output.join("batch_999.md").exists());
        assert_eq!(std::fs::read(&external).unwrap(), b"preserve");
        assert_eq!(
            std::fs::read(output.join("unrelated.txt")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn test_and_high_risk_indexes_are_bounded_and_evidence_backed() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "tests/auth_worker_test.rs",
            b"fn test_password_delete() { let password = \"fixture\"; }\n",
        );
        let bytes = std::fs::read(directory.path().join("tests/auth_worker_test.rs")).unwrap();
        let entries = vec![FileEntry {
            rel_path: "tests/auth_worker_test.rs".to_owned(),
            size_bytes: bytes.len(),
            kind: "source".to_owned(),
            interface_relevant: false,
            sha256: sha256_hex(&bytes),
        }];
        let index = build_test_evidence_index(directory.path(), &entries, "run-1234");
        assert_eq!(index["test_file_count"], 1);
        assert_eq!(
            index["test_files"][0]["named_tests"][0],
            "test_password_delete"
        );
        let high_risk = high_risk_file_inventory(directory.path(), &entries);
        assert_eq!(high_risk.len(), 1);
        assert!(
            high_risk[0]["risk_reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason == "credential or secret handling")
        );
    }

    #[test]
    fn owned_output_discovery_stops_at_the_first_matching_root() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        let ownership = ArtifactOwnership::default();
        let first = repo.join("audit/one");
        let nested = first.join("nested");
        let second = repo.join("audit/two");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        write_ownership_marker(&first, repo, &[], "now", &ownership).unwrap();
        write_ownership_marker(&nested, repo, &[], "now", &ownership).unwrap();
        write_ownership_marker(&second, repo, &[], "now", &ownership).unwrap();
        assert_eq!(
            discover_owned_output_dirs(repo, false, false, &ownership),
            ["audit/one", "audit/two"]
        );
    }
}
