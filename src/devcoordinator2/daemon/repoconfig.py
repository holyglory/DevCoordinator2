""".devcoordinator.toml loading and strict validation (docs/repository-config.md).

Unknown keys are rejected everywhere so typos never silently change meaning.
This module validates [test.*]; deploy_config.py validates [deployment.*]
from the same file.
"""

from __future__ import annotations

import hashlib
import re
import tomllib
from dataclasses import dataclass, replace
from pathlib import Path

CONFIG_NAME = ".devcoordinator.toml"
MAX_CONFIG_BYTES = 262144
TEST_NAME_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,31}$")
CHECK_NAME_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,63}$")
CASE_ID_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
TIMEOUT_MIN, TIMEOUT_MAX, TIMEOUT_DEFAULT = 1, 21600, 600
VALIDATION_TIERS = ("development", "pre-merge", "release")
DIAGNOSTIC_REPORT_FORMATS = ("junit", "playwright-json", "rust-json")
MAX_DIAGNOSTIC_SOURCES = 8
MAX_RETAINED_ARTIFACTS = 8
MAX_RETAINED_ARTIFACT_BYTES = 1024 * 1024 * 1024
MAX_RETAINED_ARTIFACT_TOTAL_BYTES = 2 * 1024 * 1024 * 1024
_TIER_RANK = {tier: index for index, tier in enumerate(VALIDATION_TIERS)}
POSTGRES_IMAGE_RE = re.compile(r"postgres:[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
POSTGRES_DIGEST_IMAGE_RE = re.compile(
    r"[a-z0-9][a-z0-9._/-]{0,200}"
    r"(?::[A-Za-z0-9][A-Za-z0-9._-]{0,127})?"
    r"@sha256:[0-9a-f]{64}$"
)
PG_IDENT_RE = re.compile(r"[a-z_][a-z0-9_]{0,62}$")
POSTGRES_IMAGE_DEFAULT = "postgres:16-alpine"
_SECRET_KEY_RE = re.compile(r"(token|secret|password|passwd|credential|api_?key)",
                            re.IGNORECASE)


class ConfigError(Exception):
    """Validation failure; message is safe to return to callers."""


@dataclass(frozen=True)
class PostgresSpec:
    """Test-scoped ephemeral PostgreSQL: one throwaway instance per run."""
    image: str
    database: str
    user: str


@dataclass(frozen=True)
class CaseSpec:
    """One statically declared case appended to a reviewed case command."""

    id: str
    args: tuple[str, ...]


@dataclass(frozen=True)
class DiagnosticSourceSpec:
    """One declared structured report below a leaf diagnostics directory."""

    format: str
    path: str


@dataclass(frozen=True)
class RetainedArtifactSpec:
    """One required directory snapshot retained with a successful check."""

    name: str
    path: str
    max_bytes: int


@dataclass(frozen=True)
class CheckSpec:
    """One check in a finite governed dependency graph."""

    name: str
    tier: str
    role: str
    command: tuple[str, ...] | None
    discover: tuple[str, ...] | None
    case_command: tuple[str, ...] | None
    cases: tuple[CaseSpec, ...]
    cwd: Path
    env: dict[str, str]
    after: tuple[str, ...]
    requires: tuple[str, ...]
    completion: str
    on_failure: str
    produces: tuple[str, ...]
    diagnostic_sources: tuple[DiagnosticSourceSpec, ...]
    timeout_seconds: int | None
    invalidates: tuple[str, ...]
    retained_artifacts: tuple[RetainedArtifactSpec, ...] = ()


@dataclass(frozen=True)
class TestSpec:
    name: str
    cwd: Path  # resolved absolute, proven inside the worktree
    timeout_seconds: int
    env: dict[str, str]
    postgres: PostgresSpec | None = None
    checks: tuple[CheckSpec, ...] = ()
    config_digest: str = ""


@dataclass(frozen=True)
class TestConfigSummary:
    """Command-free installation preflight result for one named test."""

    name: str
    tiers: tuple[str, ...]
    default: bool


def _load_all_test_specs(worktree_root: Path) -> tuple[str | None, dict[str, TestSpec]]:
    config_path = worktree_root / CONFIG_NAME
    try:
        if config_path.stat().st_size > MAX_CONFIG_BYTES:
            raise ConfigError(f"{CONFIG_NAME} exceeds {MAX_CONFIG_BYTES} bytes")
        raw = config_path.read_bytes()
    except FileNotFoundError:
        raise ConfigError(f"{CONFIG_NAME} not found in {worktree_root}") from None
    except OSError as exc:
        raise ConfigError(f"cannot read {CONFIG_NAME}: {exc}") from exc
    try:
        data = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise ConfigError(f"invalid TOML: {exc}") from exc

    unknown = set(data) - {"schema", "test", "deployment"}
    if unknown:
        raise ConfigError(f"unknown top-level keys: {sorted(unknown)}")
    if data.get("schema") != 2:
        raise ConfigError("'schema' must be 2; schema 1 is no longer supported")
    tests_raw = data.get("test")
    if not isinstance(tests_raw, dict) or not tests_raw:
        raise ConfigError("a [test.<name>] section is required")

    default = tests_raw.get("default")
    named = {name: section for name, section in tests_raw.items() if name != "default"}
    for name in named:
        if not TEST_NAME_RE.fullmatch(name):
            raise ConfigError(f"invalid test name {name!r}")
        if not isinstance(named[name], dict):
            raise ConfigError(f"[test.{name}] must be a table")
    if not named:
        raise ConfigError("at least one [test.<name>] section is required")

    if default is not None and default not in named:
        raise ConfigError(f"test.default {default!r} is not defined")

    digest = hashlib.sha256(raw).hexdigest()
    specs = {
        name: _validate_test(
            worktree_root, name, named[name], config_digest=digest)
        for name in named
    }
    return default, specs


def validate_test_config(worktree_root: Path) -> tuple[TestConfigSummary, ...]:
    """Validate every named test; never return commands or environment values."""
    default, specs = _load_all_test_specs(worktree_root)
    summaries = []
    for name, spec in specs.items():
        tiers = tuple(tier for tier in VALIDATION_TIERS
                      if any(check.tier == tier for check in spec.checks))
        summaries.append(TestConfigSummary(
            name=name, tiers=tiers, default=name == default))
    return tuple(summaries)


def load_test_spec(worktree_root: Path, test_name: str | None) -> TestSpec:
    default, named = _load_all_test_specs(worktree_root)
    if test_name is None:
        if default is not None:
            test_name = default
        elif len(named) == 1:
            test_name = next(iter(named))
        else:
            raise ConfigError("multiple tests defined; set test.default or name one")
    if test_name not in named:
        raise ConfigError(f"test {test_name!r} is not defined "
                          f"(available: {sorted(named)})")

    return named[test_name]


def _validate_test(worktree_root: Path, name: str, section: dict,
                   *, config_digest: str) -> TestSpec:
    legacy = set(section) & {"command", "tier"}
    if legacy:
        raise ConfigError(
            f"[test.{name}] schema 2 rejects direct test commands; "
            "declare one or more [[test.<name>.check]] tables")
    unknown = set(section) - {"cwd", "timeout_seconds", "env", "postgres", "check"}
    if unknown:
        raise ConfigError(f"[test.{name}] unknown keys: {sorted(unknown)}")

    checks_raw = section.get("check")
    if checks_raw is None:
        raise ConfigError(
            f"[test.{name}] requires at least one [[test.{name}.check]] table")

    cwd = _validate_cwd(worktree_root, f"[test.{name}].cwd",
                        section.get("cwd", "."))

    timeout = section.get("timeout_seconds", TIMEOUT_DEFAULT)
    if not isinstance(timeout, int) or isinstance(timeout, bool) or \
            not (TIMEOUT_MIN <= timeout <= TIMEOUT_MAX):
        raise ConfigError(f"[test.{name}] timeout_seconds must be an integer "
                          f"in [{TIMEOUT_MIN}, {TIMEOUT_MAX}]")

    env = _validate_env(f"[test.{name}].env", section.get("env", {}))

    postgres = None
    if "postgres" in section:
        postgres = _validate_postgres(name, section["postgres"])

    checks = _validate_checks(worktree_root, name, cwd, checks_raw)

    return TestSpec(name=name, cwd=cwd, timeout_seconds=timeout, env=env, postgres=postgres,
                    checks=checks, config_digest=config_digest)


def _validate_tier(label: str, value) -> str:
    if value not in VALIDATION_TIERS:
        raise ConfigError(
            f"{label} must be 'development', 'pre-merge', or 'release'")
    return value


def _validate_command(label: str, command) -> tuple[str, ...]:
    if isinstance(command, str):
        raise ConfigError(f"{label} must be an argv array, never a shell string")
    if (not isinstance(command, list) or not command or len(command) > 256
            or not all(isinstance(a, str) and a and len(a.encode("utf-8")) <= 4096
                       for a in command)):
        raise ConfigError(
            f"{label} must contain 1..256 non-empty strings of at most 4096 bytes")
    return tuple(command)


def _validate_cwd(worktree_root: Path, label: str, cwd_raw) -> Path:
    if not isinstance(cwd_raw, str):
        raise ConfigError(f"{label} must be a string")
    if Path(cwd_raw).is_absolute():
        raise ConfigError(f"{label} must be repository-relative")
    root = worktree_root.resolve()
    cwd = (root / cwd_raw).resolve()
    if cwd != root and root not in cwd.parents:
        raise ConfigError(f"{label} escapes the repository")
    return cwd


def _validate_env(label: str, env_raw) -> dict[str, str]:
    if not isinstance(env_raw, dict):
        raise ConfigError(f"{label} must be a table of strings")
    env: dict[str, str] = {}
    for key, value in env_raw.items():
        if not isinstance(key, str) or not key.isidentifier():
            raise ConfigError(f"{label} keys must be environment identifiers")
        if key.startswith("DEVCOORDINATOR_"):
            raise ConfigError(f"{label}.{key} uses a reserved internal prefix")
        if not isinstance(value, str):
            raise ConfigError(f"{label}.{key} must be a string")
        if _SECRET_KEY_RE.search(key) and value:
            raise ConfigError(
                f"{label}.{key} looks like a literal secret; "
                "reference secrets held outside the repository instead")
        env[key] = value
    return env


def _validate_string_list(label: str, value) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise ConfigError(f"{label} must be an array of strings")
    if len(value) != len(set(value)):
        raise ConfigError(f"{label} contains duplicates")
    return tuple(value)


def _validate_artifact_path(label: str, value: str) -> str:
    parsed = Path(value)
    if not value or len(value) > 256 or parsed.is_absolute() \
            or "\\" in value or "\0" in value \
            or any(part in ("", ".", "..") for part in parsed.parts) \
            or parsed.as_posix() != value:
        raise ConfigError(f"{label} must be a normalized repository-relative path")
    return value


def _validate_diagnostic_sources(label: str, value) \
        -> tuple[DiagnosticSourceSpec, ...]:
    if not isinstance(value, list):
        raise ConfigError(f"{label} must be an array of report tables")
    if len(value) > MAX_DIAGNOSTIC_SOURCES:
        raise ConfigError(
            f"{label} exceeds {MAX_DIAGNOSTIC_SOURCES} diagnostic sources")
    sources: list[DiagnosticSourceSpec] = []
    seen: set[tuple[str, str]] = set()
    for index, raw in enumerate(value):
        source_label = f"{label}[{index}]"
        if not isinstance(raw, dict) or set(raw) != {"format", "path"}:
            raise ConfigError(
                f"{source_label} must contain exactly format and path")
        report_format = raw["format"]
        if report_format not in DIAGNOSTIC_REPORT_FORMATS:
            raise ConfigError(
                f"{source_label}.format must be 'junit', "
                "'playwright-json', or 'rust-json'")
        path = raw["path"]
        parsed = Path(path) if isinstance(path, str) else None
        if not isinstance(path, str) or not path or len(path.encode("utf-8")) > 512 \
                or parsed is None or parsed.is_absolute() \
                or "\\" in path or "\0" in path \
                or any(part in ("", ".", "..") for part in parsed.parts) \
                or parsed.as_posix() != path:
            raise ConfigError(
                f"{source_label}.path must be a normalized leaf diagnostics-relative path")
        identity = (report_format, path)
        if identity in seen:
            raise ConfigError(f"{label} contains a duplicate diagnostic source")
        seen.add(identity)
        sources.append(DiagnosticSourceSpec(report_format, path))
    return tuple(sources)


def _validate_retained_artifacts(label: str, value) \
        -> tuple[RetainedArtifactSpec, ...]:
    if not isinstance(value, list):
        raise ConfigError(f"{label} must be an array of retained-artifact tables")
    if len(value) > MAX_RETAINED_ARTIFACTS:
        raise ConfigError(f"{label} exceeds {MAX_RETAINED_ARTIFACTS} entries")
    artifacts: list[RetainedArtifactSpec] = []
    names: set[str] = set()
    paths: list[Path] = []
    total = 0
    for index, raw in enumerate(value):
        item_label = f"{label}[{index}]"
        if not isinstance(raw, dict) or set(raw) != {"name", "path", "max_bytes"}:
            raise ConfigError(
                f"{item_label} must contain exactly name, path, and max_bytes")
        name = raw["name"]
        if not isinstance(name, str) or not CHECK_NAME_RE.fullmatch(name):
            raise ConfigError(f"{item_label}.name is invalid")
        if name in names:
            raise ConfigError(f"{label} repeats retained artifact name {name!r}")
        path = _validate_artifact_path(f"{item_label}.path", raw["path"])
        if any(ord(character) < 32 or ord(character) == 127 for character in path):
            raise ConfigError(f"{item_label}.path contains a control character")
        parsed = Path(path)
        if parsed.parts[0] in (".git", ".devcoordinator"):
            raise ConfigError(f"{item_label}.path is reserved")
        for prior in paths:
            if parsed == prior or parsed.parts[:len(prior.parts)] == prior.parts \
                    or prior.parts[:len(parsed.parts)] == parsed.parts:
                raise ConfigError(f"{label} contains overlapping paths")
        maximum = raw["max_bytes"]
        if not isinstance(maximum, int) or isinstance(maximum, bool) \
                or not 1 <= maximum <= MAX_RETAINED_ARTIFACT_BYTES:
            raise ConfigError(
                f"{item_label}.max_bytes must be an integer in "
                f"[1, {MAX_RETAINED_ARTIFACT_BYTES}]")
        total += maximum
        if total > MAX_RETAINED_ARTIFACT_TOTAL_BYTES:
            raise ConfigError(
                f"{label} declared limits exceed {MAX_RETAINED_ARTIFACT_TOTAL_BYTES} bytes")
        names.add(name)
        paths.append(parsed)
        artifacts.append(RetainedArtifactSpec(name, path, maximum))
    return tuple(artifacts)


def _validate_cases(label: str, value) -> tuple[CaseSpec, ...]:
    if not isinstance(value, list) or not value:
        raise ConfigError(f"{label} must be a non-empty array of case tables")
    if len(value) > 4096:
        raise ConfigError(f"{label} exceeds 4096 cases")
    cases: list[CaseSpec] = []
    seen: set[str] = set()
    for index, raw in enumerate(value):
        case_label = f"{label}[{index}]"
        if not isinstance(raw, dict) or set(raw) != {"id", "args"}:
            raise ConfigError(f"{case_label} must contain exactly id and args")
        case_id = raw.get("id")
        if not isinstance(case_id, str) or not CASE_ID_RE.fullmatch(case_id):
            raise ConfigError(f"{case_label}.id is invalid")
        if case_id in seen:
            raise ConfigError(f"{label} contains duplicate case id {case_id!r}")
        args = raw.get("args")
        if not isinstance(args, list) or len(args) > 64 or not all(
                isinstance(arg, str) and arg and len(arg.encode("utf-8")) <= 4096
                for arg in args):
            raise ConfigError(
                f"{case_label}.args must be at most 64 non-empty bounded strings")
        if sum(len(arg.encode("utf-8")) for arg in args) > 65536:
            raise ConfigError(f"{case_label}.args exceeds 65536 UTF-8 bytes")
        seen.add(case_id)
        cases.append(CaseSpec(case_id, tuple(args)))
    return tuple(cases)


def _validate_checks(worktree_root: Path, test_name: str, default_cwd: Path,
                     raw) -> tuple[CheckSpec, ...]:
    if not isinstance(raw, list) or not raw:
        raise ConfigError(f"[test.{test_name}].check must be a non-empty array")
    if len(raw) > 256:
        raise ConfigError(f"[test.{test_name}].check exceeds 256 checks")
    checks: list[CheckSpec] = []
    seen: set[str] = set()
    allowed = {
        "name", "tier", "role", "command", "discover", "case_command", "cases",
        "cwd", "env", "after", "requires", "completion", "on_failure", "produces",
        "timeout_seconds", "invalidates", "diagnostic_sources",
        "retained_artifacts",
    }
    for index, item in enumerate(raw):
        label = f"[test.{test_name}.check[{index}]]"
        if not isinstance(item, dict):
            raise ConfigError(f"{label} must be a table")
        unknown = set(item) - allowed
        if unknown:
            raise ConfigError(f"{label} unknown keys: {sorted(unknown)}")
        check_name = item.get("name")
        if not isinstance(check_name, str) or not CHECK_NAME_RE.fullmatch(check_name):
            raise ConfigError(f"{label}.name is invalid")
        if check_name in seen:
            raise ConfigError(f"[test.{test_name}] duplicate check {check_name!r}")
        seen.add(check_name)
        tier = _validate_tier(f"{label}.tier", item.get("tier"))
        role = item.get("role", "work")
        if role not in ("work", "preflight"):
            raise ConfigError(f"{label}.role must be 'work' or 'preflight'")
        command = item.get("command")
        discover = item.get("discover")
        case_command = item.get("case_command")
        cases_raw = item.get("cases")
        execution_forms = sum((command is not None,
                               discover is not None,
                               cases_raw is not None))
        if execution_forms != 1:
            raise ConfigError(
                f"{label} requires exactly one of command, discover, or cases")
        checked_command = _validate_command(f"{label}.command", command) \
            if command is not None else None
        checked_discover = _validate_command(f"{label}.discover", discover) \
            if discover is not None else None
        checked_case_command = None
        static_cases: tuple[CaseSpec, ...] = ()
        if discover is not None or cases_raw is not None:
            checked_case_command = _validate_command(
                f"{label}.case_command", case_command)
            if cases_raw is not None:
                static_cases = _validate_cases(f"{label}.cases", cases_raw)
        elif case_command is not None:
            raise ConfigError(
                f"{label}.case_command requires discover or cases")
        cwd = default_cwd if "cwd" not in item else _validate_cwd(
            worktree_root, f"{label}.cwd", item["cwd"])
        after = _validate_string_list(f"{label}.after", item.get("after", []))
        requires = _validate_string_list(
            f"{label}.requires", item.get("requires", []))
        if set(after) & set(requires):
            raise ConfigError(f"{label} repeats a dependency in after and requires")
        completion = item.get("completion", "process")
        if completion not in ("process", "event"):
            raise ConfigError(f"{label}.completion must be 'process' or 'event'")
        if completion == "event" and (role == "preflight" or checked_command is None):
            raise ConfigError(
                f"{label}.completion 'event' is unavailable for preflights or fan-out")
        on_failure = item.get("on_failure", "continue")
        if on_failure not in ("continue", "stop"):
            raise ConfigError(f"{label}.on_failure must be 'continue' or 'stop'")
        produces_raw = _validate_string_list(
            f"{label}.produces", item.get("produces", []))
        if len(produces_raw) > 16:
            raise ConfigError(f"{label}.produces exceeds 16 paths")
        produces = tuple(_validate_artifact_path(
            f"{label}.produces", value) for value in produces_raw)
        retained_artifacts = _validate_retained_artifacts(
            f"{label}.retained_artifacts", item.get("retained_artifacts", []))
        if retained_artifacts and checked_command is None:
            raise ConfigError(
                f"{label}.retained_artifacts is available only for direct checks")
        if retained_artifacts and completion != "process":
            raise ConfigError(
                f"{label}.retained_artifacts requires process completion")
        diagnostic_sources = _validate_diagnostic_sources(
            f"{label}.diagnostic_sources", item.get("diagnostic_sources", []))
        timeout = item.get("timeout_seconds")
        if timeout is not None and (
                not isinstance(timeout, int) or isinstance(timeout, bool)
                or not TIMEOUT_MIN <= timeout <= TIMEOUT_MAX):
            raise ConfigError(
                f"{label}.timeout_seconds must be an integer in "
                f"[{TIMEOUT_MIN}, {TIMEOUT_MAX}]")
        invalidates = _validate_string_list(
            f"{label}.invalidates", item.get("invalidates", []))
        if invalidates and role != "preflight":
            raise ConfigError(f"{label}.invalidates requires role = 'preflight'")
        if role == "preflight" and not invalidates:
            raise ConfigError(f"{label} preflight must invalidate at least one check")
        checks.append(CheckSpec(
            name=check_name,
            tier=tier,
            role=role,
            command=checked_command,
            discover=checked_discover,
            case_command=checked_case_command,
            cases=static_cases,
            cwd=cwd,
            env=_validate_env(f"{label}.env", item.get("env", {})),
            after=after,
            requires=requires,
            completion=completion,
            on_failure=on_failure,
            produces=produces,
            diagnostic_sources=diagnostic_sources,
            timeout_seconds=timeout,
            invalidates=invalidates,
            retained_artifacts=retained_artifacts,
        ))

    names = {check.name for check in checks}
    by_name = {check.name: check for check in checks}
    for check in checks:
        missing = (set(check.after) | set(check.requires) | set(check.invalidates)) - names
        if missing:
            raise ConfigError(
                f"[test.{test_name}.check.{check.name}] unknown dependencies: "
                f"{sorted(missing)}")
        if check.name in check.after or check.name in check.requires:
            raise ConfigError(
                f"[test.{test_name}.check.{check.name}] cannot depend on itself")
        if check.name in check.invalidates:
            raise ConfigError(
                f"[test.{test_name}.check.{check.name}] cannot invalidate itself")
        for dependency in (*check.after, *check.requires):
            if _TIER_RANK[by_name[dependency].tier] > _TIER_RANK[check.tier]:
                raise ConfigError(
                    f"[test.{test_name}.check.{check.name}] tier inversion: "
                    f"dependency {dependency!r} belongs to a higher tier")

    compiled: list[CheckSpec] = list(checks)
    indexes = {check.name: index for index, check in enumerate(compiled)}
    for preflight in checks:
        for target_name in preflight.invalidates:
            target = compiled[indexes[target_name]]
            if _TIER_RANK[preflight.tier] > _TIER_RANK[target.tier]:
                raise ConfigError(
                    f"[test.{test_name}.check.{preflight.name}] tier inversion: "
                    f"cannot invalidate lower-tier check {target_name!r}")
            if preflight.name in target.after or preflight.name in target.requires:
                raise ConfigError(
                    f"[test.{test_name}] duplicate dependency edge from "
                    f"{preflight.name!r} to {target_name!r}")
            compiled[indexes[target_name]] = replace(
                target, requires=(*target.requires, preflight.name))
    _validate_acyclic(test_name, compiled)
    return tuple(compiled)


def _validate_acyclic(test_name: str, checks: list[CheckSpec]) -> None:
    edges = {check.name: set(check.after) | set(check.requires) for check in checks}
    remaining = set(edges)
    while remaining:
        ready = {name for name in remaining if not (edges[name] & remaining)}
        if not ready:
            raise ConfigError(f"[test.{test_name}] check dependencies contain a cycle")
        remaining -= ready


def _validate_postgres(name: str, section) -> PostgresSpec:
    if not isinstance(section, dict):
        raise ConfigError(f"[test.{name}.postgres] must be a table")
    unknown = set(section) - {"image", "database", "user"}
    if unknown:
        raise ConfigError(f"[test.{name}.postgres] unknown keys: {sorted(unknown)}")
    image = section.get("image", POSTGRES_IMAGE_DEFAULT)
    if not isinstance(image, str) or not (
            POSTGRES_IMAGE_RE.fullmatch(image)
            or POSTGRES_DIGEST_IMAGE_RE.fullmatch(image)):
        raise ConfigError(
            f"[test.{name}.postgres] image must be an official 'postgres:<tag>'"
            " reference or a PostgreSQL-compatible image pinned by sha256 digest")
    database = section.get("database", "test")
    user = section.get("user", "test")
    for key, value in (("database", database), ("user", user)):
        if not isinstance(value, str) or not PG_IDENT_RE.fullmatch(value):
            raise ConfigError(f"[test.{name}.postgres] {key} must match "
                              "[a-z_][a-z0-9_]{0,62}")
    return PostgresSpec(image=image, database=database, user=user)
