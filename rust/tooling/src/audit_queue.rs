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
use crate::audit_ledger::{
    create_directory_all_nofollow, validate_directory_nofollow, write_bytes_nofollow,
};

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
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub start_byte: Option<usize>,
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

fn render_interface_focus(entries: &[AuditUnit]) -> String {
    let files = entries
        .iter()
        .filter(|entry| entry.interface_relevant)
        .map(|entry| entry.rel_path.clone())
        .collect::<BTreeSet<_>>();
    if files.is_empty() {
        return String::new();
    }
    format!(
        "\n## Interface Audit Focus\n\nThese files are likely to define UI, visible copy, navigation, forms, or interface behavior:\n\n{}\n\nFor these files, inventory visible product promises and trace them to implementation:\n- Buttons, icon buttons, menu items, command items, tabs, links, and shortcuts.\n- Text fields, selectors, filters, uploads, toggles, settings, and forms.\n- Toasts, banners, empty states, tooltips, helper text, validation text, success messages, and error messages.\n- Loading, empty, error, permission denied, background job, undo/redo, and destructive confirmation states.\n\nFlag interface elements that are unimplemented, handler-only placeholders, console-only behavior, disabled dead ends, not persisted, not validated, not reflected in API/state, misleadingly labeled, inaccessible, or wired to the wrong route/action. Include the exact visible label or message text whenever possible.\nRecord the result in the required `Interface Inventory` section even when no gap is found.\n",
        files
            .into_iter()
            .map(|path| format!("- `{path}`"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn render_journey_file_list(entries: &[FileEntry]) -> String {
    if entries.is_empty() {
        return "- No interface-relevant files were detected.".to_owned();
    }
    entries
        .iter()
        .map(|entry| {
            format!(
                "- `{}` ({}, {} bytes, sha256=`{}`)",
                entry.rel_path, entry.kind, entry.size_bytes, entry.sha256
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn replace_tokens(template: &str, values: &[(&str, String)]) -> String {
    values
        .iter()
        .fold(template.to_owned(), |text, (token, value)| {
            text.replace(token, value)
        })
}

const JOURNEY_SOURCE_TEMPLATE: &str = r#"# Full Repo Audit User Journey Source Worker

Repo root: `@@REPO@@`
Run ID: `@@RUN_ID@@`

@@DELIVERY@@
@@DISPATCH@@

You are a separate low-effort worker focused on user journeys through the UI. Do not edit the audited repository; write only the exact audit artifacts authorized above. Use the interface-relevant source files below, plus repo docs/routes/config when needed, to determine whether the app describes complete user journeys, required feature/UI elements, and test expectations, and whether the UI source supports them.

## Interface-Relevant Files

@@FILES@@

## Tasks

1. Find explicit journeys, feature/UI inventories, onboarding, workflows, routes, product flows, support/common-task docs, test expectations, and source-backed route flows.
2. Draft reasonable frequent journeys from app intent, routes, visible copy, and code when documentation is incomplete. Mark every such journey `draft-needs-user-confirmation`; never treat it as confirmed product truth.
3. Walk every confirmed or drafted journey. Classify mentioned UI relevance as `critical-always`, `primary-frequent`, `secondary-occasional`, or `rare-under-5-percent`; mark assumptions `confirmed`, `source-inferred`, or `missing`.
4. Check the primary decision, required facts, warnings, frequent actions, reachable rare detail, desktop/native/mobile availability, responsive fit, and loading/empty/error/permission states.
5. For badges, flags, rows, disclosures, scrolling details, messages, tool/results, copy and icon controls, mark every checklist label `pass`, `gap`, `blocked`, or `not applicable`: `badge-detail`, `row-hit-target`, `navigation-cursor`, `transient-disclosure`, `disclosure-scrollbar`, `icon-meaning`, `stable-expansion-width`, `hover-copy`, `status-summary`, `message-metadata`.
6. Identify a safe test/fixture path for visually exercising the journey. Treat unconfirmed journeys as questions or assumption-based coverage, never clean UI proof.

## Required Report File

Write Markdown with exactly these sections to the report path above:

## Run ID
@@RUN_ID@@

## Worker
journey_source

## Journey Sources
List source files/docs/routes that define or imply the journeys.

## Proposed Journeys
List every confirmed journey and every `draft-needs-user-confirmation` journey with target user, goal, entry, route/screen sequence, decisions, required and rare UI, assumption status, tests, and success/failure end states.

## UI Source Journey Checks
| Journey | Step | Files | Primary navigation/decision elements | Relevance estimate | Required information | Interaction and metadata checklist | Mobile/Desktop availability | Test mode evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |

Every checklist cell must contain all ten exact labels and statuses.

## Findings
Use atomic P0/P1/P2/P3 blocks with `Files`, `Evidence`, `Interface evidence`, `Expected behavior/standard`, `Gap`, and `Suggested direction`, or exactly `No findings.`.

## Open Questions
Ask the lead to clarify frequent use cases when journey information is missing or ambiguous.
"#;

fn render_journey_source_prompt(
    repo: &Path,
    run_id: &str,
    entries: &[FileEntry],
    report_path: &Path,
) -> Result<String, String> {
    Ok(replace_tokens(
        JOURNEY_SOURCE_TEMPLATE,
        &[
            ("@@REPO@@", repo.to_string_lossy().into_owned()),
            ("@@RUN_ID@@", run_id.to_owned()),
            ("@@DELIVERY@@", artifact_delivery_contract(report_path)?),
            ("@@DISPATCH@@", isolated_light_worker_contract().to_owned()),
            ("@@FILES@@", render_journey_file_list(entries)),
        ],
    ))
}

const VISUAL_JOURNEY_TEMPLATE: &str = r#"# Full Repo Audit Visual Journey Worker

Repo root: `@@REPO@@`
Run ID: `@@RUN_ID@@`

@@DELIVERY@@
@@DISPATCH@@

Authorized visual evidence manifest: `@@EVIDENCE_MANIFEST@@`.
Screenshot, formal-verifier, journey-evidence, changed-review queue, decision, and manual-review artifacts may be written only beneath the same audit-output directory and must be registered in that manifest.

You are a separate low-effort worker focused on visual journey verification. Do not edit the audited repository. Prefer fixture/test mode and avoid production or heavy side effects. For CLI/library/plugin packages with no repo-owned rendered UI, mark visual checks `not applicable` with evidence; do not mistake host-owned rendering for a repo defect.

## Interface-Relevant Files

@@FILES@@

## Tasks

1. Identify Playwright/Cypress/Storybook/browser or native preview tooling and the safe test mode.
2. Walk confirmed or drafted high-frequency journeys at desktop and narrow-mobile viewports, checking navigation, primary decisions, required facts, reachable rare details, warnings, loading/empty/error states, hierarchy, clipping, overflow, typography, contrast, theme consistency, scrollability, compactness, and accessibility.
3. Mark all interaction checklist labels: `badge-detail`, `row-hit-target`, `navigation-cursor`, `transient-disclosure`, `disclosure-scrollbar`, `icon-meaning`, `stable-expansion-width`, `hover-copy`, `status-summary`, `message-metadata`.
4. For safe web render paths run `formal-web-ui-verification` with complete journey/theme/region/continuation/UI-input declarations. Preserve visible-scrollbar and palette-risk inventories.
5. Finish automation before manual review. Read `review-queue.json`, open only queued initial/full-page screenshots, record `pass`, `gap`, or `blocked`, finalize with the Rust `formal-ui review` tool, and carry unchanged prior gaps without reopening images. Hash drift is integrity evidence only.
6. Populate `visual_evidence.json` with confined paths, SHA-256, detected MIME, dimensions, route/state/viewport metadata, capture tool, formal report, `journey-evidence`, review queue, and `manual-review`; cite real artifacts as `evidence:<id>`.

## Required Report File

## Run ID
@@RUN_ID@@

## Worker
visual_journey

## Visual Tooling
List modes, commands, formal verifier status, scrollbars, palette risks, and blockers.

## Visual Journey Checks
| Journey | Viewport | Route/screen | Evidence | Navigation visibility | Decision information | Interaction and metadata checklist | Visual quality | Result |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Changed Visual Review
| Review cell | Trigger/status | Initial viewport evidence | Full-page evidence | Decision | Note |
| --- | --- | --- | --- | --- | --- |

Cite both screenshot ids for queued cells and the formal report, journey evidence, review queue, and manual-review ids. Carried unchanged cells say `not reopened — carried unchanged`; every gap/blocked or carried-gap/carried-blocked row requires a finding.

## Findings
Use atomic P0/P1/P2/P3 blocks with `Files`, `Evidence`, `Interface evidence`, `Expected behavior/standard`, `Gap`, and `Suggested direction`, or exactly `No findings.`.

## Open Questions
List missing test-mode or journey clarifications for the lead.
"#;

fn render_visual_journey_prompt(
    repo: &Path,
    run_id: &str,
    entries: &[FileEntry],
    report_path: &Path,
) -> Result<String, String> {
    let evidence_manifest = report_path
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .join("visual_evidence.json");
    Ok(replace_tokens(
        VISUAL_JOURNEY_TEMPLATE,
        &[
            ("@@REPO@@", repo.to_string_lossy().into_owned()),
            ("@@RUN_ID@@", run_id.to_owned()),
            ("@@DELIVERY@@", artifact_delivery_contract(report_path)?),
            ("@@DISPATCH@@", isolated_light_worker_contract().to_owned()),
            ("@@FILES@@", render_journey_file_list(entries)),
            (
                "@@EVIDENCE_MANIFEST@@",
                absolute_display(&evidence_manifest)?,
            ),
        ],
    ))
}

const LEAD_TEMPLATE: &str = r#"# Full Repo Audit Lead Reconciliation

Repo root: `@@REPO@@`
Run ID: `@@RUN_ID@@`

@@DELIVERY@@

This required lead-owned artifact is completed only after every batch and applicable journey report. Independently reopen assigned source and recheck every batch PASS responsibility anchor; sampling is insufficient. Do not edit the audited repository.

Trace every real feature, API, command, route, job, event, configuration, schema/migration, build/deploy, and operational contract across file boundaries. Challenge hard-coded substitutes, ignored inputs, fake success, incomplete registration, memory-only state presented as durable, missing effects, partial read/write paths, production mocks, missing lifecycle behavior, and shape-only tests. Findings are one independently closable outcome each.

## Required Report File

## Run ID
@@RUN_ID@@

## Worker
lead_reconciliation

## Cross-File Contract Trace
For a non-empty repository map every batch Contract ID exactly once into sequential `lead:C<3+ digits>` rows. For an empty manifest write exactly `No source-backed implementation contracts were queued.`

| Contract ID | Batch Contract IDs | Contract/source anchors | entry-registration | core-logic | data-lifecycle | integration-boundary | authorization-trust | failure-recovery | observable-outcome | operational-lifecycle | verification | Result |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

Each trace cell begins `pass —`, `gap —`, `blocked —`, or justified `not applicable —`. Every source anchor from every mapped batch row recurs in contract anchors, observable outcome, and verification. Verification declares exactly one `evidence-type: test|runtime|source-only`, one `counterfactual:` or `invariance:`, and for test/runtime one `evidence-ref:`. PASS test/runtime rows also name an observed `outcome:` or `result:`. Persistence, integration, external-effect, and success PASS claims require test/runtime evidence. GAP/BLOCKED mappings cannot become PASS; every GAP/BLOCKED row has an atomic finding citing its lead Contract ID.

## Findings
Use atomic P0/P1/P2/P3 blocks with `Files`, `Evidence`, `Interface evidence`, `Expected behavior/standard`, `Gap`, and `Suggested direction`, or exactly `No findings.`.

## Open Questions
List unresolved ambiguities or exactly `None.`. Questions and hypotheses are not findings.
"#;

fn render_lead_reconciliation_prompt(
    repo: &Path,
    run_id: &str,
    report_path: &Path,
) -> Result<String, String> {
    Ok(replace_tokens(
        LEAD_TEMPLATE,
        &[
            ("@@REPO@@", repo.to_string_lossy().into_owned()),
            ("@@RUN_ID@@", run_id.to_owned()),
            (
                "@@DELIVERY@@",
                lead_artifact_delivery_contract(report_path)?,
            ),
        ],
    ))
}

const BATCH_TEMPLATE: &str = r#"# Full Repo Audit Batch @@BATCH@@/@@TOTAL@@

Repo root: `@@REPO@@`
Batch purpose: @@PURPOSE@@

@@DELIVERY@@
@@DISPATCH@@

You are a low-effort worker auditing only this batch. Do not edit the repository. Inspect every assigned unit and report only evidence tied to it.

## Cross-Batch Verification Guardrail

Consult `@@TEST_INDEX@@` before calling a responsibility untested. Inspect named candidate tests, then make at most one behavior-specific manifest search. Tests in another batch are evidence but remain owned there. Browser fixtures do not prove persistence or external effects. Run only relevant bounded tests, preserve completed outcomes and exits, and after one non-destructive environment workaround record one BLOCKED contract with its unblock condition rather than repeated product findings.

## Pre-return Report Gate

Use one row per contract responsibility and exactly one atomic finding per GAP/BLOCKED. Copy parser-recognized `name@L...:C...` or `name@B...` anchors exactly; never invent coordinates. Declarative/manual rows cite an exact raw backticked token. Save the report, then validate this batch with the Rust verifier before returning:

`devcoordinator2-tooling audit verify-full-repo --manifest <audit-output>/manifest.json --batch-id batch_@@BATCH@@ --reports <exact-report-path> --json`

## Files You Own

@@FILES@@
@@RANGE@@
@@INTERFACE@@

## Audit Questions

Trace each responsibility from registration/entry through validation and domain logic into dependencies, persistence/effects, observable output, failure/recovery, authorization/trust, and verification. Find explicit stubs and marker-free gaps: constants substituted for calculations, ignored configuration, unregistered routes/jobs/exports, memory-only durability, swallowed errors, presentation-only authorization, production fixtures, missing migration/rollback/retry/cancel/cleanup, or tests that prove shape rather than changed outcome. Check reliability, security, accessibility, performance, maintainability, state/error/permission handling, and every visible product promise.

## Required Report File

## Run ID
@@RUN_ID@@

## Batch ID
batch_@@BATCH@@

## Batch Summary
Briefly describe this batch.

## File Coverage
| File | Status | SHA-256 | Purpose |
| --- | --- | --- | --- |

One CHECKED/UNCHECKED row per exact file or range unit.

## Implementation Inventory
| File/unit | Contract ID | Contract/responsibility | Entrypoints/source anchors | Implementation/data/side-effect trace | Failure/edge/permission/recovery trace | Verification evidence | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |

Every unit appears at least once; independent responsibilities use sequential `batch_@@BATCH@@:C<3+ digits>` IDs. Every responsibility has exactly one `Basis: <kind> — <backticked reference>` and one `Discovery: parsed|manual — <backticked anchor/token>`. Allowed basis kinds are `user-requirement`, `acceptance-criterion`, `recorded-decision`, `public-contract`, `interface-promise`, `caller-contract`, `schema-invariant`, `operational-contract`, and honest `source-inferred`. Trace cells begin `pass —`, `gap —`, `blocked —`, or justified `not applicable —`. Verification declares exactly one `evidence-type: test`, `evidence-type: runtime`, or `evidence-type: source-only` plus one counterfactual/invariance; test/runtime adds one `evidence-ref:` and PASS adds observed `outcome:` or `result:`. Persistence/integration/external-effect/success PASS requires test/runtime evidence. Result is GAP if any gap, else BLOCKED if any blocked, else PASS.

## Interface Inventory
For interface units, inventory each visible surface/text/control/message, expected handler/state/API/persistence/verification path, and actual evidence. Otherwise write exactly `No interface-relevant files in this batch.`

## Findings
One atomic P0/P1/P2/P3 subsection per GAP/BLOCKED Contract ID with `Files`, `Evidence`, `Interface evidence`, `Expected behavior/standard`, `Gap`, and `Suggested direction`.

## No Finding Notes
List checked units with no notable issue.

## Open Questions
List ambiguities for the lead.
"#;

fn absolute_display(path: &Path) -> Result<String, String> {
    if path.is_absolute() {
        return Ok(path.to_string_lossy().into_owned());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path).to_string_lossy().into_owned())
        .map_err(|error| format!("cannot resolve artifact path: {error}"))
}

fn render_batch_prompt(
    repo: &Path,
    run_id: &str,
    batch_id: usize,
    total_batches: usize,
    entries: &[AuditUnit],
    report_path: &Path,
) -> Result<String, String> {
    let files = entries
        .iter()
        .map(|entry| {
            if let (Some(start), Some(end)) = (entry.start_line, entry.end_line) {
                format!(
                    "- Unit `{}`: `{}` lines {start}-{end} ({}, approx {} bytes in this range, interface={}, full-file sha256=`{}`)",
                    entry.unit_id, entry.rel_path, entry.kind, entry.size_bytes,
                    entry.interface_relevant, entry.sha256
                )
            } else if let (Some(start), Some(end)) = (entry.start_byte, entry.end_byte) {
                format!(
                    "- Unit `{}`: `{}` bytes {start}-{end} ({}, {} bytes in this range, interface={}, full-file sha256=`{}`)",
                    entry.unit_id, entry.rel_path, entry.kind, entry.size_bytes,
                    entry.interface_relevant, entry.sha256
                )
            } else {
                format!(
                    "- Unit `{}`: `{}` ({}, {} bytes, interface={}, sha256=`{}`)",
                    entry.unit_id, entry.rel_path, entry.kind, entry.size_bytes,
                    entry.interface_relevant, entry.sha256
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let range = if entries
        .iter()
        .any(|entry| entry.start_line.is_some() || entry.start_byte.is_some())
    {
        "\n## Range Review Scope\n\nInspect each assigned line/byte range manually and only nearby context needed to understand it. Use the exact ranged unit id in File Coverage and Findings/No Finding Notes; cite the real repo path plus range in evidence.\n".to_owned()
    } else {
        String::new()
    };
    let batch = format!("{batch_id:03}");
    Ok(replace_tokens(
        BATCH_TEMPLATE,
        &[
            ("@@REPO@@", repo.to_string_lossy().into_owned()),
            ("@@RUN_ID@@", run_id.to_owned()),
            ("@@BATCH@@", batch),
            ("@@TOTAL@@", format!("{total_batches:03}")),
            ("@@PURPOSE@@", purpose_for(entries)),
            ("@@DELIVERY@@", artifact_delivery_contract(report_path)?),
            ("@@DISPATCH@@", isolated_light_worker_contract().to_owned()),
            ("@@FILES@@", files),
            ("@@RANGE@@", range),
            ("@@INTERFACE@@", render_interface_focus(entries)),
            (
                "@@TEST_INDEX@@",
                absolute_display(
                    &report_path
                        .parent()
                        .and_then(Path::parent)
                        .unwrap_or_else(|| Path::new("."))
                        .join("test_evidence_index.json"),
                )?,
            ),
        ],
    ))
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    write_bytes_nofollow(path, text.as_bytes(), 0o600).map_err(|error| error.to_string())
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn write_completion_marker(
    out_dir: &Path,
    manifest: &Value,
    completed_at: &str,
    ownership: &ArtifactOwnership,
) -> Result<(), String> {
    let marker = json!({
        "run_id":manifest["run_id"],
        "completed_at":completed_at,
        "phase":"queue_generated",
        "audit_verified":false,
        "marker_semantics":"Queue artifacts were generated; subagent reports and effort ledger still require verifier completion.",
        "manifest":"manifest.json",
        "audit_index":"audit_index.md",
        "effort_ledger":"effort_ledger.json",
        "excluded_files":"excluded_files.json",
        "reports_dir":"reports",
        "logs_dir":"logs",
        "final_report":"final-report.md",
        "ownership_marker":ownership.marker_name,
        "batch_count":manifest["batch_count"],
        "source_file_count":manifest["source_file_count"],
    });
    let temporary = out_dir.join("queue_complete.json.tmp");
    let destination = out_dir.join("queue_complete.json");
    write_json(&temporary, &marker)?;
    if destination.symlink_metadata().is_ok() {
        return Err(format!(
            "queue completion destination unexpectedly exists: {}",
            destination.display()
        ));
    }
    std::fs::rename(&temporary, &destination)
        .map_err(|error| format!("cannot publish queue completion marker: {error}"))
}

fn write_effort_ledger(out_dir: &Path, manifest: &Value) -> Result<(), String> {
    let journey = manifest
        .get("journey_audit")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let lead = manifest
        .get("lead_reconciliation")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let journey_required = journey.get("required") == Some(&json!(true));
    let pruned_hints = manifest
        .get("pruned_directory_review_hints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tracked_deletions = manifest
        .get("tracked_deletions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let pruned_count = manifest
        .get("pruned_directory_review_hint_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let deletion_count = manifest
        .get("tracked_deletion_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let high_risk_count = manifest
        .get("high_risk_file_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let pruned_decisions = pruned_hints
        .iter()
        .filter_map(Value::as_object)
        .map(|hint| {
            json!({
                "path":hint.get("path").cloned().unwrap_or(Value::Null),
                "source_like_sample_paths":hint.get("source_like_sample_paths").cloned().unwrap_or_else(|| json!([])),
                "decision":Value::Null,"rationale":Value::Null,
            })
        })
        .collect::<Vec<_>>();
    let deletion_decisions = tracked_deletions
        .iter()
        .filter_map(Value::as_object)
        .map(|removal| {
            json!({
                "path":removal.get("path").cloned().unwrap_or(Value::Null),
                "baseline_sha256":removal.get("baseline_sha256").cloned().unwrap_or(Value::Null),
                "decision":Value::Null,"rationale":Value::Null,"evidence":Value::Null,
            })
        })
        .collect::<Vec<_>>();
    let high_risk_files = manifest
        .get("high_risk_files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|item| {
            let mut item = item.clone();
            item.extend([
                ("status".to_owned(), json!("pending")),
                ("evidence".to_owned(), Value::Null),
                ("notes".to_owned(), Value::Null),
            ]);
            Value::Object(item)
        })
        .collect::<Vec<_>>();
    let batches = manifest
        .get("batches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|batch| {
            let id = batch.get("id").and_then(Value::as_str).unwrap_or("");
            json!({
                "batch_id":id,
                "prompt":batch.get("prompt").cloned().unwrap_or(Value::Null),
                "required_reasoning_effort":"low",
                "agent_id":Value::Null,"actual_reasoning_effort":Value::Null,
                "status":"pending","report":format!("reports/{id}.md"),
                "runtime_provenance":Value::Null,"effort_claim_basis":Value::Null,
                "effort_claim_label":Value::Null,"notes":Value::Null,
            })
        })
        .collect::<Vec<_>>();
    let ledger = json!({
        "run_id":manifest["run_id"],"repo_root":manifest["repo_root"],
        "provenance_scope":"Lead-recorded runtime ledger. The verifier checks recorded agent ids, effort values, reports, journey-worker assignments, and fallback consistency; it cannot independently prove platform scheduler settings.",
        "effort_verification_scope":"ledger-recorded",
        "subagent_capability_check":{
            "status":"pending","spawn_tool":Value::Null,"can_set_reasoning_effort":Value::Null,
            "claim_basis":Value::Null,"claim_label":Value::Null,"evidence":Value::Null,
            "notes":"Lead agent must record whether subagent spawning and reasoning_effort settings are available before dispatch.",
        },
        "lead":{
            "status":"pending","required_reasoning_effort":"xhigh",
            "actual_reasoning_effort":Value::Null,"agent_id":Value::Null,
            "effort_claim_basis":Value::Null,"effort_claim_label":Value::Null,
            "runtime_provenance":Value::Null,"notes":Value::Null,
        },
        "fallback_mode":{"active":false,"reason":Value::Null},
        "pruned_directory_review":{
            "status":if pruned_count > 0 {"pending"} else {"not-applicable"},
            "hint_count":pruned_count,"decisions":pruned_decisions,
            "notes":if pruned_count > 0 {
                "Lead must review pruned_directory_review_hints before claiming full coverage."
            } else {"No pruned directories contained source-like samples."},
        },
        "tracked_deletion_review":{
            "status":if deletion_count > 0 {"pending"} else {"not-applicable"},
            "removal_count":deletion_count,"decisions":deletion_decisions,
            "notes":if deletion_count > 0 {
                "Lead must review every tracked removal against its baseline and current references before claiming full coverage."
            } else {"No tracked files are deleted from the current worktree."},
        },
        "lead_high_risk_review":{
            "status":if high_risk_count > 0 {"pending"} else {"not-applicable"},
            "files":high_risk_files,
        },
        "lead_reconciliation":{
            "status":"pending","prompt":lead.get("prompt").cloned().unwrap_or(Value::Null),
            "report":lead.get("report").cloned().unwrap_or(Value::Null),
            "notes":"Required lead-owned cross-file semantic implementation reconciliation.",
        },
        "journey_source_worker":{
            "status":if journey_required {"pending"} else {"not-applicable"},
            "prompt":journey.get("source_prompt").cloned().unwrap_or(Value::Null),
            "required_reasoning_effort":if journey_required {json!("low")} else {Value::Null},
            "agent_id":Value::Null,"actual_reasoning_effort":Value::Null,
            "report":journey.get("source_report").cloned().unwrap_or(Value::Null),
            "runtime_provenance":Value::Null,"effort_claim_basis":Value::Null,
            "effort_claim_label":Value::Null,
            "notes":if journey_required {
                "Required when interface-relevant files are queued; inspect source-level user journeys, relevance, decision information, and test-mode support."
            } else {"No interface-relevant files were queued."},
        },
        "visual_journey_worker":{
            "status":if journey_required {"pending"} else {"not-applicable"},
            "prompt":journey.get("visual_prompt").cloned().unwrap_or(Value::Null),
            "required_reasoning_effort":if journey_required {json!("low")} else {Value::Null},
            "agent_id":Value::Null,"actual_reasoning_effort":Value::Null,
            "report":journey.get("visual_report").cloned().unwrap_or(Value::Null),
            "runtime_provenance":Value::Null,"effort_claim_basis":Value::Null,
            "effort_claim_label":Value::Null,
            "notes":if journey_required {
                "Required when interface-relevant files are queued; use available visual tooling in test mode or report the blocker."
            } else {"No interface-relevant files were queued."},
        },
        "batches":batches,
    });
    write_json(&out_dir.join("effort_ledger.json"), &ledger)
}

fn table_cell(value: &Value) -> String {
    let text = match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    };
    text.replace(['\n', '\r'], " ")
        .replace('\\', "\\\\")
        .replace('|', "\\|")
}

fn render_index(repo: &Path, out_dir: &Path, manifest: &Value) -> String {
    let batches = manifest
        .get("batches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rows = if batches.is_empty() {
        "| none | none | 0 | 0 | 0 | 0 | No source-like files were queued. |".to_owned()
    } else {
        batches
            .iter()
            .map(|batch| {
                format!(
                    "| {} | `{}` | {} | {} | {} | {} | {} |",
                    table_cell(&batch["id"]),
                    table_cell(&batch["prompt"]),
                    batch["file_count"],
                    batch
                        .get("coverage_unit_count")
                        .unwrap_or(&batch["file_count"]),
                    batch["interface_file_count"],
                    batch["byte_count"],
                    table_cell(&batch["purpose"]),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let journey = &manifest["journey_audit"];
    let (journey_instruction, journey_prompts) = if journey["required"] == true {
        (
            "3. Dispatch the two generated journey workers and save their exact reports.",
            format!(
                "- Source journey worker prompt: `{}` -> `{}`\n- Visual journey worker prompt: `{}` -> `{}`",
                journey["source_prompt"].as_str().unwrap_or(""),
                journey["source_report"].as_str().unwrap_or(""),
                journey["visual_prompt"].as_str().unwrap_or(""),
                journey["visual_report"].as_str().unwrap_or("")
            ),
        )
    } else {
        (
            "3. No journey workers are required because no interface-relevant files were queued.",
            "- No journey worker prompts were generated because no interface-relevant files were queued."
                .to_owned(),
        )
    };
    format!(
        "# Full Repo Audit Queue\n\nRepo root: `{}`\nOutput directory: `{}`\nGenerated: `{}`\nRun ID: `{}`\nQueue completion marker: `queue_complete.json`\n\nSource files queued: **{}**\nInterface-relevant files queued: **{}**\nExcluded high-signal files needing lead review: **{}**\nPruned directories with source-like samples needing lead review: **{}**\nTracked worktree removals needing lead review: **{}**\nBatches: **{}**\nAll source files queued exactly once: **{}**\n\n## Lead Agent Instructions\n\n1. Run the lead architectural audit with extra-high effort.\n2. Spawn one fresh isolated worker per batch prompt with the runtime/user-selected effort and the entire prompt plus applicable project-ledger requirements.\n{journey_instruction}\n4. Workers write complete reports to exact paths and return only bounded `REPORT_SAVED` receipts.\n5. Confirm `queue_complete.json` and run IDs before dispatch.\n6. Fill `effort_ledger.json`, including capability, lead, batch, journey, pruned-directory, deletion, and high-risk review status.\n7. Resolve every `scope_warning`, pruned review hint, and tracked deletion.\n8. Verify exact reports with `{}`.\n9. Requeue missing or unchecked units and reconcile every feature/entry point across batches.\n10. Complete `lead_reconciliation.md`, validate candidates, keep verbose output in `logs/`, and write complete synthesis to `final-report.md`.\n\n## Batches\n\n| Batch | Prompt | Files | Units | UI Files | Bytes | Purpose |\n| --- | --- | ---: | ---: | ---: | ---: | --- |\n{rows}\n\n## Journey Worker Prompts\n\n{journey_prompts}\n\n## Coverage Files\n\n- `manifest.json`: source inventory and coverage invariants.\n- `{}`: ownership marker for safe reruns.\n- `queue_complete.json`: queue-generation marker, not audit completion.\n- `{VERIFICATION_RECEIPT_NAME}`: stable verifier receipt required for consolidation.\n- `effort_ledger.json`, `excluded_files.json`, `test_evidence_index.json`, `reports/`, `logs/`, and `final-report.md`: authoritative audit artifacts.\n- `lead_reconciliation.md` and conditional journey prompts: required reconciliation surfaces.\n- `batch_###.md`: exact isolated worker prompts.\n",
        repo.display(),
        out_dir.display(),
        manifest["generated_at"].as_str().unwrap_or(""),
        manifest["run_id"].as_str().unwrap_or(""),
        manifest["source_file_count"],
        manifest["interface_file_count"],
        manifest["scope_warning_count"],
        manifest["pruned_directory_review_hint_count"],
        manifest["tracked_deletion_count"],
        manifest["batch_count"],
        manifest["coverage_invariants"]["all_source_files_queued_exactly_once"],
        manifest["verifier_command"].as_str().unwrap_or(""),
        manifest["artifact_marker"].as_str().unwrap_or(""),
    )
}

#[derive(Clone, Debug)]
pub struct FullRepoOutputOptions {
    pub generated_at: String,
    pub archive_stamp: String,
    pub verifier_program: PathBuf,
    pub ownership: ArtifactOwnership,
}

pub fn write_full_repo_outputs(
    repo: &Path,
    out_dir: &Path,
    collection: &FileCollection,
    units: &[AuditUnit],
    batches: &[Vec<AuditUnit>],
    run_id: &str,
    options: &FullRepoOutputOptions,
) -> Result<Value, String> {
    validate_generated_artifact_tokens(&collection.entries, units)?;
    let existing = ensure_output_dir_safe(out_dir, repo, &options.ownership)?;
    create_directory_all_nofollow(out_dir, 0o700).map_err(|error| error.to_string())?;
    let marker = if existing.is_none() {
        write_ownership_marker(
            out_dir,
            repo,
            &[],
            &options.generated_at,
            &options.ownership,
        )?;
        read_ownership_marker(out_dir, &options.ownership)
    } else {
        existing
    };
    clean_generated_artifacts(out_dir, marker.as_ref(), &options.ownership)?;

    let reports_dir = out_dir.join("reports");
    let mut archived_reports_dir = None;
    let mut archived_reports_name = None;
    if let Ok(metadata) = reports_dir.symlink_metadata() {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "Audit reports path must be a non-symlinked directory: {}",
                reports_dir.display()
            ));
        }
        validate_directory_nofollow(&reports_dir).map_err(|error| error.to_string())?;
        if !directory_entries(&reports_dir).is_empty() {
            let mut suffix = 1usize;
            let mut archive = out_dir.join(format!("reports.stale.{}", options.archive_stamp));
            while archive.exists() {
                suffix += 1;
                archive = out_dir.join(format!(
                    "reports.stale.{}.{}",
                    options.archive_stamp, suffix
                ));
            }
            std::fs::rename(&reports_dir, &archive)
                .map_err(|error| format!("cannot archive stale reports: {error}"))?;
            archived_reports_name = archive
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned);
            archived_reports_dir = Some(archive.to_string_lossy().into_owned());
        }
    }
    create_directory_all_nofollow(&reports_dir, 0o700).map_err(|error| error.to_string())?;
    let logs_dir = create_directory_all_nofollow(&out_dir.join("logs"), 0o700)
        .map_err(|error| error.to_string())?;
    let test_index = build_test_evidence_index(repo, &collection.entries, run_id);
    write_json(&out_dir.join("test_evidence_index.json"), &test_index)?;

    let mut batch_records = Vec::new();
    let mut all_batched_paths = Vec::new();
    let mut all_batched_units = Vec::new();
    for (offset, batch) in batches.iter().enumerate() {
        let index = offset + 1;
        let prompt_name = format!("batch_{index:03}.md");
        let report_path = reports_dir.join(&prompt_name);
        write_text(
            &out_dir.join(&prompt_name),
            &render_batch_prompt(repo, run_id, index, batches.len(), batch, &report_path)?,
        )?;
        let paths = batch
            .iter()
            .map(|item| item.rel_path.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let unit_ids = batch
            .iter()
            .map(|item| item.unit_id.clone())
            .collect::<Vec<_>>();
        all_batched_paths.extend(paths.iter().cloned());
        all_batched_units.extend(unit_ids.iter().cloned());
        batch_records.push(json!({
            "id":format!("batch_{index:03}"),"prompt":prompt_name,
            "report":format!("reports/batch_{index:03}.md"),
            "file_count":paths.len(),"coverage_unit_count":batch.len(),
            "interface_file_count":batch.iter().filter(|item| item.interface_relevant).count(),
            "byte_count":batch.iter().map(|item| item.size_bytes).sum::<usize>(),
            "files":paths,"coverage_units":unit_ids,"purpose":purpose_for(batch),
        }));
    }
    let source_paths = collection
        .entries
        .iter()
        .map(|entry| entry.rel_path.clone())
        .collect::<BTreeSet<_>>();
    let coverage_units = units
        .iter()
        .map(|unit| unit.unit_id.clone())
        .collect::<BTreeSet<_>>();
    let batched_paths = all_batched_paths.iter().cloned().collect::<BTreeSet<_>>();
    let batched_units = all_batched_units.iter().cloned().collect::<BTreeSet<_>>();
    let duplicate_units = duplicate_strings(&all_batched_units);
    let missing_units = coverage_units
        .difference(&batched_units)
        .cloned()
        .collect::<Vec<_>>();
    let extra_units = batched_units
        .difference(&coverage_units)
        .cloned()
        .collect::<Vec<_>>();
    let missing = source_paths
        .difference(&batched_paths)
        .cloned()
        .collect::<Vec<_>>();
    let extra = batched_paths
        .difference(&source_paths)
        .cloned()
        .collect::<Vec<_>>();
    let duplicate_paths = duplicate_whole_file_paths_for_batches(batches);
    let scope_warnings = collection
        .excluded
        .iter()
        .filter(|item| item.get("scope_warning") == Some(&json!(true)))
        .cloned()
        .collect::<Vec<_>>();
    let pruned_hints = collection
        .excluded
        .iter()
        .filter(|item| {
            item.get("entry_type") == Some(&json!("directory"))
                && item.get("contains_source_like_samples") == Some(&json!(true))
        })
        .cloned()
        .collect::<Vec<_>>();
    let interface_entries = collection
        .entries
        .iter()
        .filter(|entry| entry.interface_relevant)
        .cloned()
        .collect::<Vec<_>>();
    let high_risk_files = high_risk_file_inventory(repo, &collection.entries);
    let journey_required = !interface_entries.is_empty();
    let journey = json!({
        "required":journey_required,
        "interface_files":interface_entries.iter().map(|entry| entry.rel_path.clone()).collect::<Vec<_>>(),
        "source_prompt":if journey_required {json!("journey_audit.md")} else {Value::Null},
        "source_report":if journey_required {json!("reports/journey_audit.md")} else {Value::Null},
        "visual_prompt":if journey_required {json!("visual_journey_audit.md")} else {Value::Null},
        "visual_report":if journey_required {json!("reports/visual_journey_audit.md")} else {Value::Null},
    });
    let lead = json!({
        "required":true,"worker":"lead_reconciliation","prompt":"lead_reconciliation.md",
        "report":"reports/lead_reconciliation.md",
    });
    write_text(
        &out_dir.join("lead_reconciliation.md"),
        &render_lead_reconciliation_prompt(
            repo,
            run_id,
            &reports_dir.join("lead_reconciliation.md"),
        )?,
    )?;
    if journey_required {
        write_text(
            &out_dir.join("journey_audit.md"),
            &render_journey_source_prompt(
                repo,
                run_id,
                &interface_entries,
                &reports_dir.join("journey_audit.md"),
            )?,
        )?;
        write_text(
            &out_dir.join("visual_journey_audit.md"),
            &render_visual_journey_prompt(
                repo,
                run_id,
                &interface_entries,
                &reports_dir.join("visual_journey_audit.md"),
            )?,
        )?;
        write_json(
            &out_dir.join("visual_evidence.json"),
            &json!({"schema_version":1,"run_id":run_id,"artifacts":[]}),
        )?;
    }
    let verifier_args = vec![
        options.verifier_program.to_string_lossy().into_owned(),
        "audit".to_owned(),
        "verify-full-repo".to_owned(),
        "--manifest".to_owned(),
        out_dir.join("manifest.json").to_string_lossy().into_owned(),
        "--reports".to_owned(),
        reports_dir.to_string_lossy().into_owned(),
        "--receipt-out".to_owned(),
        out_dir
            .join(VERIFICATION_RECEIPT_NAME)
            .to_string_lossy()
            .into_owned(),
    ];
    let verifier_command = verifier_args
        .iter()
        .map(|argument| shell_quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let mut generated_artifacts = vec![
        "audit_index.md".to_owned(),
        "effort_ledger.json".to_owned(),
        "excluded_files.json".to_owned(),
        "manifest.json".to_owned(),
        "queue_complete.json".to_owned(),
        "test_evidence_index.json".to_owned(),
        "lead_reconciliation.md".to_owned(),
        "final-report.md".to_owned(),
        "logs".to_owned(),
        VERIFICATION_RECEIPT_NAME.to_owned(),
    ];
    if journey_required {
        generated_artifacts.extend([
            "journey_audit.md".to_owned(),
            "visual_journey_audit.md".to_owned(),
            "visual_evidence.json".to_owned(),
        ]);
    }
    generated_artifacts.extend(archived_reports_name);
    generated_artifacts.extend(batch_records.iter().filter_map(|batch| {
        batch
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }));
    let all_units_once =
        missing_units.is_empty() && duplicate_units.is_empty() && extra_units.is_empty();
    let all_sources_once = missing.is_empty() && extra.is_empty() && all_units_once;
    let manifest = json!({
        "repo_root":repo.to_string_lossy(),"run_id":run_id,
        "generated_at":options.generated_at,
        "reports_dir":reports_dir.to_string_lossy(),"logs_dir":logs_dir.to_string_lossy(),
        "final_report":out_dir.join("final-report.md").to_string_lossy(),
        "archived_reports_dir":archived_reports_dir,
        "artifact_marker":out_dir.join(&options.ownership.marker_name).to_string_lossy(),
        "effort_ledger":out_dir.join("effort_ledger.json").to_string_lossy(),
        "test_evidence_index":out_dir.join("test_evidence_index.json").to_string_lossy(),
        "test_evidence_file_count":test_index["test_file_count"],
        "generated_artifacts":generated_artifacts,
        "verifier_command":verifier_command,"verifier_args":verifier_args,
        "source_file_count":collection.entries.len(),
        "interface_file_count":interface_entries.len(),
        "scope_warning_count":scope_warnings.len(),
        "pruned_directory_review_hint_count":pruned_hints.len(),
        "tracked_deletion_count":collection.tracked_deletions.len(),
        "high_risk_file_count":high_risk_files.len(),
        "excluded_file_count":collection.excluded.len(),
        "excluded_files_sha256":canonical_json_sha256(&Value::Array(collection.excluded.clone()))?,
        "batch_count":batches.len(),
        "source_files":serde_json::to_value(&collection.entries).map_err(|error| error.to_string())?,
        "coverage_unit_count":units.len(),
        "coverage_units":serde_json::to_value(units).map_err(|error| error.to_string())?,
        "batches":batch_records,
        "journey_audit":journey,"lead_reconciliation":lead,
        "coverage_invariants":{
            "unique_batched_file_count":batched_paths.len(),
            "unique_batched_unit_count":batched_units.len(),
            "missing_from_batches":missing,"duplicates_in_batches":duplicate_paths,
            "extra_in_batches":extra,"missing_units_from_batches":missing_units,
            "duplicate_units_in_batches":duplicate_units,"extra_units_in_batches":extra_units,
            "all_coverage_units_queued_exactly_once":all_units_once,
            "all_source_files_queued_exactly_once":all_sources_once,
        },
        "scope_warnings":scope_warnings,"pruned_directory_review_hints":pruned_hints,
        "tracked_deletions":collection.tracked_deletions,"high_risk_files":high_risk_files,
    });
    write_json(&out_dir.join("manifest.json"), &manifest)?;
    write_json(
        &out_dir.join("excluded_files.json"),
        &Value::Array(collection.excluded.clone()),
    )?;
    write_text(
        &out_dir.join("audit_index.md"),
        &render_index(repo, out_dir, &manifest),
    )?;
    write_effort_ledger(out_dir, &manifest)?;
    let generated_artifacts = manifest["generated_artifacts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    write_ownership_marker(
        out_dir,
        repo,
        &generated_artifacts,
        &options.generated_at,
        &options.ownership,
    )?;
    write_completion_marker(
        out_dir,
        &manifest,
        &options.generated_at,
        &options.ownership,
    )?;
    Ok(manifest)
}

fn duplicate_strings(values: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            duplicates.insert(value.clone());
        }
    }
    duplicates.into_iter().collect()
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
        write(repo, ".env.example", b"TOKEN=example\n"); // public-artifact-guard: allow text-secret
        write(repo, ".env", b"TOKEN=private\n"); // public-artifact-guard: allow text-secret
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

    #[test]
    fn full_repo_publication_writes_exact_invariants_prompts_and_archives_reports() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        write(
            &repo,
            "src/App.tsx",
            b"export function App() { return <button>Save</button>; }\n",
        );
        write(
            &repo,
            "tests/app.test.ts",
            b"test('saves changes', () => {});\n",
        );
        git(&repo, &["add", "src/App.tsx", "tests/app.test.ts"]);
        let collection = collect_files(
            &repo,
            &CollectOptions {
                include_config: true,
                ..Default::default()
            },
        );
        let units = audit_units_for(&repo, &collection.entries, DEFAULT_MAX_BATCH_BYTES);
        let batches = batch_files(&units, 8, DEFAULT_MAX_BATCH_BYTES).unwrap();
        let output = directory.path().join("audit-output");
        let mut options = FullRepoOutputOptions {
            generated_at: "2026-09-04T00:00:00Z".to_owned(),
            archive_stamp: "20260904T000000Z".to_owned(),
            verifier_program: PathBuf::from("/usr/local/bin/devcoordinator2-tooling"),
            ownership: ArtifactOwnership::default(),
        };
        let manifest = write_full_repo_outputs(
            &repo,
            &output,
            &collection,
            &units,
            &batches,
            "run-1234",
            &options,
        )
        .unwrap();
        assert_eq!(manifest["source_file_count"], 2, "{collection:#?}");
        assert_eq!(manifest["interface_file_count"], 1);
        assert_eq!(
            manifest["coverage_invariants"]["all_source_files_queued_exactly_once"],
            true
        );
        assert_eq!(manifest["coverage_units"][0]["start_line"], Value::Null);
        assert!(
            manifest["verifier_args"]
                .as_array()
                .unwrap()
                .iter()
                .all(|value| !value.as_str().unwrap_or("").contains("python"))
        );
        for name in [
            "manifest.json",
            "excluded_files.json",
            "test_evidence_index.json",
            "effort_ledger.json",
            "queue_complete.json",
            "audit_index.md",
            "lead_reconciliation.md",
            "journey_audit.md",
            "visual_journey_audit.md",
            "visual_evidence.json",
            "batch_001.md",
        ] {
            assert!(output.join(name).is_file(), "missing {name}");
        }
        let batch = std::fs::read_to_string(output.join("batch_001.md")).unwrap();
        for token in [
            "fresh isolated context",
            "runtime/user-selected effort",
            "REPORT_SAVED",
            "Implementation Inventory",
            "evidence-type: test",
            "evidence-type: runtime",
            "evidence-type: source-only",
            "No interface-relevant files in this batch.",
        ] {
            assert!(batch.contains(token), "batch prompt omitted {token}");
        }
        let visual = std::fs::read_to_string(output.join("visual_journey_audit.md")).unwrap();
        for token in [
            "review-queue.json",
            "journey-evidence",
            "manual-review",
            "visible-scrollbar",
            "evidence:<id>",
        ] {
            assert!(visual.contains(token), "visual prompt omitted {token}");
        }
        let completion: Value =
            serde_json::from_slice(&std::fs::read(output.join("queue_complete.json")).unwrap())
                .unwrap();
        assert_eq!(completion["phase"], "queue_generated");
        assert_eq!(completion["audit_verified"], false);
        let effort: Value =
            serde_json::from_slice(&std::fs::read(output.join("effort_ledger.json")).unwrap())
                .unwrap();
        assert_eq!(effort["lead"]["required_reasoning_effort"], "xhigh");
        assert_eq!(effort["journey_source_worker"]["status"], "pending");

        write(&output, "reports/batch_001.md", b"old report");
        write(&output, "unrelated.txt", b"keep");
        options.generated_at = "2026-09-04T00:01:00Z".to_owned();
        options.archive_stamp = "20260904T000100Z".to_owned();
        let rerun = write_full_repo_outputs(
            &repo,
            &output,
            &collection,
            &units,
            &batches,
            "run-5678",
            &options,
        )
        .unwrap();
        assert!(
            output
                .join("reports.stale.20260904T000100Z/batch_001.md")
                .is_file()
        );
        assert_eq!(
            rerun["archived_reports_dir"],
            json!(
                output
                    .join("reports.stale.20260904T000100Z")
                    .to_string_lossy()
            )
        );
        assert_eq!(
            std::fs::read(output.join("unrelated.txt")).unwrap(),
            b"keep"
        );
    }
}
