#!/usr/bin/env python3
"""Build deterministic source-file audit batches for the full-repo-audit skill."""

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import uuid
from collections import Counter
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Iterable


SOURCE_EXTENSIONS = {
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
    ".hh",
    ".hpp",
    ".hrl",
    ".html",
    ".java",
    ".js",
    ".jsx",
    ".j2",
    ".kt",
    ".kts",
    ".less",
    ".lua",
    ".m",
    ".mdx",
    ".mm",
    ".mjs",
    ".mts",
    ".php",
    ".prisma",
    ".proto",
    ".pl",
    ".pm",
    ".ps1",
    ".py",
    ".r",
    ".rb",
    ".razor",
    ".rs",
    ".sass",
    ".scala",
    ".scss",
    ".sh",
    ".sql",
    ".svelte",
    ".svg",
    ".ejs",
    ".hbs",
    ".handlebars",
    ".jinja",
    ".jinja2",
    ".liquid",
    ".mustache",
    ".njk",
    ".pug",
    ".tpl",
    ".twig",
    ".swift",
    ".storyboard",
    ".tf",
    ".tfvars",
    ".ts",
    ".tsx",
    ".vb",
    ".vue",
    ".xib",
    ".zsh",
}

DIR_EXCLUSION_SAMPLE_LIMIT = 20
DIR_EXCLUSION_COUNT_LIMIT = 100
DEFAULT_MAX_BATCH_BYTES = 60_000

CONFIG_EXTENSIONS = {
    ".cjs",
    ".csproj",
    ".conf",
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
    ".xml",
    ".xcconfig",
    ".xaml",
    ".yaml",
    ".yml",
}

SOURCE_FILENAMES = {
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
    "Dockerfile",
    "docker-compose.yaml",
    "docker-compose.yml",
    "Directory.Build.props",
    "Directory.Build.targets",
    "Directory.Packages.props",
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
}

LOCK_FILENAMES = {
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
}

SOURCE_MARKDOWN_FILENAMES = {
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
}

SOURCE_MARKDOWN_DIRS = {
    "docs",
    "documentation",
    "guides",
    "prompts",
    "references",
}

SOURCE_SCRIPT_DIRS = {
    ".husky",
    "hooks",
    "script",
    "scripts",
    "tools",
}

SOURCE_SUFFIXES = (
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
)

MESSAGE_CATALOG_EXTENSIONS = {
    ".arb",
    ".ftl",
    ".po",
    ".pot",
    ".properties",
    ".resx",
    ".strings",
    ".xlf",
    ".xliff",
}

MESSAGE_CATALOG_CONFIG_EXTENSIONS = {
    ".json",
    ".jsonc",
    ".yaml",
    ".yml",
}

GENERATED_DIRS = {
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
}

GENERATED_FILE_SUFFIXES = {
    ".tsbuildinfo",
}

VENDOR_DIRS = {
    "bower_components",
    "node_modules",
    "Pods",
    "vendor",
}

TOOLING_DIRS = {
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
}

HIDDEN_PROJECT_DIRS = {
    ".changeset",
    ".claude",
    ".devcontainer",
    ".github",
    ".gitlab",
    ".husky",
    ".storybook",
    ".well-known",
}

FIRST_PARTY_HIDDEN_PROJECT_PARENT_DIRS = {
    "app",
    "apps",
    "backend",
    "client",
    "engine",
    "frontend",
    "lib",
    "libs",
    "package",
    "packages",
    "server",
    "service",
    "services",
    "src",
}

EXCLUDED_FILENAMES = {
    ".DS_Store",
}


def is_local_tooling_file(rel_path: str) -> bool:
    """Return true only for runtime-local files that are never project truth."""
    parts = PurePosixPath(rel_path).parts
    return len(parts) >= 2 and ".claude" in parts[:-1] and parts[-1] == "settings.local.json"

ENV_EXAMPLE_MARKERS = (
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
)
ENV_EXAMPLE_BASENAMES = {
    "example",
    "sample",
    "template",
}
ENV_EXAMPLE_TOKEN_MARKERS = {
    "dist",
    "example",
    "sample",
    "schema",
    "template",
}

BINARY_EXTENSIONS = {
    ".a",
    ".avif",
    ".bmp",
    ".class",
    ".dll",
    ".dmg",
    ".eot",
    ".exe",
    ".gif",
    ".gz",
    ".ico",
    ".jar",
    ".jpeg",
    ".jpg",
    ".mov",
    ".mp3",
    ".mp4",
    ".o",
    ".pdf",
    ".png",
    ".so",
    ".sqlite",
    ".ttf",
    ".wasm",
    ".webm",
    ".webp",
    ".woff",
    ".woff2",
    ".zip",
}

UI_ASSET_EXTENSIONS = {
    ".avif",
    ".bmp",
    ".eot",
    ".gif",
    ".ico",
    ".jpeg",
    ".jpg",
    ".png",
    ".ttf",
    ".webp",
    ".woff",
    ".woff2",
}
UI_ASSET_DIRS = {
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
}
UI_ASSET_NAME_TOKENS = {
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
}

INTERFACE_EXTENSIONS = {
    ".axaml",
    ".astro",
    ".cshtml",
    ".css",
    ".ejs",
    ".html",
    ".hbs",
    ".handlebars",
    ".j2",
    ".jinja",
    ".jinja2",
    ".jsx",
    ".liquid",
    ".less",
    ".mdx",
    ".mustache",
    ".njk",
    ".pug",
    ".razor",
    ".sass",
    ".scss",
    ".svelte",
    ".svg",
    ".storyboard",
    ".tpl",
    ".tsx",
    ".twig",
    ".vue",
    ".xaml",
    ".xib",
}

ANDROID_INTERFACE_DIRS = {
    "layout",
    "menu",
    "navigation",
}

INTERFACE_PATH_PARTS = {
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
}

# Test and tooling directories can live beneath an otherwise UI-oriented
# application root. They are audited as implementation, but their fixtures and
# dependency metadata are not themselves user-facing interface surfaces.
NON_INTERFACE_SOURCE_PARTS = {
    "__tests__",
    "e2e",
    "fixtures",
    "scripts",
    "test",
    "tests",
    "tools",
}

INTERFACE_TEXT_PARTS = {
    "i18n",
    "lang",
    "locale",
    "locales",
    "messages",
    "translations",
}

INTERFACE_NAME_TOKENS = (
    "button",
    "checkbox",
    "command",
    "dialog",
    "drawer",
    "dropdown",
    "field",
    "form",
    "input",
    "menu",
    "modal",
    "nav",
    "page",
    "popover",
    "screen",
    "select",
    "sidebar",
    "tab",
    "toast",
    "toolbar",
    "tooltip",
    "view",
)

INTERFACE_KEY_MARKERS = {
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
}

HIGH_SIGNAL_SOURCE_DIRS = {
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
}

ARTIFACT_MARKER = ".full-repo-audit-artifacts.json"
ARTIFACT_OWNER = "full-repo-audit"
VERIFICATION_RECEIPT_NAME = "verification_receipt.json"
KNOWN_GENERATED_ARTIFACTS = {
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
}
COMPANION_SCRIPT_DIR = Path(__file__).resolve().parent
MARKDOWN_UNSAFE_PATH_CHARS = {"|", "`"}


@dataclass(frozen=True)
class FileEntry:
    rel_path: str
    size_bytes: int
    kind: str
    interface_relevant: bool
    sha256: str


@dataclass(frozen=True)
class AuditUnit:
    unit_id: str
    rel_path: str
    size_bytes: int
    kind: str
    interface_relevant: bool
    sha256: str
    start_line: int | None = None
    end_line: int | None = None
    start_byte: int | None = None
    end_byte: int | None = None


def positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be an integer") from exc
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return parsed


RUN_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{7,127}$")


def run_id_token(value: str) -> str:
    if not RUN_ID_RE.fullmatch(value):
        raise argparse.ArgumentTypeError(
            "must be 8-128 characters using letters, numbers, '.', '_', or '-'"
        )
    return value


def validate_markdown_safe_token(value: str, field_name: str) -> None:
    if value != value.strip():
        raise ValueError(f"{field_name} must not have leading or trailing whitespace.")
    if any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise ValueError(f"{field_name} must not contain ASCII control characters.")
    unsafe = sorted(char for char in MARKDOWN_UNSAFE_PATH_CHARS if char in value)
    if unsafe:
        raise ValueError(f"{field_name} must not contain Markdown table/code delimiters: {unsafe}.")


def validate_repo_relative_path_token(value: str, field_name: str) -> None:
    validate_markdown_safe_token(value, field_name)
    if "\\" in value:
        raise ValueError(f"{field_name} must use POSIX repo-relative paths.")
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts):
        raise ValueError(f"{field_name} must be a repo-relative path without '.' or '..' segments.")


def artifact_delivery_contract(report_path: Path) -> str:
    """Render the shared cold-artifact/compact-receipt worker contract."""
    report_text = str(report_path.resolve())
    validate_markdown_safe_token(report_text, "worker report path")
    report_name = report_path.name
    validate_markdown_safe_token(report_name, "worker report filename")
    return f"""## Artifact Delivery Contract

Write the complete required Markdown report directly to `{report_text}`. The
report file is the result of record; do not paste or repeat its body in the
subagent response. Writing this audit artifact does not authorize editing the
audited repository.

After the file is durably written, compute its SHA-256 and byte length, then
respond with only this compact receipt:

`REPORT_SAVED filename={report_name}; status=<complete|blocked> sha256=<64-hex> bytes=<integer> findings=<integer-or-unknown>`

Keep the receipt at or below 80 tokens. If the report cannot be written, return
only `REPORT_NOT_SAVED path=<exact-report-path>; reason=<short-reason>`, still
at or below 80 tokens. Never substitute the full report body for a failed
artifact write.
"""


def isolated_light_worker_contract() -> str:
    return """## Dispatch Contract

The lead must pass this entire generated prompt plus applicable project-ledger
requirements. Spawn this worker in a fresh isolated context with the
runtime/user-selected effort. Never rely on an inherited lead transcript.
"""


def lead_artifact_delivery_contract(report_path: Path) -> str:
    report_text = str(report_path.resolve())
    validate_markdown_safe_token(report_text, "lead report path")
    return f"""## Artifact Delivery Contract

Write the complete lead-owned reconciliation directly to `{report_text}`.
Keep this detailed artifact out of routine chat context; inspect it through
targeted sections and let the verifier validate it. Writing this artifact does
not authorize editing the audited repository.
"""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Create full-repo-audit source-file manifest and subagent batch prompts."
    )
    parser.add_argument("--repo", default=".", help="Repository root to audit. Defaults to cwd.")
    parser.add_argument(
        "--out",
        default=None,
        help="Output directory. Defaults to a system temp directory outside the audited repo.",
    )
    parser.add_argument("--batch-size", type=positive_int, default=8, help="Maximum files per batch.")
    parser.add_argument(
        "--max-batch-bytes",
        type=positive_int,
        default=DEFAULT_MAX_BATCH_BYTES,
        help="Maximum total file bytes per batch. Larger text files are split into line-range or byte-range coverage units.",
    )
    parser.add_argument(
        "--include-config",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Include source-adjacent config and schema files. Default: true.",
    )
    parser.add_argument(
        "--include-env",
        action="store_true",
        help="Include real .env files. Default excludes likely secret-bearing env files.",
    )
    parser.add_argument(
        "--include-generated",
        action="store_true",
        help="Include generated/build output directories such as dist, build, out, and target.",
    )
    parser.add_argument(
        "--include-vendor",
        action="store_true",
        help="Include vendored dependency directories such as node_modules, vendor, Pods, and bower_components.",
    )
    parser.add_argument(
        "--include-assets",
        action="store_true",
        help="Include source-backed binary UI assets such as public logos, icons, screenshots, and fonts. Default warns on likely UI assets.",
    )
    parser.add_argument(
        "--run-id",
        type=run_id_token,
        default=None,
        help="Optional stable audit run id to write into manifest, prompts, and verifier expectations.",
    )
    parser.add_argument(
        "--exclude-glob",
        action="append",
        default=[],
        help="Additional repo-relative glob to exclude. Can be repeated.",
    )
    parser.add_argument(
        "--include-file",
        action="append",
        default=[],
        help="Force-include a repo-relative file that classification would otherwise skip. Can be repeated.",
    )
    parser.add_argument(
        "--include-glob",
        action="append",
        default=[],
        help="Force-include repo-relative files matching a glob. Can be repeated.",
    )
    return parser.parse_args()


def utc_stamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def run_git_files(repo: Path) -> list[str] | None:
    try:
        subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "--is-inside-work-tree"],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        result = subprocess.run(
            ["git", "-C", str(repo), "ls-files", "-co", "--exclude-standard", "-z"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    return [os.fsdecode(item) for item in result.stdout.split(b"\0") if item]


def run_git_deleted_paths(repo: Path) -> list[str] | None:
    """Return tracked paths deleted from either the index or worktree."""

    paths: set[str] = set()
    for command in (
        ("diff", "--name-only", "--diff-filter=D", "--no-renames", "-z"),
        ("diff", "--cached", "--name-only", "--diff-filter=D", "--no-renames", "-z"),
    ):
        try:
            result = subprocess.run(
                ["git", "-C", str(repo), *command],
                check=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        except (OSError, subprocess.CalledProcessError):
            return None
        paths.update(os.fsdecode(item) for item in result.stdout.split(b"\0") if item)
    return sorted(paths)


def git_deleted_path_evidence(repo: Path, rel_path: str) -> dict[str, object]:
    """Capture baseline metadata for a deleted path without restoring it."""

    for revision in (f":{rel_path}", f"HEAD:{rel_path}"):
        try:
            result = subprocess.run(
                ["git", "-C", str(repo), "show", revision],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
        except OSError:
            continue
        if result.returncode == 0:
            return {
                "path": rel_path,
                "baseline_sha256": hashlib.sha256(result.stdout).hexdigest(),
                "baseline_size_bytes": len(result.stdout),
                "baseline_available": True,
            }
    return {
        "path": rel_path,
        "baseline_sha256": None,
        "baseline_size_bytes": None,
        "baseline_available": False,
    }


def excluded_pathspecs(
    include_generated: bool,
    include_vendor: bool,
    output_rel_dirs: list[str] | None = None,
) -> list[str]:
    output_rel_dirs = output_rel_dirs or []
    dir_names = set(TOOLING_DIRS)
    if not include_generated:
        dir_names.update(GENERATED_DIRS)
    if not include_vendor:
        dir_names.update(VENDOR_DIRS)
    pathspecs = []
    for name in sorted(dir_names):
        pathspecs.append(f":(exclude){name}/**")
        pathspecs.append(f":(exclude)**/{name}/**")
    pathspecs.extend(f":(exclude){rel_dir.rstrip('/')}/**" for rel_dir in output_rel_dirs if rel_dir)
    return pathspecs


def run_git_ignored_files(
    repo: Path,
    include_generated: bool,
    include_vendor: bool,
    output_rel_dirs: list[str] | None = None,
) -> list[str]:
    try:
        result = subprocess.run(
            [
                "git",
                "-C",
                str(repo),
                "ls-files",
                "-i",
                "-o",
                "--exclude-standard",
                "-z",
                "--",
                *excluded_pathspecs(include_generated, include_vendor, output_rel_dirs),
            ],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except (OSError, subprocess.CalledProcessError):
        return []
    return [os.fsdecode(item) for item in result.stdout.split(b"\0") if item]


def is_first_party_hidden_project_dir(rel_dir: str) -> bool:
    parts = PurePosixPath(rel_dir).parts
    if not parts or parts[-1] not in HIDDEN_PROJECT_DIRS:
        return False
    if len(parts) == 1:
        return True
    return bool(set(parts[:-1]) & FIRST_PARTY_HIDDEN_PROJECT_PARENT_DIRS)


def is_excluded_dir(part: str, include_generated: bool, include_vendor: bool, rel_dir: str | None = None) -> bool:
    if part in HIDDEN_PROJECT_DIRS and is_first_party_hidden_project_dir(rel_dir or part):
        return False
    if part in TOOLING_DIRS:
        return True
    if part in HIDDEN_PROJECT_DIRS:
        return True
    if part in GENERATED_DIRS and not include_generated:
        return True
    if part in VENDOR_DIRS and not include_vendor:
        return True
    return False


def excluded_dir_reason(part: str, include_generated: bool, include_vendor: bool, rel_dir: str | None = None) -> str | None:
    if part in TOOLING_DIRS:
        return f"excluded tooling directory: {part}"
    if part in HIDDEN_PROJECT_DIRS and not is_first_party_hidden_project_dir(rel_dir or part):
        return f"excluded nested hidden project directory: {rel_dir or part}"
    if part in GENERATED_DIRS and not include_generated:
        return f"excluded generated/build directory: {part}; pass --include-generated to audit"
    if part in VENDOR_DIRS and not include_vendor:
        return f"excluded vendor directory: {part}; pass --include-vendor to audit"
    return None


def walk_files(repo: Path, include_generated: bool, include_vendor: bool) -> Iterable[str]:
    for root, dirs, files in os.walk(repo):
        root_path = Path(root)
        dirs[:] = [
            item
            for item in dirs
            if not is_excluded_dir(
                item,
                include_generated,
                include_vendor,
                (root_path / item).relative_to(repo).as_posix(),
            )
        ]
        for filename in files:
            path = root_path / filename
            yield path.relative_to(repo).as_posix()


def include_glob_explicitly_targets_dir(rel_dir: str, include_globs: list[str]) -> bool:
    rel_dir = rel_dir.rstrip("/")
    if not rel_dir:
        return False
    rel_parts = PurePosixPath(rel_dir).parts
    for pattern in include_globs:
        normalized = pattern.strip()
        if normalized.startswith("./"):
            normalized = normalized[2:]
        while normalized.startswith("**/"):
            normalized = normalized[3:]
        wildcard_positions = [pos for token in ("*", "?", "[") if (pos := normalized.find(token)) != -1]
        wildcard_at = min(wildcard_positions) if wildcard_positions else len(normalized)
        literal_prefix = normalized[:wildcard_at]
        if not literal_prefix:
            continue
        prefix_parts = tuple(part for part in PurePosixPath(literal_prefix.rstrip("/")).parts if part != "**")
        if not prefix_parts:
            continue
        if prefix_parts == rel_parts:
            return True
        if len(prefix_parts) > len(rel_parts) and prefix_parts[: len(rel_parts)] == rel_parts:
            return True
        if len(prefix_parts) < len(rel_parts) and rel_parts[: len(prefix_parts)] == prefix_parts:
            return True
    return False


def include_glob_may_match_excluded_dir(rel_dir: str, include_globs: list[str]) -> bool:
    rel_parts = PurePosixPath(rel_dir.rstrip("/")).parts
    for pattern in include_globs:
        normalized = pattern.strip()
        if normalized.startswith("./"):
            normalized = normalized[2:]
        parts = PurePosixPath(normalized).parts
        literal_parts = {
            part
            for part in parts[:-1]
            if part != "**" and not any(token in part for token in ("*", "?", "["))
        }
        if literal_parts and any(part in literal_parts for part in rel_parts):
            return True
    return False


def include_glob_explicitly_targets_excluded_path(rel_path: str, include_globs: list[str]) -> bool:
    parts = PurePosixPath(rel_path).parts[:-1]
    for index, part in enumerate(parts):
        rel_dir = PurePosixPath(*parts[: index + 1]).as_posix()
        if part in GENERATED_DIRS or part in VENDOR_DIRS:
            if include_glob_explicitly_targets_dir(rel_dir, include_globs):
                return True
    return False


def walk_for_include_globs(
    repo: Path,
    include_generated: bool,
    include_vendor: bool,
    output_rel_dirs: list[str] | None,
    include_globs: list[str],
) -> Iterable[str]:
    output_rel_dirs = output_rel_dirs or []
    for root, dirs, files in os.walk(repo):
        root_path = Path(root)
        kept_dirs = []
        for dirname in dirs:
            rel_dir = (root_path / dirname).relative_to(repo).as_posix()
            if excluded_by_output_dir(rel_dir, output_rel_dirs):
                continue
            if (
                is_excluded_dir(dirname, include_generated, include_vendor, rel_dir)
                and not include_glob_may_match_excluded_dir(rel_dir, include_globs)
                and not include_glob_explicitly_targets_dir(rel_dir, include_globs)
            ):
                continue
            kept_dirs.append(dirname)
        dirs[:] = kept_dirs
        for filename in files:
            path = root_path / filename
            yield path.relative_to(repo).as_posix()


def summarize_pruned_dirs(
    repo: Path,
    include_config: bool,
    include_env: bool,
    include_generated: bool,
    include_vendor: bool,
    include_files: set[str],
    include_globs: list[str],
    output_rel_dirs: list[str] | None = None,
) -> list[dict]:
    output_rel_dirs = output_rel_dirs or []
    summaries: list[dict] = []
    seen: set[str] = set()
    for root, dirs, _files in os.walk(repo):
        root_path = Path(root)
        kept_dirs = []
        for dirname in dirs:
            dir_path = root_path / dirname
            rel_path = dir_path.relative_to(repo).as_posix()
            if excluded_by_output_dir(rel_path, output_rel_dirs):
                continue
            if dirname in HIDDEN_PROJECT_DIRS and is_first_party_hidden_project_dir(rel_path):
                kept_dirs.append(dirname)
                continue
            reason = excluded_dir_reason(dirname, include_generated, include_vendor, rel_path)
            if reason is None:
                kept_dirs.append(dirname)
                continue
            if rel_path in seen:
                continue
            seen.add(rel_path)
            file_count = 0
            file_count_capped = False
            sample_paths: list[str] = []
            source_like_sample_paths: list[str] = []
            source_like_sample_count = 0
            unresolved_source_like_sample_count = 0
            scan_capped = False
            for child_root, child_dirs, child_files in os.walk(dir_path):
                child_root_path = Path(child_root)
                child_dirs[:] = sorted(
                    item
                    for item in child_dirs
                    if item not in TOOLING_DIRS
                    and excluded_dir_reason(
                        item,
                        include_generated,
                        include_vendor,
                        (child_root_path / item).relative_to(repo).as_posix(),
                    )
                    is None
                )
                for child_file in sorted(child_files):
                    child_path = Path(child_root) / child_file
                    child_rel_path = child_path.relative_to(repo).as_posix()
                    if file_count >= DIR_EXCLUSION_COUNT_LIMIT:
                        file_count_capped = True
                        scan_capped = True
                        break
                    file_count += 1
                    if len(sample_paths) < DIR_EXCLUSION_SAMPLE_LIMIT:
                        sample_paths.append(child_rel_path)
                    if reason.startswith(("excluded generated/build directory", "excluded vendor directory")):
                        try:
                            source_like = (
                                classify(child_rel_path, include_config, include_env) is not None
                                or is_extensionless_script_candidate(child_rel_path, child_path)
                                or is_ui_asset_path(child_rel_path)
                            )
                        except OSError:
                            source_like = False
                        if source_like:
                            source_like_sample_count += 1
                            if not forced_include_reason(child_rel_path, include_files, include_globs):
                                unresolved_source_like_sample_count += 1
                                if len(source_like_sample_paths) < DIR_EXCLUSION_SAMPLE_LIMIT:
                                    source_like_sample_paths.append(child_rel_path)
                if scan_capped:
                    child_dirs[:] = []
                    break
            summary = {
                "path": rel_path,
                "reason": reason,
                "size_bytes": 0,
                "scope_warning": False,
                "entry_type": "directory",
                "file_count": file_count,
                "file_count_capped": file_count_capped,
                "scan_file_limit": DIR_EXCLUSION_COUNT_LIMIT,
                "sample_paths": sample_paths,
            }
            if source_like_sample_count:
                if file_count_capped:
                    summary["source_like_observed_count"] = source_like_sample_count
                    summary["source_like_count_capped"] = True
                else:
                    summary["source_like_total_count"] = source_like_sample_count
                    summary["source_like_count_capped"] = False
            if unresolved_source_like_sample_count:
                summary.update(
                    {
                        "contains_source_like_samples": True,
                        "source_like_sample_count": unresolved_source_like_sample_count,
                        "source_like_sample_count_capped": file_count_capped,
                        "source_like_sample_paths": source_like_sample_paths,
                        "review_hint": "Pruned directory contains source-like samples from a bounded scan; pass the relevant include flag or --include-file/--include-glob if these are first-party files.",
                    }
                )
            summaries.append(
                summary
            )
        dirs[:] = kept_dirs
    return summaries


def walk_secret_env_files(repo: Path, include_generated: bool, include_vendor: bool) -> Iterable[str]:
    for root, dirs, files in os.walk(repo):
        root_path = Path(root)
        dirs[:] = [
            item
            for item in dirs
            if not is_excluded_dir(
                item,
                include_generated,
                include_vendor,
                (root_path / item).relative_to(repo).as_posix(),
            )
        ]
        for filename in files:
            if is_secret_env_file(filename):
                yield (root_path / filename).relative_to(repo).as_posix()


def has_dir_part(rel_path: str, names: set[str]) -> bool:
    return any(part in names for part in Path(rel_path).parts[:-1])


def is_under_rel_dir(rel_path: str, rel_dir: str) -> bool:
    normalized = rel_dir.rstrip("/")
    return bool(normalized) and (rel_path == normalized or rel_path.startswith(f"{normalized}/"))


def excluded_by_output_dir(rel_path: str, output_rel_dirs: list[str]) -> str | None:
    for output_rel_dir in output_rel_dirs:
        if is_under_rel_dir(rel_path, output_rel_dir):
            return f"audit output directory excluded: {output_rel_dir}"
    return None


def walk_requested_extra_files(
    repo: Path,
    include_env: bool,
    include_generated: bool,
    include_vendor: bool,
) -> Iterable[str]:
    for rel_path in walk_files(repo, include_generated, include_vendor):
        path = Path(rel_path)
        if include_env and is_secret_env_file(path.name):
            yield rel_path
        elif include_generated and (
            has_dir_part(rel_path, GENERATED_DIRS) or is_generated_file(rel_path)
        ):
            yield rel_path
        elif include_vendor and has_dir_part(rel_path, VENDOR_DIRS):
            yield rel_path


def is_hidden_path(rel_path: str, include_generated: bool) -> bool:
    parts = Path(rel_path).parts[:-1]
    for index, part in enumerate(parts):
        if not part.startswith("."):
            continue
        rel_dir = PurePosixPath(*parts[: index + 1]).as_posix()
        if part in HIDDEN_PROJECT_DIRS and is_first_party_hidden_project_dir(rel_dir):
            continue
        if include_generated and part in GENERATED_DIRS:
            continue
        return True
    return False


def excluded_by_dir(rel_path: str, include_generated: bool, include_vendor: bool) -> str | None:
    for part in Path(rel_path).parts[:-1]:
        if part in TOOLING_DIRS:
            return f"excluded tooling directory: {part}"
        if part in GENERATED_DIRS and not include_generated:
            return f"excluded generated/build directory: {part}"
        if part in VENDOR_DIRS and not include_vendor:
            return f"excluded vendor directory: {part}"
    return None


def is_generated_file(rel_path: str) -> bool:
    return Path(rel_path).suffix.lower() in GENERATED_FILE_SUFFIXES


def excluded_generated_file_reason(
    rel_path: str, include_generated: bool
) -> str | None:
    if not include_generated and is_generated_file(rel_path):
        return (
            "excluded generated/build file: "
            f"{Path(rel_path).suffix.lower()}; pass --include-generated to audit"
        )
    return None


def matches_any_glob(rel_path: str, patterns: list[str]) -> str | None:
    for pattern in patterns:
        normalized_pattern = normalize_glob_pattern(pattern)
        if glob_matches(rel_path, normalized_pattern):
            return f"matched --exclude-glob {pattern}"
    return None


def glob_matches(rel_path: str, normalized_pattern: str) -> bool:
    if fnmatch.fnmatch(rel_path, normalized_pattern):
        return True
    if normalized_pattern.startswith("**/") and fnmatch.fnmatch(rel_path, normalized_pattern[3:]):
        return True
    return False


def forced_include_reason(rel_path: str, include_files: set[str], include_globs: list[str]) -> str | None:
    if rel_path in include_files:
        return "matched --include-file"
    for pattern in include_globs:
        normalized_pattern = normalize_glob_pattern(pattern)
        if glob_matches(rel_path, normalized_pattern):
            return f"matched --include-glob {pattern}"
    return None


def normalize_glob_pattern(pattern: str) -> str:
    while pattern.startswith("./"):
        pattern = pattern[2:]
    return pattern


def has_nul_byte(path: Path) -> bool:
    try:
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                if b"\0" in chunk:
                    return True
    except OSError:
        return False
    return False


def read_initial_bytes(path: Path, limit: int = 8192) -> bytes:
    try:
        with path.open("rb") as handle:
            return handle.read(limit)
    except OSError:
        return b""


def is_text_like_sample(sample: bytes) -> bool:
    if b"\0" in sample:
        return False
    if not sample:
        return True
    try:
        sample.decode("utf-8")
    except UnicodeDecodeError:
        return False
    control_bytes = sum(byte < 32 and byte not in b"\n\r\t\f\b" for byte in sample)
    return control_bytes / max(len(sample), 1) < 0.01


def is_extensionless_script_candidate(rel_path: str, path: Path) -> bool:
    rel = Path(rel_path)
    if rel.suffix:
        return False
    sample = read_initial_bytes(path)
    if not is_text_like_sample(sample):
        return False
    if sample.startswith(b"#!"):
        return True
    return bool(set(part.lower() for part in rel.parts[:-1]) & SOURCE_SCRIPT_DIRS)


def is_high_signal_unknown(rel_path: str, path: Path) -> bool:
    rel = Path(rel_path)
    parts = {part.lower() for part in rel.parts[:-1]}
    is_top_level_operational = (
        len(rel.parts) == 1
        and (
            rel.name.startswith(".")
            or (not rel.suffix and rel.name.lower().endswith(("file", "rc")))
        )
    )
    if not is_top_level_operational and not (
        parts & (HIGH_SIGNAL_SOURCE_DIRS | SOURCE_SCRIPT_DIRS | HIDDEN_PROJECT_DIRS)
    ):
        return False
    return is_text_like_sample(read_initial_bytes(path))


def is_high_signal_ignored(rel_path: str, path: Path, include_config: bool, include_env: bool) -> bool:
    rel = Path(rel_path)
    parts = {part.lower() for part in rel.parts[:-1]}
    if is_ui_asset_path(rel_path):
        return True
    if not (parts & (HIGH_SIGNAL_SOURCE_DIRS | SOURCE_SCRIPT_DIRS | HIDDEN_PROJECT_DIRS)):
        return False
    if classify(rel_path, include_config, include_env) is not None:
        return True
    return is_extensionless_script_candidate(rel_path, path) or is_high_signal_unknown(rel_path, path)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def is_source_markdown(rel_path: str) -> bool:
    path = Path(rel_path)
    if path.suffix.lower() != ".md":
        return False
    if len(path.parts) == 1:
        return True
    if path.name.lower() in {name.lower() for name in SOURCE_MARKDOWN_FILENAMES}:
        return True
    return any(part.lower() in SOURCE_MARKDOWN_DIRS for part in path.parts[:-1])


def is_env_example_file(name: str) -> bool:
    stem, suffix = os.path.splitext(name)
    if suffix == ".env" and stem.lower() in ENV_EXAMPLE_BASENAMES:
        return True
    if ".env." in name:
        tokens = {part.lower() for part in name.split(".") if part}
        if tokens & ENV_EXAMPLE_TOKEN_MARKERS:
            return True
    return ".env." in name and name.endswith(ENV_EXAMPLE_MARKERS)


def is_secret_env_file(name: str) -> bool:
    if is_env_example_file(name):
        return False
    if name == ".envrc":
        return True
    if name == ".env" or name.endswith(".env"):
        return True
    if ".env." in name and not name.endswith(ENV_EXAMPLE_MARKERS):
        return True
    return False


def is_message_catalog_path(rel_path: str, suffix: str) -> bool:
    path = Path(rel_path)
    parts = {part.lower() for part in path.parts[:-1]}
    if suffix in MESSAGE_CATALOG_EXTENSIONS:
        return True
    return suffix in MESSAGE_CATALOG_CONFIG_EXTENSIONS and bool(parts & INTERFACE_TEXT_PARTS)


def classify(rel_path: str, include_config: bool, include_env: bool) -> str | None:
    path = Path(rel_path)
    name = path.name
    lower_name = name.lower()
    suffix = path.suffix.lower()

    if not include_env and is_secret_env_file(name):
        return None
    if is_source_markdown(rel_path):
        return "source/contract"
    if is_env_example_file(name):
        return "source/config"
    if include_env and is_secret_env_file(name):
        return "config"
    if name in LOCK_FILENAMES:
        return "source/config"
    if name in SOURCE_FILENAMES or lower_name == "dockerfile" or lower_name.startswith("dockerfile."):
        return "source/config"
    if is_message_catalog_path(rel_path, suffix):
        return "source/message-catalog"
    if suffix in SOURCE_EXTENSIONS or any(lower_name.endswith(item) for item in SOURCE_SUFFIXES):
        return "source"
    if include_config and (suffix in CONFIG_EXTENSIONS or name in SOURCE_FILENAMES):
        return "config"
    if include_config and (".env." in name or name.endswith(".schema.json")):
        return "config"
    return None


def filename_words(name: str) -> set[str]:
    stem = Path(name).stem
    spaced = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", stem)
    return {part.lower() for part in re.split(r"[^A-Za-z0-9]+", spaced) if part}


def is_static_build_config_filename(name: str) -> bool:
    """Identify JS/TS build configuration that has no rendered UI surface."""

    lowered = name.lower()
    return bool(
        re.fullmatch(
            r"(?:[a-z0-9_-]+\.)?config\.(?:[cm]?[jt]s|json|jsonc)",
            lowered,
        )
    )


def is_ui_asset_path(rel_path: str) -> bool:
    rel = Path(rel_path)
    suffix = rel.suffix.lower()
    if suffix not in UI_ASSET_EXTENSIONS:
        return False
    parts = {part.lower() for part in rel.parts[:-1]}
    return bool(parts & UI_ASSET_DIRS or filename_words(rel.name) & UI_ASSET_NAME_TOKENS)


def has_interface_key_markers(path: Path) -> bool:
    if path.suffix.lower() not in CONFIG_EXTENSIONS and path.suffix.lower() not in {".md"}:
        return False
    text = read_initial_bytes(path, limit=1_000_000).decode("utf-8", errors="ignore")
    for key in INTERFACE_KEY_MARKERS:
        if re.search(rf"(?i)(^|[\"'\s_-]){re.escape(key)}[\"'\s_-]*[:=]", text):
            return True
    return False


def is_interface_file(rel_path: str, fs_path: Path) -> bool:
    rel = Path(rel_path)
    parts = {part.lower() for part in rel.parts[:-1]}
    suffix = rel.suffix.lower()

    if is_ui_asset_path(rel_path):
        return True
    if suffix in INTERFACE_EXTENSIONS:
        return True
    if suffix == ".xml" and "res" in parts and parts & ANDROID_INTERFACE_DIRS:
        return True
    if is_message_catalog_path(rel_path, suffix):
        return True
    if rel.name in LOCK_FILENAMES or rel.name in SOURCE_FILENAMES:
        return False
    if is_static_build_config_filename(rel.name):
        return False
    if parts & NON_INTERFACE_SOURCE_PARTS:
        return has_interface_key_markers(fs_path)
    if parts & INTERFACE_TEXT_PARTS and suffix in CONFIG_EXTENSIONS:
        return True
    if parts & INTERFACE_TEXT_PARTS and suffix in SOURCE_EXTENSIONS | CONFIG_EXTENSIONS | MESSAGE_CATALOG_EXTENSIONS:
        return True
    if parts & INTERFACE_PATH_PARTS and suffix in SOURCE_EXTENSIONS | CONFIG_EXTENSIONS | MESSAGE_CATALOG_EXTENSIONS:
        return True
    if filename_words(rel.name) & set(INTERFACE_NAME_TOKENS):
        return True
    return has_interface_key_markers(fs_path)


def should_warn_excluded(rel_path: str, fs_path: Path, reason: str | None, include_config: bool, include_env: bool) -> bool:
    if reason is None:
        return False
    if reason == "not source-like":
        return is_high_signal_unknown(rel_path, fs_path)
    if reason.startswith("audit output directory excluded"):
        return False
    if reason.startswith("secret-bearing env file"):
        return False
    if reason == "local Claude settings excluded":
        return False
    if reason.startswith((
        "excluded generated/build directory",
        "excluded generated/build file",
        "excluded vendor directory",
    )):
        return False
    if reason == "binary/static asset extension":
        return is_ui_asset_path(rel_path)
    if reason == "binary file content":
        return is_ui_asset_path(rel_path)
    return classify(rel_path, include_config, include_env) is not None or is_interface_file(rel_path, fs_path)


def collect_files(
    repo: Path,
    include_config: bool,
    include_env: bool,
    include_generated: bool,
    include_vendor: bool,
    include_assets: bool,
    exclude_globs: list[str],
    include_files: set[str] | None = None,
    include_globs: list[str] | None = None,
    output_rel_dirs: list[str] | None = None,
) -> tuple[list[FileEntry], list[dict], list[dict[str, object]]]:
    output_rel_dirs = output_rel_dirs or []
    include_files = include_files or set()
    include_globs = include_globs or []
    git_ignored_paths: set[str] = set()
    deleted_tracked_paths: set[str] = set()
    rel_paths = run_git_files(repo)
    glob_forced_paths = {
        rel_path
        for rel_path in walk_for_include_globs(
            repo, include_generated, include_vendor, output_rel_dirs, include_globs
        )
        if forced_include_reason(rel_path, set(), include_globs)
    } if include_globs else set()
    if rel_paths is None:
        rel_paths = sorted(set(walk_files(repo, include_generated, include_vendor)) | include_files | glob_forced_paths)
    else:
        deleted_tracked_paths = set(run_git_deleted_paths(repo) or ())
        git_ignored_paths = set(run_git_ignored_files(repo, include_generated, include_vendor, output_rel_dirs))
        extra_paths = set(walk_secret_env_files(repo, include_generated, include_vendor))
        extra_paths.update(include_files)
        extra_paths.update(glob_forced_paths)
        if include_assets:
            extra_paths.update(rel_path for rel_path in git_ignored_paths if is_ui_asset_path(rel_path))
        if include_env or include_generated or include_vendor:
            extra_paths.update(walk_requested_extra_files(repo, include_env, include_generated, include_vendor))
        rel_paths = sorted(set(rel_paths) | extra_paths)

    entries: list[FileEntry] = []
    excluded: list[dict] = []
    tracked_deletions: list[dict[str, object]] = []
    excluded.extend(
        summarize_pruned_dirs(
            repo,
            include_config,
            include_env,
            include_generated,
            include_vendor,
            include_files,
            include_globs,
            output_rel_dirs,
        )
    )

    for rel_path in sorted(set(rel_paths)):
        force_reason = forced_include_reason(rel_path, include_files, include_globs)
        reason = (
            excluded_by_output_dir(rel_path, output_rel_dirs)
            or excluded_by_dir(rel_path, include_generated, include_vendor)
            or excluded_generated_file_reason(rel_path, include_generated)
            or matches_any_glob(rel_path, exclude_globs)
        )
        if force_reason and reason and not reason.startswith("audit output directory excluded"):
            if rel_path in include_files:
                reason = None
            elif reason.startswith(("excluded generated/build directory", "excluded vendor directory")):
                if include_glob_explicitly_targets_excluded_path(rel_path, include_globs):
                    reason = None
            else:
                reason = None
        path = repo / rel_path

        if reason is None and rel_path in deleted_tracked_paths and not path.exists():
            tracked_deletions.append(git_deleted_path_evidence(repo, rel_path))
            continue
        if reason is None and not path.exists():
            reason = "path does not exist"
        if reason is None and not path.is_file():
            reason = "not a regular file"
        if reason is None and path.is_symlink():
            reason = "symlink skipped"
        if reason is None and path.name in EXCLUDED_FILENAMES:
            reason = "excluded filename"
        if reason is None and is_local_tooling_file(rel_path):
            reason = "local Claude settings excluded"
        if reason is None and not include_env and is_secret_env_file(path.name):
            reason = "secret-bearing env file excluded; pass --include-env to audit intentionally"
        kind = None
        if reason is None and path.suffix.lower() in BINARY_EXTENSIONS:
            if include_assets and is_ui_asset_path(rel_path):
                kind = "source/ui-asset"
            else:
                reason = "binary/static asset extension"
        if reason is None and is_hidden_path(rel_path, include_generated) and not force_reason:
            reason = "hidden tooling directory"
        if reason is None and kind is None:
            if include_generated and is_generated_file(rel_path):
                kind = "generated/build metadata"
        if reason is None and kind is None:
            kind = classify(rel_path, include_config, include_env)
        if reason is None and kind is None and is_extensionless_script_candidate(rel_path, path):
            kind = "source/script"
        if reason is None and kind is None and force_reason:
            kind = "source/manual"
        if reason is None and kind is None:
            reason = "not source-like"
        if reason is None and kind != "source/ui-asset" and has_nul_byte(path):
            reason = "binary file content"

        try:
            size = path.stat().st_size if path.exists() else 0
        except OSError:
            size = 0

        if reason is None and kind is not None:
            try:
                interface_relevant = is_interface_file(rel_path, path)
                file_sha256 = sha256_file(path)
            except OSError as exc:
                reason = f"file became unreadable during scan: {exc}"

        if reason is None and kind is not None:
            entries.append(
                FileEntry(
                    rel_path=rel_path,
                    size_bytes=size,
                    kind=kind,
                    interface_relevant=interface_relevant,
                    sha256=file_sha256,
                )
            )
        else:
            excluded.append(
                {
                    "path": rel_path,
                    "reason": reason,
                    "size_bytes": size,
                    "scope_warning": should_warn_excluded(rel_path, path, reason, include_config, include_env),
                }
            )

    for rel_path in sorted(git_ignored_paths - set(rel_paths)):
        path = repo / rel_path
        if excluded_by_output_dir(rel_path, output_rel_dirs):
            continue
        if excluded_by_dir(rel_path, include_generated, include_vendor):
            continue
        if excluded_generated_file_reason(rel_path, include_generated):
            continue
        if not path.exists() or not path.is_file() or path.is_symlink():
            continue
        if path.name in EXCLUDED_FILENAMES:
            continue
        if is_local_tooling_file(rel_path):
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            excluded.append(
                {
                    "path": rel_path,
                    "reason": "local Claude settings excluded",
                    "size_bytes": size,
                    "scope_warning": False,
                }
            )
            continue
        if not is_high_signal_ignored(rel_path, path, include_config, include_env):
            continue
        try:
            size = path.stat().st_size
        except OSError:
            size = 0
        excluded.append(
            {
                "path": rel_path,
                "reason": "gitignored source-like file excluded by gitignore",
                "size_bytes": size,
                "scope_warning": True,
            }
        )

    return entries, excluded, sorted(tracked_deletions, key=lambda item: str(item["path"]))


def line_range_units_for(path: Path, max_unit_bytes: int) -> list[tuple[int, int, int]] | None:
    try:
        handle = path.open("rb")
    except OSError:
        return None
    chunks: list[tuple[int, int, int]] = []
    start_line = 1
    current_bytes = 0
    current_end_line = 0
    line_count = 0
    try:
        with handle:
            for index, line in enumerate(handle, start=1):
                try:
                    line.decode("utf-8")
                except UnicodeDecodeError:
                    return None
                line_count = index
                line_bytes = len(line)
                if line_bytes > max_unit_bytes:
                    return None
                if current_bytes and current_bytes + line_bytes > max_unit_bytes:
                    chunks.append((start_line, current_end_line, current_bytes))
                    start_line = index
                    current_bytes = 0
                current_bytes += line_bytes
                current_end_line = index
    except OSError:
        return None
    if line_count <= 1:
        return None
    if current_bytes:
        chunks.append((start_line, current_end_line, current_bytes))
    return chunks if len(chunks) > 1 else None


def audit_units_for(repo: Path, entries: list[FileEntry], max_unit_bytes: int) -> list[AuditUnit]:
    units: list[AuditUnit] = []
    for entry in entries:
        if entry.size_bytes <= max_unit_bytes:
            units.append(
                AuditUnit(
                    unit_id=entry.rel_path,
                    rel_path=entry.rel_path,
                    size_bytes=entry.size_bytes,
                    kind=entry.kind,
                    interface_relevant=entry.interface_relevant,
                    sha256=entry.sha256,
                )
            )
            continue

        line_units = line_range_units_for(repo / entry.rel_path, max_unit_bytes)
        if line_units:
            for start_line, end_line, size_bytes in line_units:
                units.append(
                    AuditUnit(
                        unit_id=f"{entry.rel_path}#L{start_line}-L{end_line}",
                        rel_path=entry.rel_path,
                        size_bytes=size_bytes,
                        kind=entry.kind,
                        interface_relevant=entry.interface_relevant,
                        sha256=entry.sha256,
                        start_line=start_line,
                        end_line=end_line,
                    )
                )
            continue

        chunk_count = max(2, (entry.size_bytes + max_unit_bytes - 1) // max_unit_bytes)
        for chunk_index in range(chunk_count):
            start_byte = chunk_index * max_unit_bytes + 1
            end_byte = min(entry.size_bytes, (chunk_index + 1) * max_unit_bytes)
            if start_byte > end_byte:
                continue
            units.append(
                AuditUnit(
                    unit_id=f"{entry.rel_path}#B{start_byte}-{end_byte}",
                    rel_path=entry.rel_path,
                    size_bytes=end_byte - start_byte + 1,
                    kind=entry.kind,
                    interface_relevant=entry.interface_relevant,
                    sha256=entry.sha256,
                    start_byte=start_byte,
                    end_byte=end_byte,
                )
            )
    return units


def batch_files(entries: list[AuditUnit], batch_size: int, max_batch_bytes: int) -> list[list[AuditUnit]]:
    if batch_size < 1:
        raise ValueError("--batch-size must be at least 1")
    if max_batch_bytes < 1:
        raise ValueError("--max-batch-bytes must be at least 1")

    batches: list[list[AuditUnit]] = []
    current: list[AuditUnit] = []
    current_bytes = 0

    for entry in entries:
        would_exceed_files = len(current) >= batch_size
        would_exceed_bytes = bool(current) and current_bytes + entry.size_bytes > max_batch_bytes
        if would_exceed_files or would_exceed_bytes:
            batches.append(current)
            current = []
            current_bytes = 0
        current.append(entry)
        current_bytes += entry.size_bytes

    if current:
        batches.append(current)

    return batches


def language_hint(entry: FileEntry | AuditUnit) -> str:
    suffix = Path(entry.rel_path).suffix.lower()
    if suffix:
        return suffix.removeprefix(".")
    return "config"


def purpose_for(entries: list[FileEntry] | list[AuditUnit]) -> str:
    top_dirs = Counter(Path(item.rel_path).parts[0] if len(Path(item.rel_path).parts) > 1 else "." for item in entries)
    kinds = Counter(item.kind for item in entries)
    langs = Counter(language_hint(item) for item in entries)

    dirs = ", ".join(name for name, _ in top_dirs.most_common(3))
    kind_text = ", ".join(name for name, _ in kinds.most_common(3))
    lang_text = ", ".join(name for name, _ in langs.most_common(4))
    interface_count = sum(1 for item in entries if item.interface_relevant)
    interface_text = f" Includes {interface_count} interface-relevant file(s)." if interface_count else ""
    return f"Audit {kind_text} files mostly under {dirs}; primary file types: {lang_text}.{interface_text}"


def render_interface_focus(entries: list[FileEntry] | list[AuditUnit]) -> str:
    interface_entries = sorted({entry.rel_path for entry in entries if entry.interface_relevant})
    if not interface_entries:
        return ""

    file_lines = "\n".join(f"- `{rel_path}`" for rel_path in interface_entries)
    return f"""
## Interface Audit Focus

These files are likely to define UI, visible copy, navigation, forms, or interface behavior:

{file_lines}

For these files, inventory visible product promises and trace them to implementation:
- Buttons, icon buttons, menu items, command items, tabs, links, and shortcuts.
- Text fields, selectors, filters, uploads, toggles, settings, and forms.
- Toasts, banners, empty states, tooltips, helper text, validation text, success messages, and error messages.
- Loading, empty, error, permission denied, background job, undo/redo, and destructive confirmation states.

Flag interface elements that are unimplemented, handler-only placeholders, console-only behavior, disabled dead ends, not persisted, not validated, not reflected in API/state, misleadingly labeled, inaccessible, or wired to the wrong route/action. Include the exact visible label or message text whenever possible.
Record the result in the required `Interface Inventory` section even when no gap is found.
"""


def render_journey_file_list(entries: list[FileEntry]) -> str:
    if not entries:
        return "- No interface-relevant files were detected."
    return "\n".join(
        f"- `{entry.rel_path}` ({entry.kind}, {entry.size_bytes} bytes, sha256=`{entry.sha256}`)"
        for entry in entries
    )


def render_journey_source_prompt(
    repo: Path,
    run_id: str,
    entries: list[FileEntry],
    report_path: Path,
) -> str:
    return f"""# Full Repo Audit User Journey Source Worker

Repo root: `{repo}`
Run ID: `{run_id}`

{artifact_delivery_contract(report_path)}
{isolated_light_worker_contract()}

You are a separate low-effort worker focused on user journeys through the UI. Do not edit the audited repository; write only the exact audit artifacts authorized above. Use the interface-relevant source files below, plus repo docs/routes/config when needed, to determine whether the app describes complete user journey(s), required feature/UI elements, and test expectations, and whether the UI source supports them.

## Interface-Relevant Files

{render_journey_file_list(entries)}

## Tasks

1. Find any explicit user journey, feature inventory, UI element inventory, onboarding, workflow, route map, product-flow description, analytics note, support/common-task doc, test expectation, or source-backed route flow in the repo.
2. If journey documentation is missing or incomplete, draft all reasonable frequent journey(s) from app intent, routes, visible copy, and code. Mark each as `draft-needs-user-confirmation`; do not treat drafted journeys as confirmed product truth.
3. Walk the UI source along every confirmed or drafted journey. For each required feature, visible navigation element, action, decision element, field, summary, detail, warning, and empty/error/loading state you mention, estimate relevance for that journey: `critical-always`, `primary-frequent`, `secondary-occasional`, or `rare-under-5-percent`.
4. Record UI assumption status for each journey/surface as `confirmed`, `source-inferred`, or `missing`. Do not convert source-inferred layout into product truth.
5. Compare what the user currently sees against the journey decision model: primary decision, required facts, warning/flag conditions, frequent actions, secondary/rare actions, and unconfirmed assumptions.
6. Check whether each journey step gives users enough information on desktop, native, and mobile surfaces to make the documented decision. Rare or conditional information should remain reachable through an appropriate detail path; threshold warnings/notices must be available at the decision point.
7. Check compactness and responsive fit: critical journey information and primary actions should fit without accidental horizontal scroll, overlap, cropping, truncation, hidden overflow without a scroll path, unreadable compression, low contrast, invisible theme text, or displacement by decorative/low-relevance content.
7. Check interaction and metadata affordances anywhere the UI contains badges, flags, expandable rows, scrollable details, message streams, tool/result blocks, copy controls, navigation rows, or icon-only controls. Explicitly mark these checklist labels as `pass`, `gap`, `blocked`, or `not applicable`: `badge-detail`, `row-hit-target`, `navigation-cursor`, `transient-disclosure`, `disclosure-scrollbar`, `icon-meaning`, `stable-expansion-width`, `hover-copy`, `status-summary`, `message-metadata`.
8. Check whether the interface exposes a test mode or lightweight fixture path for visually exercising the journey without heavy load or production side effects.
9. Report concrete gaps with file evidence and visible copy when possible, including missing UI elements, unwired implementation paths, interaction checklist gaps, or missing test evidence. Treat unconfirmed journeys as open questions or assumption-based coverage, not as clean UI proof.

## Required Report File

Write Markdown with exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
journey_source

## Journey Sources
List source files/docs/routes that define or imply the journeys.

## Proposed Journeys
List every confirmed journey and every `draft-needs-user-confirmation` journey. Include target user, goal, entry point, route/screen sequence, primary decision points, required feature/UI elements, rare detail needs, UI assumption status (`confirmed`, `source-inferred`, or `missing`), test expectations, and success/failure end states.

## UI Source Journey Checks
Use a Markdown table with exactly these columns:
| Journey | Step | Files | Primary navigation/decision elements | Relevance estimate | Required information | Interaction and metadata checklist | Mobile/Desktop availability | Test mode evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |

The `Interaction and metadata checklist` cell must include the exact labels `badge-detail`, `row-hit-target`, `navigation-cursor`, `transient-disclosure`, `disclosure-scrollbar`, `icon-meaning`, `stable-expansion-width`, `hover-copy`, `status-summary`, and `message-metadata` with `pass`, `gap`, `blocked`, or `not applicable` values when the surface has badges, flags, expandable rows, scrollable details, message streams, tool/result blocks, copy controls, navigation rows, or icon-only controls.

## Findings
Use P0/P1/P2/P3 finding blocks with the exact fields below, or `No findings.`:
### P0/P1/P2/P3 - Short title
- Files: `repo-relative source file`
- Evidence: concrete source, route, visible-copy, journey, or test-mode detail
- Interface evidence: visible label/control/message text when applicable, or `Not applicable`
- Expected behavior/standard: expected user journey behavior or standard
- Gap: what is unclear, incomplete, inaccessible, missing, unimplemented, untested, or wrongly prioritized
- Suggested direction: concise fix direction

## Open Questions
Ask the lead to clarify the most frequent use cases when journey information is missing or ambiguous.
"""


def render_visual_journey_prompt(
    repo: Path,
    run_id: str,
    entries: list[FileEntry],
    report_path: Path,
) -> str:
    return f"""# Full Repo Audit Visual Journey Worker

Repo root: `{repo}`
Run ID: `{run_id}`

{artifact_delivery_contract(report_path)}
{isolated_light_worker_contract()}

Authorized visual evidence manifest: `{(report_path.parent.parent / 'visual_evidence.json').resolve()}`.
Screenshot, formal-verifier, journey-evidence, changed-review queue, decision, and manual-review artifacts may be written only beneath the same
audit-output directory and must be registered in that manifest.

You are a separate low-effort worker focused on visual journey verification. Do not edit the audited repository; write only the exact audit artifacts authorized above. Check the visual surface against complete journey, feature, UI element, and test expectations. If the repo has a test mode, fixture mode, Playwright/browser automation, MCP browser tooling, or another safe visual test path, use or recommend that test mode before any production/heavy-load path. If no visual tool or no test mode is available, report the blocker as a finding or open question with evidence.

For CLI, library, plugin, or skill packages that expose only metadata/Markdown and no repo-owned rendered UI surface, mark visual desktop/mobile checks as `not applicable` with evidence instead of treating host-owned rendering as a repo defect. Still report a finding if the package promises a visual surface, ships screenshots/previews, or contains UI source without a safe visual test path.

## Interface-Relevant Files

{render_journey_file_list(entries)}

## Tasks

1. Identify available visual test tooling: Playwright, Cypress, Storybook, browser MCP tools, native UI preview/test harnesses, screenshots, `formal-web-ui-verification`, or documented local dev/test mode.
2. Prefer test mode or fixture data. Use production mode only when the user explicitly instructed it or no side-effecting/heavy path is involved.
3. For each confirmed or drafted high-frequency journey, verify or plan a visual walk through desktop and narrow mobile viewports, including required UI elements, critical states, and expected test evidence.
4. Check that navigation and primary decision elements are visible, relevant, accessible, and prioritized ahead of less probable routes, while recording whether the UI assumption is confirmed or only source-inferred.
5. Check that enough decision-making information is visible, critical information is not cropped/hidden/unreadable, rare information is reachable through an appropriate detail path, and threshold warnings are available at the decision point.
6. Check compactness: critical journey information and primary actions should fit on desktop, native, and mobile surfaces without overlap, accidental horizontal scroll, hidden overflow without scrolling, unreadable compression, low contrast, invisible theme text, nested visual frames, unstable disclosure controls, or being buried under decorative/detail/debug/low-relevance content.
7. Check interaction affordances: decision badges/flags should react to hover/focus/click and reveal useful detail when interactive; meaningful rows should activate as rows rather than tiny icon-only targets; navigational explanations/rows should have a predictable destination and pointer/focus affordance; temporary popovers and expanded panels should have an intentional close or timeout lifecycle; expand/collapse controls should not overlap or fight scrollbars; expanded/collapsed tool or result blocks should keep stable widths; copy controls should not permanently clutter message reading and should stay reachable when revealed; concise status blocks should avoid duplicate status/severity/duration noise; and timestamps or passive metadata should not be selectable content unless the journey justifies it. Explicitly mark these checklist labels for every relevant visual/source-inferred surface: `badge-detail`, `row-hit-target`, `navigation-cursor`, `transient-disclosure`, `disclosure-scrollbar`, `icon-meaning`, `stable-expansion-width`, `hover-copy`, `status-summary`, `message-metadata`.
8. Check broad layout quality: overload, crowding, ambiguous hierarchy, clipped/truncated text, oversized controls, unscannable information, nested cards/blocks, border/background stacks, grid/alignment discipline, stable expand/collapse controls, icon meaning, instruction noise, avatar/decorative clutter, message alignment, sender/routing label noise, theme consistency, readable font sizes, contrast, text/background colors, scrollability for overflow, menu/detail usability, and whether heavy-load actions can be avoided in test mode.
9. When a web UI has a safe render path, run `formal-web-ui-verification` with a complete per-target journey, theme, region, continuation, and declared UI-input contract, or explicitly report why formal verification is blocked. Treat unresolved critical formal findings for journey displacement, offscreen/unfocused continuation, WCAG contrast, theme contradiction, clipping, hidden controls, overlap, off-canvas controls, broken media, invisible text, horizontal overflow, or area violations as gaps. Always record the verifier's visible scrollbar and palette-risk inventory as evidence.
10. Finish the formal verifier and every other automatic test before manual image review. Read its `review-queue.json`, then open only the queued cell's initial-viewport and full-page screenshots. Do not reopen carried unchanged screenshots. Record `pass`, `gap`, or `blocked` decisions, finalize them with `formal_web_ui_review.py`, and preserve unchanged prior gaps as findings. Screenshot/pixel hashes prove integrity only; changed declared UI inputs, journey/theme intent, or a new route/state/viewport trigger review.
11. When visual checks are applicable, populate `visual_evidence.json` with stable records for every screenshot, formal-verifier JSON report, `journey-evidence`, `review-queue`, and `manual-review` artifact. The journey-evidence record must bind the ordered route/state/viewport cells that the Console can replay; it never replaces the formal report or manual review. Record confined relative paths, SHA-256, MIME, actual image dimensions and route/state/viewport metadata where applicable, plus capture tool. Cite each real artifact as `evidence:<id>` in the applicable row. Filenames or screenshot keywords alone are rejected. If required screens, elements, states, formal checks, review evidence, or visual tests are missing, report them as gaps. If no repo-owned rendered UI exists, explicitly mark checks `not applicable` with evidence.

## Required Report File

Write Markdown with exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
visual_journey

## Visual Tooling
List tools/modes found, commands considered or run, formal Web UI verifier status, visible scrollbar inventory when available, and blockers.

## Visual Journey Checks
Use a Markdown table with exactly these columns:
| Journey | Viewport | Route/screen | Evidence | Navigation visibility | Decision information | Interaction and metadata checklist | Visual quality | Result |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |

The `Interaction and metadata checklist` cell must include the exact labels `badge-detail`, `row-hit-target`, `navigation-cursor`, `transient-disclosure`, `disclosure-scrollbar`, `icon-meaning`, `stable-expansion-width`, `hover-copy`, `status-summary`, and `message-metadata` with `pass`, `gap`, `blocked`, or `not applicable` values for relevant surfaces. A `gap` or `blocked` value must have a finding.

## Changed Visual Review
Use a Markdown table with exactly these columns:
| Review cell | Trigger/status | Initial viewport evidence | Full-page evidence | Decision | Note |
| --- | --- | --- | --- | --- | --- |

For each queued web cell, cite both screenshot evidence ids and record `pass`, `gap`, or `blocked`. For carried unchanged cells, write `not reopened — carried unchanged` in both evidence columns and use `carried-pass`, `carried-gap`, or `carried-blocked`. Cite the formal report, journey-evidence, review queue, and manual-review manifest evidence ids in this section. Every gap/blocked or carried-gap/carried-blocked row requires a finding.

## Findings
Use P0/P1/P2/P3 finding blocks with the exact fields below, or `No findings.`:
### P0/P1/P2/P3 - Short title
- Files: `repo-relative source file`
- Evidence: concrete visual-tooling, source, route, screenshot/trace/artifact, or not-applicable detail
- Interface evidence: visible label/control/message text when applicable, or `Not applicable`
- Expected behavior/standard: expected visual journey behavior or standard
- Gap: what is unclear, incomplete, inaccessible, missing, unimplemented, untested, overloaded, crowded, clipped, truncated, low-contrast, hidden by overflow, unreadable, or visually unverified
- Suggested direction: concise fix direction

## Open Questions
List missing test-mode or journey clarifications for the lead.
"""


def render_lead_reconciliation_prompt(repo: Path, run_id: str, report_path: Path) -> str:
    return f"""# Full Repo Audit Lead Reconciliation

Repo root: `{repo}`
Run ID: `{run_id}`

{lead_artifact_delivery_contract(report_path)}

This is a required lead-owned reconciliation artifact, not a low-effort batch.
Complete it only after reading every batch report and, when applicable, both
journey reports. Independently reopen the assigned source and recheck every
batch `PASS` responsibility anchor; a batch conclusion is a lead, not a
substitute for this lead-owned review. Rechecks may be completed incrementally,
but sampling PASS rows is not sufficient for full semantic coverage. Do not
edit the audited repository; write only the exact audit artifact authorized
above.

Trace the repository's real feature, API, command, route, job, event,
configuration, schema/migration, build/deploy, and operational contracts across
file and batch boundaries. Find semantic implementation gaps even when the
source contains no TODO, placeholder, stub, or `NotImplemented` marker. In
particular, challenge hard-coded values that should be calculated, ignored
inputs/configuration, fake success, incomplete registration or dependency
plumbing, memory-only state presented as durable, missing external effects,
partial read/write paths, production mocks/fixtures, missing lifecycle work,
and tests that prove only shape rather than the promised outcome.

Every finding must describe exactly one independently closable implementation
outcome. Do not save a compound finding. Reissue it as separate atomic finding
blocks, each with its own evidence, gap, and verification direction.

## Required Report File

Write Markdown with exactly these sections to the report path above:

## Run ID
{run_id}

## Worker
lead_reconciliation

## Cross-File Contract Trace
For a non-empty repository, include one row per distinct cross-file, public,
file-local, or operational contract reconciled by the lead. Assign sequential
unique IDs in the `lead:C<3+ digits>` namespace (for example `lead:C001` and
`lead:C1000`). Map every batch `Contract ID` from
every verified Implementation Inventory to exactly one lead row; group IDs only
when they jointly implement the same end-to-end contract. No batch Contract ID
may be omitted or mapped twice.

| Contract ID | Batch Contract IDs | Contract/source anchors | entry-registration | core-logic | data-lifecycle | integration-boundary | authorization-trust | failure-recovery | observable-outcome | operational-lifecycle | verification | Result |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `lead:C<3+ digits>` (for example `lead:C001`) | one or more exact backticked `batch_<3+ digits>:C<3+ digits>` IDs | concrete repo-relative files and at least one backticked source token or occurrence anchor from every mapped batch row | `pass —` registration/caller evidence, or another allowed status plus evidence | `pass —` real calculation/domain evidence, or another allowed status plus evidence | `pass —` validation/ownership/persistence evidence, or justified `not applicable —` evidence | `pass —` dependency/API/event evidence, or justified `not applicable —` evidence | `pass —` permission/trust evidence, or justified `not applicable —` evidence | `pass —` failure/retry/rollback/cleanup evidence, or another allowed status plus evidence | `pass —` real user-visible/API/operational result citing at least one backticked source token or occurrence anchor from every mapped batch row, or another allowed status plus the same per-row anchors | `pass —` startup/scheduling/deploy/monitoring/recovery evidence, or justified `not applicable —` evidence | begin with `pass —`; include `evidence-type: test`; include the literal label evidence-ref followed by backticked `tests/contract_test.py`; include `outcome: the named test asserted the expected changed result`; include `counterfactual: changed input or state must produce the expected changed outcome`; then give independent lead-recheck evidence and at least one backticked source token or occurrence anchor from every mapped batch row | PASS, GAP, or BLOCKED |

For each of the nine trace-label cells, begin with exactly `pass —`, `gap —`, `blocked —`, or `not applicable —`, then give concrete evidence or justification. Derive `Result` from those cells: any `gap` means `GAP`; otherwise any `blocked` means `BLOCKED`; otherwise use `PASS`. Preserve at least one concrete source token or occurrence anchor from every mapped batch row in the lead row's `Contract/source anchors`, `observable-outcome`, and `verification` cells; one shared token cannot stand in for grouped responsibilities. Every lead `verification` cell must also declare exactly one `evidence-type: test`, `evidence-type: runtime`, or `evidence-type: source-only` and exactly one concrete `counterfactual: ...` or `invariance: ...`. A `test` claim must include exactly one `evidence-ref: ...` declaration containing backticked repo-relative test file path(s) present in the manifest. A `runtime` claim must instead cite backticked `evidence:<id>` reference(s) bound to valid artifacts in `visual_evidence.json`; never invent a runtime artifact or cite an unbound filename. A lead `PASS` using test or runtime evidence must also contain exactly one explicit `outcome: ...` or `result: ...` statement naming the observed result, not merely saying that a check exists or passed. This evidence records the independent lead recheck, not merely the batch conclusion. A lead `PASS` for persistence, integration, an external effect, or success requires test or runtime evidence and must never use `source-only`. A `PASS` row must mark both `observable-outcome` and `verification` as `pass`; `not applicable` cannot substitute for either clean-result proof, and an all-`not applicable` row is never a pass. A mapped batch `GAP` cannot become lead `PASS` or `BLOCKED`, and a mapped batch `BLOCKED` cannot become lead `PASS`; keep it unresolved for lead disposition. For an empty manifest, write exactly `No source-backed implementation contracts were queued.` instead of rows. Use `PASS` only when the full contract reaches its real outcome. Use `GAP` for marker-free semantic gaps as well as explicit stubs. Use `BLOCKED` only for a named external condition. Every `GAP` or `BLOCKED` row must have a finding that cites its Contract ID.

## Findings
Use one atomic subsection per confirmed lead-reconciliation issue, or exactly
`No findings.` when every traced contract passes:
### P0/P1/P2/P3 - Short title
- Files: `repo-relative source file`
- Evidence: concrete source symbols, values, branches, registrations, calls, persistence, runtime behavior, or tests
- Interface evidence: exact visible label/control/message when applicable, or `Not applicable`
- Expected behavior/standard: the real contract or outcome the implementation must provide
- Gap: the one independently closable missing, partial, substituted, unwired, unverified, or unreachable outcome
- Suggested direction: concise implementation and end-to-end verification direction

## Open Questions
List unresolved ambiguities or exactly `None.`. Questions and hypotheses are not findings and do not enter the completion ledger.
"""


def render_batch_prompt(
    repo: Path,
    run_id: str,
    batch_id: int,
    total_batches: int,
    entries: list[AuditUnit],
    report_path: Path,
) -> str:
    file_lines = "\n".join(
        (
            f"- Unit `{entry.unit_id}`: `{entry.rel_path}` lines {entry.start_line}-{entry.end_line} "
            f"({entry.kind}, approx {entry.size_bytes} bytes in this range, interface={str(entry.interface_relevant).lower()}, full-file sha256=`{entry.sha256}`)"
            if entry.start_line is not None and entry.end_line is not None
            else f"- Unit `{entry.unit_id}`: `{entry.rel_path}` bytes {entry.start_byte}-{entry.end_byte} "
            f"({entry.kind}, {entry.size_bytes} bytes in this range, interface={str(entry.interface_relevant).lower()}, full-file sha256=`{entry.sha256}`)"
            if entry.start_byte is not None and entry.end_byte is not None
            else f"- Unit `{entry.unit_id}`: `{entry.rel_path}` ({entry.kind}, {entry.size_bytes} bytes, interface={str(entry.interface_relevant).lower()}, sha256=`{entry.sha256}`)"
        )
        for entry in entries
    )
    range_text = ""
    if any(entry.start_line is not None or entry.start_byte is not None for entry in entries):
        range_text = """
## Range Review Scope

Some owned units are line or byte ranges from oversized files. Inspect the assigned range manually, plus nearby imports/types/callers only as needed to understand that range. In `File Coverage`, use the exact unit id from `Files You Own` for ranged units, for example `path.py#L1-L200` or `path.js#B1-B120`. In findings, reference the real repo file path in `Files` and cite the range in `Evidence`.
The exact ranged unit id must also appear in either `Findings` or `No Finding Notes` so the verifier can prove that this specific range was manually checked.
"""
    return f"""# Full Repo Audit Batch {batch_id:03d}/{total_batches:03d}

Repo root: `{repo}`
Batch purpose: {purpose_for(entries)}

{artifact_delivery_contract(report_path)}
{isolated_light_worker_contract()}

You are a low-effort subagent performing a manual source-code audit for only this batch. Do not edit the audited repository; write only the exact audit artifact authorized above. Inspect every listed file directly and report only evidence you can tie to these files.

## Cross-Batch Verification Guardrail

Consult `{(report_path.parent.parent / 'test_evidence_index.json').resolve()}`
before calling an owned responsibility untested. Inspect the named candidate
tests, then make at most one behavior-specific manifest search when the index
does not resolve the question. Tests in another batch may provide evidence but
remain owned by that batch. Browser fixtures prove only client behavior; they
do not prove persistence or external effects.

Run only a relevant, bounded test. Preserve its completed result and exit
status; partial streamed output is not a result. On environment failure, try
one non-destructive isolated workaround. If no assertion or source defect is
established, record one BLOCKED contract with the command and unblock condition,
not repeated product findings. Inspect language, SDK, browser, or database
runtime details only when the assigned files actually use that runtime or
boundary, and never treat a shared database as disposable.

## Pre-return Report Gate

Do not call this batch complete from a prose review alone. Use one row per
distinct contract responsibility. Multiple internal definitions may share a
row only when the trace explains their role in the same outcome; separate entry
points or independently observable outcomes remain separate rows. Each detected
definition anchor must appear in a row's source anchors, ordered trace, and
verification. Every GAP/BLOCKED has exactly one matching atomic finding.

Anchor provenance is strict: copy a `name@L...:C...` anchor only when the
parser recognizes that exact occurrence in the assigned source. Never invent a
line or column such as `C1`. For JSON, Markdown, configuration, assets, and
other manual/declarative responsibilities without a recognized occurrence, use
`Discovery: manual` and an exact raw backticked token that literally occurs in
the assigned unit (for example `lockfileVersion`), then repeat that same raw
token in source anchors, trace, and verification. A synthesized
`lockfileVersion@L...:C...` token is invalid because it is neither source text
nor a parser-recognized occurrence.

Save the draft, then validate only this batch:
`verify_audit_results.py --manifest <audit-output>/manifest.json --batch-id
batch_{batch_id:03d} --reports <exact-report-path> --json`.
Repair any reported batch issue before returning. The lead runs the complete
directory verifier after all reports and reconciliation artifacts exist.

## Files You Own

{file_lines}
{range_text}
{render_interface_focus(entries)}

## Audit Questions

For each file, check:
- What user-facing, system, or build responsibility does this file appear to own?
- If it defines interface, what visible controls, fields, navigation items, and messages does it expose?
- Which requirements, public APIs, package exports, commands, routes, events, jobs, schedulers, configuration keys, schemas, migrations, feature flags, build/install paths, deployment paths, and recovery paths does it define or help fulfill?
- Trace each applicable responsibility from registration or entry point through validation and domain logic to dependencies, persistence or side effects, observable output, failure/recovery behavior, authorization/trust checks, and verification. A present symbol or test name is a lead, not proof that the path works.
- Is anything stubbed, TODO-only, unreachable, mocked as real behavior, dead-ended, or only partially wired?
- Is configuration parsed but ignored, a route/job/export implemented but never registered, state described as saved but kept only in memory, an exception swallowed into apparent success, authorization enforced only in presentation code, a production boundary backed by fixtures/mocks, or lifecycle behavior such as migration, rollback, retry, cancellation, cleanup, backup, or recovery missing?
- Even when no marker advertises a gap, does the code return a hard-coded value where the contract requires a calculation, ignore parameters or parsed data, use a constant status/identifier/result for every case, leave a branch or lifecycle phase unwired, pass data through plumbing without invoking the real dependency, or test only response shape without proving the intended state change or outcome?
- Could the implementation violate common industry expectations for correctness, reliability, security, accessibility, performance, maintainability, or testability?
- Could a user reasonably expect behavior that this code does not fully provide?
- Does this code fully provide the intended feature, UI element, journey step, state, handler, persistence path, permission path, and test evidence it implies?
- Are errors, loading states, permissions, data validation, migrations, observability, or edge cases missing?
- Are tests absent or too shallow for the behavior this file owns?

## Required Report File

Write Markdown with exactly these sections to the report path above:

## Run ID
{run_id}

## Batch ID
batch_{batch_id:03d}

## Batch Summary
Briefly describe what this batch appears to do.

## File Coverage
One row per file or range unit listed above:
| File | Status | SHA-256 | Purpose |
| --- | --- | --- | --- |
| `path`, `path#Lstart-Lend`, or `path#Bstart-end` | CHECKED or UNCHECKED | `sha256 from Files You Own` | one-line purpose |

## Implementation Inventory
Include at least one row for every file or range unit listed above, using the exact unit id. Add a separate row for every distinct contract, responsibility, entry point, calculation, state transition, integration, or operational path in that unit; repeat the unit id as needed and assign sequential unique Contract IDs in the `batch_<3+ digits>:C<3+ digits>` namespace. Do not truncate counters after 999: `batch_1000:C1000` is valid. You may inspect adjacent callers and callees for context, but do not claim coverage for files outside this batch.

| File/unit | Contract ID | Contract/responsibility | Entrypoints/source anchors | Implementation/data/side-effect trace | Failure/edge/permission/recovery trace | Verification evidence | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `path`, `path#Lstart-Lend`, or `path#Bstart-end` | `batch_<3+ digits>:C<3+ digits>` (for example `batch_{batch_id:03d}:C001`) | one concrete responsibility; Basis: public-contract — `docs/requirements.md#L12-L18`; Discovery: parsed — `definition@L12:C5` or Discovery: manual — `exact literal source token` | concrete backticked symbols/tokens from the assigned source unit; every parsed definition uses its exact parser-recognized occurrence anchor such as `definition@L12:C5` (or `definition@B123` for a byte unit), while manual/declarative responsibilities use an exact raw token present in the file such as `lockfileVersion`; use the unit SHA-256 only for an empty/non-text byte unit | `pass —` / `gap —` / `blocked —` / `not applicable —` then entry/registration -> validation/core logic -> dependency/persistence/side effect -> observable outcome | the same status prefix, then concrete failure, edge, authorization, trust, retry, rollback, cancellation, cleanup, or justified non-applicability evidence | the same status prefix, then `evidence-type: test`, `evidence-type: runtime`, or `evidence-type: source-only`; when test/runtime, one `evidence-ref: ...` declaration; when test/runtime PASS, one explicit `outcome: ...` or `result: ...`; exactly one concrete `counterfactual: ...` or `invariance: ...`; and a named behavior check tied to the anchors | PASS, GAP, or BLOCKED |

Every `Contract/responsibility` cell must include exactly one explicit basis and
one discovery statement. Use `Basis: <kind> — <concrete backticked reference>`,
where `<kind>` is `user-requirement`, `acceptance-criterion`,
`recorded-decision`, `public-contract`, `interface-promise`,
`caller-contract`, `schema-invariant`, `operational-contract`, or
`source-inferred`. The first eight kinds identify authoritative intent;
`source-inferred` must be labeled honestly when no authoritative contract is
available. For every non-`source-inferred` kind, every backticked Basis
reference must resolve to a manifest-owned repository file, optionally with a
precise `#Lx-Ly`, `#Bx-By`, or `#symbol` suffix. A user statement that was not
materialized in a queued requirement or decision artifact cannot be presented
as mechanically authenticated: use honest `source-inferred`/manual discovery
or mark the contract `BLOCKED`. Use `Discovery: parsed — ...` only when the row cites a recognized
occurrence-aware named-definition anchor in its assigned source. Format the
anchor as `<name>@L<absolute-line>:C<column>`, or `<name>@B<absolute-byte>` for
a byte-range unit, and repeat that exact backticked anchor in the row's source
anchors, implementation trace, and verification evidence. Occurrences that
jointly implement the same contract outcome may share a row only when every
anchor is repeated through that row's trace and verification. Separate entry
points or independently observable outcomes require separate rows. Otherwise use `Discovery:
manual — ...` and cite an exact raw backticked token that appears literally in
the assigned source unit. Do not add invented `@L...:C...` coordinates to a
manual token. Every high-confidence named definition must appear in a row.
Declarative, unsupported, or unparsed responsibilities require manual discovery.
If they cannot be manually enumerated, record `BLOCKED` and the exact unblock
condition rather than a clean result.

Begin each of the three trace/evidence cells with exactly `pass —`, `gap —`, `blocked —`, or `not applicable —`. Set `Result` mechanically: any `gap` cell means `GAP`; otherwise any `blocked` cell means `BLOCKED`; otherwise use `PASS`. Every `Verification evidence` cell must declare exactly one `evidence-type: test`, `evidence-type: runtime`, or `evidence-type: source-only` and exactly one concrete `counterfactual: ...` or `invariance: ...`. A `test` claim also requires exactly one `evidence-ref: ...` declaration containing only backticked repo-relative test source path(s) present in the manifest. A `runtime` claim requires the same declaration with only backticked `evidence:<id>` reference(s) bound to valid audit artifacts in `visual_evidence.json`; never invent runtime artifacts. Every `PASS` using test or runtime evidence additionally requires exactly one explicit `outcome: ...` or `result: ...` statement naming what the cited check observed, not merely that it passed. A counterfactual states which changed input, configuration, state, dependency response, or failure must change the result; an invariance explains why the result must remain constant. `PASS` for persistence, integration, external-effect, or success claims requires test or runtime evidence and must never use `source-only`. A `PASS` row must mark both `Implementation/data/side-effect trace` and `Verification evidence` as `pass`; only a genuinely inapplicable failure/edge/permission/recovery path may be `not applicable`, and an all-`not applicable` row is never a pass. Use `PASS` only when that one responsibility is semantically implemented—not merely present or free of TODO markers—and the row cites concrete behavior-specific verification evidence. Follow inputs and configuration into the real calculation/domain behavior, state/persistence or external side effect, and observable result. Inventory every high-confidence named definition occurrence in the assigned source; do not cover only a working helper while omitting another entry point. Use `GAP` for hard-coded substitutes, ignored inputs, incomplete plumbing, fake success, memory-only persistence presented as durable, test fixtures/mocks on production paths, missing branches/lifecycle work, missing/partial/unverified/unreachable behavior, or tests that prove only shape. Use `BLOCKED` only when an external condition prevents determining or completing the path, and name that condition. Every `GAP` or `BLOCKED` row must have a finding bound to the real repository file and cite its Contract ID; include the exact range unit too when applicable. Documentation, configuration, assets, package markers, and declarative files still require a concrete responsibility/consumer trace rather than an implementation-inventory sentinel.

## Interface Inventory
For batches with interface-relevant files, include one or more rows for every interface-relevant file:
| File | Surface | Visible text/control/message | Expected behavior path | Actual implementation notes |
| --- | --- | --- | --- | --- |
| `path` | control/message/prompt/navigation/state/none | exact visible text or `None found` | handler/state/API/persistence/verification expected | implemented/missing behavior evidence |

For batches without interface-relevant files, write exactly:
No interface-relevant files in this batch.

## Findings
Use one subsection per independently closable issue. Every finding must cite
exactly one `GAP` or `BLOCKED` Contract ID from this batch; never combine
multiple responsibilities or completion outcomes in one block. Reissue
compound issues as separate atomic blocks before returning the report:
### P0/P1/P2/P3 - Short title
- Files: `path`
- Evidence: concrete code detail, symbol, route, state, TODO, or behavior
- Interface evidence: visible label/control/message text when applicable
- Expected behavior/standard: expected behavior, feature, UI element, journey, or test standard this code should meet
- Gap: what is incomplete, partial, missing, unimplemented, untested, non-standard, or likely surprising
- Suggested direction: concise fix direction

## No Finding Notes
List files checked with no notable issue.

## Open Questions
List ambiguities the lead agent should resolve.
"""


def read_ownership_marker(out_dir: Path) -> dict | None:
    marker_path = out_dir / ARTIFACT_MARKER
    if not marker_path.is_file():
        return None
    try:
        marker = json.loads(marker_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return marker if isinstance(marker, dict) else None


def ensure_output_dir_safe(out_dir: Path, repo: Path) -> dict | None:
    if not out_dir.exists():
        return None
    if not out_dir.is_dir():
        raise ValueError(f"Output path exists but is not a directory: {out_dir}")
    if not any(out_dir.iterdir()):
        return None
    marker = read_ownership_marker(out_dir)
    if marker and marker.get("owned_by") == ARTIFACT_OWNER:
        if marker.get("repo_root") != str(repo):
            raise ValueError(
                f"Output directory is marked as {ARTIFACT_OWNER}-owned for a different repo: {out_dir}"
            )
        return marker
    raise ValueError(
        f"Output directory is non-empty and is not marked as {ARTIFACT_OWNER}-owned: {out_dir}. "
        "Choose a new empty directory or an existing directory created by this harness."
    )


def write_ownership_marker(out_dir: Path, repo: Path, generated_artifacts: list[str]) -> None:
    marker = {
        "owned_by": ARTIFACT_OWNER,
        "repo_root": str(repo),
        "claimed_at": datetime.now(timezone.utc).isoformat(),
        "generated_artifacts": generated_artifacts,
    }
    write_json(out_dir / ARTIFACT_MARKER, marker)


def previous_generated_artifacts(out_dir: Path, marker: dict | None) -> list[str]:
    if marker and isinstance(marker.get("generated_artifacts"), list):
        marker_items = [item for item in marker["generated_artifacts"] if isinstance(item, str)]
        if marker_items:
            return marker_items
    manifest_path = out_dir / "manifest.json"
    if manifest_path.is_file():
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            manifest = {}
        if isinstance(manifest, dict):
            prompts = [
                batch.get("prompt")
                for batch in manifest.get("batches", [])
                if isinstance(batch, dict) and isinstance(batch.get("prompt"), str)
            ]
            journey = manifest.get("journey_audit", {})
            journey_prompts = [
                value
                for value in (journey.get("source_prompt"), journey.get("visual_prompt"))
                if isinstance(value, str)
            ] if isinstance(journey, dict) else []
            lead_reconciliation = manifest.get("lead_reconciliation", {})
            lead_prompt = (
                [lead_reconciliation.get("prompt")]
                if isinstance(lead_reconciliation, dict) and isinstance(lead_reconciliation.get("prompt"), str)
                else []
            )
            archived_reports = []
            archived_reports_dir = manifest.get("archived_reports_dir")
            if isinstance(archived_reports_dir, str):
                rel_archive = relative_dir_if_child(out_dir, Path(archived_reports_dir))
                if rel_archive:
                    archived_reports.append(rel_archive)
            return [
                "audit_complete.json",
                "audit_index.md",
                "effort_ledger.json",
                "excluded_files.json",
                "manifest.json",
                "queue_complete.json",
                *lead_prompt,
                *journey_prompts,
                *archived_reports,
                *prompts,
            ]
    fallback_prompts = [
        path.name
        for path in out_dir.glob("batch_*.md")
        if path.is_file() and re.fullmatch(r"batch_\d{3}\.md", path.name)
    ]
    return [
        "audit_complete.json",
        "audit_index.md",
        "effort_ledger.json",
        "excluded_files.json",
        "manifest.json",
        "queue_complete.json",
        "journey_audit.md",
        "visual_journey_audit.md",
        "lead_reconciliation.md",
        *fallback_prompts,
    ]


def is_safe_generated_artifact_name(name: str) -> bool:
    path = Path(name)
    if not name or path.is_absolute():
        return False
    if path == Path(".") or any(part in {"", ".", ".."} for part in path.parts):
        return False
    return True


def is_relative_to_path(child: Path, parent: Path) -> bool:
    try:
        child.relative_to(parent)
    except ValueError:
        return False
    return True


def has_symlinked_parent(path: Path, root: Path) -> bool:
    try:
        relative = path.relative_to(root)
    except ValueError:
        return True
    current = root
    for part in relative.parts[:-1]:
        current = current / part
        if current.is_symlink():
            return True
    return False


def generated_artifact_path_is_safe(out_dir: Path, path: Path) -> bool:
    out_root = out_dir.resolve()
    try:
        if path.resolve(strict=False) == out_root:
            return False
    except OSError:
        return False
    if has_symlinked_parent(path, out_dir):
        return False
    if path.is_symlink():
        return is_relative_to_path(path.parent.resolve(strict=False), out_root)
    try:
        return is_relative_to_path(path.resolve(strict=False), out_root)
    except OSError:
        return False


def clean_generated_artifacts(out_dir: Path, marker: dict | None) -> None:
    recovery_names = set(previous_generated_artifacts(out_dir, marker)) | KNOWN_GENERATED_ARTIFACTS
    for candidate in out_dir.iterdir():
        if re.fullmatch(r"batch_\d{3,}\.md", candidate.name) or re.fullmatch(
            r"reports\.stale\.\d{8}T\d{6}Z(?:\.\d+)?", candidate.name
        ):
            recovery_names.add(candidate.name)
    for name in sorted(recovery_names):
        if not is_safe_generated_artifact_name(name):
            continue
        path = out_dir / name
        if not generated_artifact_path_is_safe(out_dir, path):
            continue
        if path.is_symlink() or path.is_file():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)


def write_json(path: Path, data: dict | list) -> None:
    path.write_text(json.dumps(data, indent=2), encoding="utf-8")


def canonical_json_sha256(data: dict | list) -> str:
    payload = json.dumps(data, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def write_completion_marker(out_dir: Path, manifest: dict) -> None:
    marker = {
        "run_id": manifest["run_id"],
        "completed_at": datetime.now(timezone.utc).isoformat(),
        "phase": "queue_generated",
        "audit_verified": False,
        "marker_semantics": "Queue artifacts were generated; subagent reports and effort ledger still require verifier completion.",
        "manifest": "manifest.json",
        "audit_index": "audit_index.md",
        "effort_ledger": "effort_ledger.json",
        "excluded_files": "excluded_files.json",
        "reports_dir": "reports",
        "logs_dir": "logs",
        "final_report": "final-report.md",
        "ownership_marker": ARTIFACT_MARKER,
        "batch_count": manifest["batch_count"],
        "source_file_count": manifest["source_file_count"],
    }
    marker_path = out_dir / "queue_complete.json"
    tmp_path = out_dir / "queue_complete.json.tmp"
    write_json(tmp_path, marker)
    tmp_path.replace(marker_path)


def write_effort_ledger(out_dir: Path, manifest: dict) -> None:
    journey = manifest.get("journey_audit", {})
    lead_reconciliation = manifest.get("lead_reconciliation", {})
    journey_required = bool(journey.get("required"))
    pruned_hints = manifest.get("pruned_directory_review_hints", [])
    tracked_deletions = manifest.get("tracked_deletions", [])
    ledger = {
        "run_id": manifest["run_id"],
        "repo_root": manifest["repo_root"],
        "provenance_scope": (
            "Lead-recorded runtime ledger. The verifier checks recorded agent ids, effort values, reports, "
            "journey-worker assignments, and fallback consistency; it cannot independently prove platform scheduler settings."
        ),
        "effort_verification_scope": "ledger-recorded",
        "subagent_capability_check": {
            "status": "pending",
            "spawn_tool": None,
            "can_set_reasoning_effort": None,
            "claim_basis": None,
            "claim_label": None,
            "evidence": None,
            "notes": "Lead agent must record whether subagent spawning and reasoning_effort settings are available before dispatch.",
        },
        "lead": {
            "status": "pending",
            "required_reasoning_effort": "xhigh",
            "actual_reasoning_effort": None,
            "agent_id": None,
            "effort_claim_basis": None,
            "effort_claim_label": None,
            "runtime_provenance": None,
            "notes": None,
        },
        "fallback_mode": {
            "active": False,
            "reason": None,
        },
        "pruned_directory_review": {
            "status": "pending" if manifest.get("pruned_directory_review_hint_count") else "not-applicable",
            "hint_count": manifest.get("pruned_directory_review_hint_count", 0),
            "decisions": [
                {
                    "path": hint.get("path"),
                    "source_like_sample_paths": hint.get("source_like_sample_paths", []),
                    "decision": None,
                    "rationale": None,
                }
                for hint in pruned_hints
                if isinstance(hint, dict)
            ],
            "notes": (
                "Lead must review pruned_directory_review_hints before claiming full coverage."
                if manifest.get("pruned_directory_review_hint_count")
                else "No pruned directories contained source-like samples."
            ),
        },
        "tracked_deletion_review": {
            "status": "pending" if manifest.get("tracked_deletion_count") else "not-applicable",
            "removal_count": manifest.get("tracked_deletion_count", 0),
            "decisions": [
                {
                    "path": removal.get("path"),
                    "baseline_sha256": removal.get("baseline_sha256"),
                    "decision": None,
                    "rationale": None,
                    "evidence": None,
                }
                for removal in tracked_deletions
                if isinstance(removal, dict)
            ],
            "notes": (
                "Lead must review every tracked removal against its baseline and current references before claiming full coverage."
                if manifest.get("tracked_deletion_count")
                else "No tracked files are deleted from the current worktree."
            ),
        },
        "lead_high_risk_review": {
            "status": "pending" if manifest.get("high_risk_file_count") else "not-applicable",
            "files": [
                {
                    **item,
                    "status": "pending",
                    "evidence": None,
                    "notes": None,
                }
                for item in manifest.get("high_risk_files", [])
            ],
        },
        "lead_reconciliation": {
            "status": "pending",
            "prompt": lead_reconciliation.get("prompt"),
            "report": lead_reconciliation.get("report"),
            "notes": "Required lead-owned cross-file semantic implementation reconciliation.",
        },
        "journey_source_worker": {
            "status": "pending" if journey_required else "not-applicable",
            "prompt": journey.get("source_prompt"),
            "required_reasoning_effort": "low" if journey_required else None,
            "agent_id": None,
            "actual_reasoning_effort": None,
            "report": journey.get("source_report"),
            "runtime_provenance": None,
            "effort_claim_basis": None,
            "effort_claim_label": None,
            "notes": (
                "Required when interface-relevant files are queued; inspect source-level user journeys, relevance, "
                "decision information, and test-mode support."
            )
            if journey_required
            else "No interface-relevant files were queued.",
        },
        "visual_journey_worker": {
            "status": "pending" if journey_required else "not-applicable",
            "prompt": journey.get("visual_prompt"),
            "required_reasoning_effort": "low" if journey_required else None,
            "agent_id": None,
            "actual_reasoning_effort": None,
            "report": journey.get("visual_report"),
            "runtime_provenance": None,
            "effort_claim_basis": None,
            "effort_claim_label": None,
            "notes": (
                "Required when interface-relevant files are queued; use available visual tooling in test mode "
                "or report the blocker."
            )
            if journey_required
            else "No interface-relevant files were queued.",
        },
        "batches": [
            {
                "batch_id": batch["id"],
                "prompt": batch["prompt"],
                "required_reasoning_effort": "low",
                "agent_id": None,
                "actual_reasoning_effort": None,
                "status": "pending",
                "report": f"reports/{batch['id']}.md",
                "runtime_provenance": None,
                "effort_claim_basis": None,
                "effort_claim_label": None,
                "notes": None,
            }
            for batch in manifest["batches"]
        ],
    }
    write_json(out_dir / "effort_ledger.json", ledger)


HIGH_RISK_PATH_TOKENS = {
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
}
HIGH_RISK_SOURCE_PATTERNS = (
    (re.compile(r"\b(?:subprocess\.(?:Popen|run|call)|child_process|Runtime\.getRuntime\(\)\.exec|ProcessBuilder)\b"), "process execution"),
    (re.compile(r"\bshell\s*=\s*True\b|\beval\s*\(|\bexec\s*\("), "dynamic or shell execution"),
    (re.compile(r"\b(?:DROP|TRUNCATE|DELETE\s+FROM|pg_restore|pg_dump|pg_dumpall)\b", re.IGNORECASE), "destructive or backup database operation"),
    (re.compile(r"\b(?:password|secret|access[_-]?token|private[_-]?key)\b", re.IGNORECASE), "credential or secret handling"),
)
HIGH_RISK_CODE_SUFFIXES = {"", ".c", ".cc", ".cpp", ".cs", ".go", ".java", ".js", ".jsx", ".kt", ".kts", ".m", ".mm", ".php", ".py", ".rb", ".rs", ".sh", ".swift", ".ts", ".tsx"}

TEST_PATH_PARTS = {"__tests__", "spec", "specs", "test", "tests"}
TEST_NAME_PATTERNS = (
    re.compile(r"\b(?:async\s+)?def\s+(test_[A-Za-z0-9_]+)\s*\("),
    re.compile(r"\bfunc\s+(test[A-Za-z0-9_]+)\s*\("),
    re.compile(r"\b(?:function\s+)?(test[A-Za-z0-9_]+)\s*\("),
    re.compile(r"\b(?:describe|it|test)\s*\(\s*['\"]([^'\"]{1,160})['\"]"),
)


def is_test_source_path(rel_path: str) -> bool:
    path = PurePosixPath(rel_path)
    name = path.name.casefold()
    stem = path.stem.casefold()
    return bool(
        TEST_PATH_PARTS.intersection(part.casefold() for part in path.parts[:-1])
        or name in {"self_test.py", "selftest.py"}
        or stem.startswith(("test_", "spec_"))
        or stem.endswith(("_test", "_spec", ".test", ".spec"))
    )


def build_test_evidence_index(repo: Path, entries: list[FileEntry], run_id: str) -> dict:
    """Create one bounded, deterministic test inventory shared by every batch."""
    records: list[dict] = []
    for entry in entries:
        if not is_test_source_path(entry.rel_path):
            continue
        try:
            text = read_initial_bytes(repo / entry.rel_path, limit=1_000_000).decode(
                "utf-8", errors="ignore"
            )
        except OSError:
            text = ""
        names: list[str] = []
        for pattern in TEST_NAME_PATTERNS:
            for match in pattern.finditer(text):
                name = " ".join(match.group(1).split())
                if name and name not in names:
                    names.append(name)
                if len(names) >= 200:
                    break
            if len(names) >= 200:
                break
        records.append(
            {
                "rel_path": entry.rel_path,
                "sha256": entry.sha256,
                "named_tests": names,
                "named_tests_truncated": len(names) >= 200,
            }
        )
    return {
        "schema_version": 1,
        "run_id": run_id,
        "test_file_count": len(records),
        "test_files": records,
    }


def high_risk_file_inventory(repo: Path, entries: list[FileEntry]) -> list[dict]:
    result: list[dict] = []
    for entry in entries:
        reasons: set[str] = set()
        path = Path(entry.rel_path)
        tokens = set(path.parts[:-1]) | filename_words(path.name)
        matched_tokens = sorted(token for token in tokens if token.lower() in HIGH_RISK_PATH_TOKENS)
        if matched_tokens:
            reasons.add("high-risk path/domain tokens: " + ", ".join(matched_tokens))
        try:
            text = read_initial_bytes(repo / entry.rel_path, limit=600_000).decode("utf-8", errors="ignore")
        except OSError:
            text = ""
        if path.suffix.lower() in HIGH_RISK_CODE_SUFFIXES:
            for pattern, reason in HIGH_RISK_SOURCE_PATTERNS:
                if pattern.search(text):
                    reasons.add(reason)
        if reasons:
            result.append({"rel_path": entry.rel_path, "sha256": entry.sha256, "risk_reasons": sorted(reasons)})
    return result


def relative_dir_if_child(parent: Path, child: Path) -> str | None:
    try:
        rel_path = child.relative_to(parent)
    except ValueError:
        return None
    if rel_path == Path("."):
        return ""
    return rel_path.as_posix()


def discover_owned_output_dirs(repo: Path, include_generated: bool, include_vendor: bool) -> list[str]:
    output_dirs: list[str] = []
    for root, dirs, files in os.walk(repo):
        root_path = Path(root)
        dirs[:] = [
            item
            for item in dirs
            if not is_excluded_dir(
                item,
                include_generated,
                include_vendor,
                (root_path / item).relative_to(repo).as_posix(),
            )
        ]
        if ARTIFACT_MARKER not in files:
            continue
        out_dir = Path(root)
        marker = read_ownership_marker(out_dir)
        if marker and marker.get("owned_by") == ARTIFACT_OWNER and marker.get("repo_root") == str(repo):
            rel_dir = relative_dir_if_child(repo, out_dir)
            if rel_dir:
                output_dirs.append(rel_dir)
            dirs[:] = []
    return sorted(set(output_dirs))


def validate_repo_relative_include(repo: Path, raw_path: str) -> str:
    rel = PurePosixPath(raw_path)
    if rel.is_absolute() or not rel.parts or any(part in {"", ".", ".."} for part in rel.parts):
        raise ValueError(f"--include-file must be a repo-relative path without '.' or '..': {raw_path}")
    validate_repo_relative_path_token(rel.as_posix(), "--include-file")
    path = repo / rel.as_posix()
    if not path.exists() or not path.is_file():
        raise ValueError(f"--include-file does not name an existing file: {raw_path}")
    return rel.as_posix()


def duplicate_whole_file_paths_for_batches(batches: list[list[AuditUnit]]) -> list[str]:
    whole_file_paths = [
        item.rel_path
        for batch in batches
        for item in batch
        if (
            item.start_line is None
            and item.end_line is None
            and item.start_byte is None
            and item.end_byte is None
        )
    ]
    return sorted(path for path, count in Counter(whole_file_paths).items() if count > 1)


def validate_generated_artifact_tokens(entries: list[FileEntry], units: list[AuditUnit]) -> None:
    for index, entry in enumerate(entries):
        validate_repo_relative_path_token(entry.rel_path, f"source_files[{index}].rel_path")
    for index, unit in enumerate(units):
        validate_markdown_safe_token(unit.unit_id, f"coverage_units[{index}].unit_id")
        validate_repo_relative_path_token(unit.rel_path, f"coverage_units[{index}].rel_path")


def write_outputs(
    repo: Path,
    out_dir: Path,
    entries: list[FileEntry],
    excluded: list[dict],
    tracked_deletions: list[dict[str, object]],
    units: list[AuditUnit],
    batches: list[list[AuditUnit]],
    run_id: str,
) -> None:
    validate_generated_artifact_tokens(entries, units)
    verifier_script = COMPANION_SCRIPT_DIR / "verify_audit_results.py"
    if not verifier_script.is_file():
        raise ValueError(
            "full_repo_harness.queue is a shared library, not a standalone audit builder. "
            "Run a skill-local build script such as skills/full-repo-audit/scripts/build_audit_batches.py "
            "so the companion verifier path can be resolved."
        )
    ownership_marker = ensure_output_dir_safe(out_dir, repo)
    out_dir.mkdir(parents=True, exist_ok=True)
    if ownership_marker is None:
        write_ownership_marker(out_dir, repo, [])
        ownership_marker = read_ownership_marker(out_dir)
    clean_generated_artifacts(out_dir, ownership_marker)
    reports_dir = out_dir / "reports"
    if reports_dir.is_symlink():
        raise ValueError(f"Audit reports path must not be a symlink: {reports_dir}")
    archived_reports_dir = None
    archived_reports_name = None
    if reports_dir.exists() and any(reports_dir.iterdir()):
        archive_candidate = out_dir / f"reports.stale.{utc_stamp()}"
        suffix = 1
        while archive_candidate.exists():
            suffix += 1
            archive_candidate = out_dir / f"reports.stale.{utc_stamp()}.{suffix}"
        reports_dir.rename(archive_candidate)
        archived_reports_dir = str(archive_candidate)
        archived_reports_name = archive_candidate.name
    reports_dir.mkdir(exist_ok=True)
    logs_dir = out_dir / "logs"
    logs_dir.mkdir(exist_ok=True)
    test_evidence_index = build_test_evidence_index(repo, entries, run_id)
    write_json(out_dir / "test_evidence_index.json", test_evidence_index)

    batch_records = []
    all_batched_paths: list[str] = []
    all_batched_units: list[str] = []
    total_batches = len(batches)

    for index, batch in enumerate(batches, start=1):
        prompt_name = f"batch_{index:03d}.md"
        prompt_path = out_dir / prompt_name
        report_path = reports_dir / prompt_name
        prompt_path.write_text(
            render_batch_prompt(repo, run_id, index, total_batches, batch, report_path),
            encoding="utf-8",
        )
        paths = sorted({item.rel_path for item in batch})
        unit_ids = [item.unit_id for item in batch]
        all_batched_paths.extend(paths)
        all_batched_units.extend(unit_ids)
        batch_records.append(
            {
                "id": f"batch_{index:03d}",
                "prompt": prompt_name,
                "report": f"reports/{prompt_name}",
                "file_count": len(paths),
                "coverage_unit_count": len(batch),
                "interface_file_count": sum(1 for item in batch if item.interface_relevant),
                "byte_count": sum(item.size_bytes for item in batch),
                "files": paths,
                "coverage_units": unit_ids,
                "purpose": purpose_for(batch),
            }
        )

    source_paths = [item.rel_path for item in entries]
    coverage_unit_ids = [item.unit_id for item in units]
    duplicate_paths = duplicate_whole_file_paths_for_batches(batches)
    duplicate_units = sorted(unit_id for unit_id, count in Counter(all_batched_units).items() if count > 1)
    missing_units = sorted(set(coverage_unit_ids) - set(all_batched_units))
    extra_units = sorted(set(all_batched_units) - set(coverage_unit_ids))
    missing = sorted(set(source_paths) - set(all_batched_paths))
    extra = sorted(set(all_batched_paths) - set(source_paths))
    scope_warnings = [item for item in excluded if item.get("scope_warning")]
    pruned_directory_review_hints = [
        item
        for item in excluded
        if item.get("entry_type") == "directory" and item.get("contains_source_like_samples")
    ]
    interface_entries = [item for item in entries if item.interface_relevant]
    high_risk_files = high_risk_file_inventory(repo, entries)
    journey_required = bool(interface_entries)
    journey_audit = {
        "required": journey_required,
        "interface_files": [item.rel_path for item in interface_entries],
        "source_prompt": "journey_audit.md" if journey_required else None,
        "source_report": "reports/journey_audit.md" if journey_required else None,
        "visual_prompt": "visual_journey_audit.md" if journey_required else None,
        "visual_report": "reports/visual_journey_audit.md" if journey_required else None,
    }
    lead_reconciliation = {
        "required": True,
        "worker": "lead_reconciliation",
        "prompt": "lead_reconciliation.md",
        "report": "reports/lead_reconciliation.md",
    }
    (out_dir / "lead_reconciliation.md").write_text(
        render_lead_reconciliation_prompt(repo, run_id, reports_dir / "lead_reconciliation.md"),
        encoding="utf-8",
    )
    if journey_required:
        (out_dir / "journey_audit.md").write_text(
            render_journey_source_prompt(repo, run_id, interface_entries, reports_dir / "journey_audit.md"),
            encoding="utf-8",
        )
        (out_dir / "visual_journey_audit.md").write_text(
            render_visual_journey_prompt(
                repo,
                run_id,
                interface_entries,
                reports_dir / "visual_journey_audit.md",
            ),
            encoding="utf-8",
        )
        write_json(out_dir / "visual_evidence.json", {"schema_version": 1, "run_id": run_id, "artifacts": []})

    verifier_args = [
        sys.executable,
        str(verifier_script),
        "--manifest",
        str(out_dir / "manifest.json"),
        "--reports",
        str(reports_dir),
        "--receipt-out",
        str(out_dir / VERIFICATION_RECEIPT_NAME),
    ]
    verifier_command = " ".join(shlex.quote(arg) for arg in verifier_args)
    generated_artifacts = [
        "audit_index.md",
        "effort_ledger.json",
        "excluded_files.json",
        "manifest.json",
        "queue_complete.json",
        "test_evidence_index.json",
        "lead_reconciliation.md",
        "final-report.md",
        "logs",
        VERIFICATION_RECEIPT_NAME,
        *(
            ["journey_audit.md", "visual_journey_audit.md", "visual_evidence.json"]
            if journey_required
            else []
        ),
        *([archived_reports_name] if archived_reports_name else []),
        *[batch["prompt"] for batch in batch_records],
    ]

    manifest = {
        "repo_root": str(repo),
        "run_id": run_id,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "reports_dir": str(reports_dir),
        "logs_dir": str(logs_dir),
        "final_report": str(out_dir / "final-report.md"),
        "archived_reports_dir": archived_reports_dir,
        "artifact_marker": str(out_dir / ARTIFACT_MARKER),
        "effort_ledger": str(out_dir / "effort_ledger.json"),
        "test_evidence_index": str(out_dir / "test_evidence_index.json"),
        "test_evidence_file_count": test_evidence_index["test_file_count"],
        "generated_artifacts": generated_artifacts,
        "verifier_command": verifier_command,
        "verifier_args": verifier_args,
        "source_file_count": len(entries),
        "interface_file_count": sum(1 for item in entries if item.interface_relevant),
        "scope_warning_count": len(scope_warnings),
        "pruned_directory_review_hint_count": len(pruned_directory_review_hints),
        "tracked_deletion_count": len(tracked_deletions),
        "high_risk_file_count": len(high_risk_files),
        "excluded_file_count": len(excluded),
        "excluded_files_sha256": canonical_json_sha256(excluded),
        "batch_count": len(batches),
        "source_files": [asdict(item) for item in entries],
        "coverage_unit_count": len(units),
        "coverage_units": [asdict(item) for item in units],
        "batches": batch_records,
        "journey_audit": journey_audit,
        "lead_reconciliation": lead_reconciliation,
        "coverage_invariants": {
            "unique_batched_file_count": len(set(all_batched_paths)),
            "unique_batched_unit_count": len(set(all_batched_units)),
            "missing_from_batches": missing,
            "duplicates_in_batches": duplicate_paths,
            "extra_in_batches": extra,
            "missing_units_from_batches": missing_units,
            "duplicate_units_in_batches": duplicate_units,
            "extra_units_in_batches": extra_units,
            "all_coverage_units_queued_exactly_once": not missing_units and not duplicate_units and not extra_units,
            "all_source_files_queued_exactly_once": not missing and not extra and not missing_units and not duplicate_units and not extra_units,
        },
        "scope_warnings": scope_warnings,
        "pruned_directory_review_hints": pruned_directory_review_hints,
        "tracked_deletions": tracked_deletions,
        "high_risk_files": high_risk_files,
    }
    write_json(out_dir / "manifest.json", manifest)
    write_json(out_dir / "excluded_files.json", excluded)
    (out_dir / "audit_index.md").write_text(render_index(repo, out_dir, manifest), encoding="utf-8")
    write_effort_ledger(out_dir, manifest)
    write_ownership_marker(out_dir, repo, generated_artifacts)
    write_completion_marker(out_dir, manifest)


def render_index(repo: Path, out_dir: Path, manifest: dict) -> str:
    def table_cell(value: object) -> str:
        text = str(value).replace("\n", " ").replace("\r", " ")
        return text.replace("\\", "\\\\").replace("|", "\\|")

    rows = "\n".join(
        f"| {table_cell(batch['id'])} | `{table_cell(batch['prompt'])}` | {batch['file_count']} | {batch.get('coverage_unit_count', batch['file_count'])} | {batch['interface_file_count']} | {batch['byte_count']} | {table_cell(batch['purpose'])} |"
        for batch in manifest["batches"]
    )
    if not rows:
        rows = "| none | none | 0 | 0 | 0 | 0 | No source-like files were queued. |"

    invariant = manifest["coverage_invariants"]["all_source_files_queued_exactly_once"]
    journey = manifest.get("journey_audit", {})
    lead_reconciliation = manifest.get("lead_reconciliation", {})
    if journey.get("required"):
        journey_prompts = (
            f"- Source journey worker prompt: `{journey['source_prompt']}` -> `{journey['source_report']}`\n"
            f"- Visual journey worker prompt: `{journey['visual_prompt']}` -> `{journey['visual_report']}`"
        )
        journey_instruction = (
            "3. Spawn separate low-effort workers for `journey_audit.md` and `visual_journey_audit.md`; "
            "save their reports at the ledger paths before final synthesis."
        )
    else:
        journey_prompts = "- No journey worker prompts were generated because no interface-relevant files were queued."
        journey_instruction = "3. No journey workers are required because no interface-relevant files were queued."
    pruned_hint_count = manifest.get("pruned_directory_review_hint_count", 0)
    if pruned_hint_count:
        pruned_hint_text = (
            f"Pruned directories with source-like samples needing lead review: **{pruned_hint_count}**\n"
            "Inspect `manifest.json` key `pruned_directory_review_hints` and either requeue first-party samples "
            "with the needed include flag/include glob or disclose why those pruned directories stay out of scope."
        )
    else:
        pruned_hint_text = "Pruned directories with source-like samples needing lead review: **0**"
    tracked_deletion_count = manifest.get("tracked_deletion_count", 0)
    if tracked_deletion_count:
        tracked_deletion_text = (
            f"Tracked worktree removals needing lead review: **{tracked_deletion_count}**\n"
            "Inspect `manifest.json` key `tracked_deletions` and record one evidence-backed disposition per removal in `effort_ledger.json`."
        )
    else:
        tracked_deletion_text = "Tracked worktree removals needing lead review: **0**"
    return f"""# Full Repo Audit Queue

Repo root: `{repo}`
Output directory: `{out_dir}`
Generated: `{manifest['generated_at']}`
Run ID: `{manifest['run_id']}`
Queue completion marker: `queue_complete.json`

Source files queued: **{manifest['source_file_count']}**
Interface-relevant files queued: **{manifest['interface_file_count']}**
Excluded high-signal files needing lead review: **{manifest['scope_warning_count']}**
{pruned_hint_text}
{tracked_deletion_text}
Batches: **{manifest['batch_count']}**
All source files queued exactly once: **{str(invariant).lower()}**

## Lead Agent Instructions

1. Run the lead architectural audit with extra-high effort.
2. Spawn one fresh isolated worker per batch prompt, in waves if needed. Use the runtime/user-selected effort and pass the entire prompt plus applicable project-ledger requirements.
{journey_instruction}
4. Each worker writes its complete report to the exact path embedded in its prompt and returns only the bounded `REPORT_SAVED` receipt. Do not request or accept the report body through the subagent response.
5. Confirm `queue_complete.json` exists and its `run_id` matches `manifest.json` before dispatching batches.
6. Fill `effort_ledger.json` with the subagent capability check, lead effort status, per-batch agent/effort status, and journey worker status.
7. Inspect `excluded_files.json`; resolve any `scope_warning: true` exclusions, review any `pruned_directory_review_hints`, and disposition every `tracked_deletions` row before claiming full coverage.
8. Confirm one Markdown report per batch exists under `reports/` using the exact filename `batch_###.md`, then verify artifact-backed subagent coverage:
   `{manifest['verifier_command']}`
9. If the verifier reports missing reports or ledger/report drift after an interrupted run, treat the verifier and manifest as authoritative: rerun the missing batch/journey prompts, save the exact report filenames, update `effort_ledger.json`, and rerun the verifier before final synthesis.
10. Requeue missing or unchecked files before final synthesis.
11. Reconcile implementation rows across batches into a feature/entry-point inventory. Trace every promised feature, API, command, route, job, event, configuration, schema/migration, build/deploy path, and operational lifecycle through registration, core logic, data/integration effects, authorization, failure/recovery, observable outcome, and verification; file coverage alone is not feature-completeness evidence.
12. Complete `{lead_reconciliation.get('prompt', 'lead_reconciliation.md')}` and save the exact verified report `{lead_reconciliation.get('report', 'reports/lead_reconciliation.md')}`. Record every lead-only cross-file gap as an atomic finding there, including marker-free semantic gaps.
13. Validate every candidate finding directly before placing it in the implementation plan or a completion-ledger projection.
14. Include implementation, interface, and journey findings for controls, fields, menu items, routes, visible decision information, messages, intended features, implementation paths, and tests that imply missing or wrong behavior.
15. Redirect verbose command stdout/stderr to `logs/`. Write the complete synthesized audit to `final-report.md`; return to chat only a compact outcome, critical/finding counts, verifier status, and artifact paths.

## Batches

| Batch | Prompt | Files | Units | UI Files | Bytes | Purpose |
| --- | --- | ---: | ---: | ---: | ---: | --- |
{rows}

## Journey Worker Prompts

{journey_prompts}

## Coverage Files

- `manifest.json`: source-file inventory and coverage invariants.
- `{ARTIFACT_MARKER}`: ownership marker that lets reruns clean only harness-owned directories.
- `queue_complete.json`: queue-generation marker written only after queue artifacts are complete; it is not proof that subagent reports were completed or verified.
- `{VERIFICATION_RECEIPT_NAME}`: written only by a passing stable verifier run and required for manifest-bound consolidation.
- `effort_ledger.json`: required lead/subagent capability and effort ledger.
- `excluded_files.json`: skipped files and reasons.
- `test_evidence_index.json`: one bounded test inventory shared across batches; workers inspect named candidates before any targeted fallback search.
- `reports/`: required destination for returned `batch_###.md` subagent reports.
- `logs/`: cold command output; do not paste full command logs into model context.
- `final-report.md`: complete lead synthesis; chat receives only a compact artifact index.
- `lead_reconciliation.md`: required lead prompt; its exact report is `reports/lead_reconciliation.md`.
- `journey_audit.md` and `visual_journey_audit.md`: generated when interface-relevant files exist and tracked in `effort_ledger.json`.
- `batch_###.md`: exact subagent prompts, including range unit ids for oversized files when needed.
"""


def main() -> int:
    args = parse_args()
    repo = Path(args.repo).expanduser().resolve()
    if not repo.exists() or not repo.is_dir():
        print(f"Repo path is not a directory: {repo}", file=sys.stderr)
        return 2

    run_id = args.run_id or uuid.uuid4().hex
    out_dir = (
        Path(args.out).expanduser().resolve()
        if args.out
        else Path(tempfile.gettempdir()) / "full-repo-audit" / (repo.name or "repo") / f"{utc_stamp()}-{run_id[:8]}"
    )
    output_rel_dirs: list[str] = []
    output_rel_dir = relative_dir_if_child(repo, out_dir)
    if output_rel_dir == "":
        print("--out cannot be the repository root; choose a dedicated audit output directory.", file=sys.stderr)
        return 2
    if output_rel_dir is not None:
        output_rel_dirs.append(output_rel_dir)
    for owned_output_dir in discover_owned_output_dirs(repo, args.include_generated, args.include_vendor):
        if owned_output_dir not in output_rel_dirs:
            output_rel_dirs.append(owned_output_dir)

    try:
        include_files = {validate_repo_relative_include(repo, raw_path) for raw_path in args.include_file}
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    entries, excluded, tracked_deletions = collect_files(
        repo,
        args.include_config,
        args.include_env,
        args.include_generated,
        args.include_vendor,
        args.include_assets,
        args.exclude_glob,
        include_files,
        args.include_glob,
        output_rel_dirs,
    )
    units = audit_units_for(repo, entries, args.max_batch_bytes)
    batches = batch_files(units, args.batch_size, args.max_batch_bytes)
    try:
        write_outputs(repo, out_dir, entries, excluded, tracked_deletions, units, batches, run_id)
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    print(f"Wrote {len(batches)} batches covering {len(entries)} source files to {out_dir}")
    print(f"Excluded {len(excluded)} files; see {out_dir / 'excluded_files.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
