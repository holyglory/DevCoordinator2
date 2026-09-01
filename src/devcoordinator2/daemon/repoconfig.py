""".devcoordinator.toml loading and strict validation (docs/repository-config.md).

Unknown keys are rejected everywhere so typos never silently change meaning.
This module validates [test.*]; deploy_config.py validates [deployment.*]
from the same file.
"""

from __future__ import annotations

import hashlib
import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

CONFIG_NAME = ".devcoordinator.toml"
MAX_CONFIG_BYTES = 262144
TEST_NAME_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,31}$")
CHECK_NAME_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,63}$")
TIMEOUT_MIN, TIMEOUT_MAX, TIMEOUT_DEFAULT = 1, 21600, 600
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
class CheckSpec:
    """One check in a finite governed dependency graph."""

    name: str
    command: tuple[str, ...]
    cwd: Path
    env: dict[str, str]
    after: tuple[str, ...]
    requires: tuple[str, ...]
    completion: str
    on_failure: str
    produces: tuple[str, ...]


@dataclass(frozen=True)
class TestSpec:
    name: str
    command: tuple[str, ...] | None
    cwd: Path  # resolved absolute, proven inside the worktree
    timeout_seconds: int
    env: dict[str, str]
    postgres: PostgresSpec | None = None
    checks: tuple[CheckSpec, ...] = ()
    config_digest: str = ""


def load_test_spec(worktree_root: Path, test_name: str | None) -> TestSpec:
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
    if data.get("schema") != 1:
        raise ConfigError("'schema' must be 1")
    tests_raw = data.get("test")
    if not isinstance(tests_raw, dict) or not tests_raw:
        raise ConfigError("a [test.<name>] section is required")

    default = tests_raw.pop("default", None)
    named = dict(tests_raw)
    for name in named:
        if not TEST_NAME_RE.fullmatch(name):
            raise ConfigError(f"invalid test name {name!r}")
        if not isinstance(named[name], dict):
            raise ConfigError(f"[test.{name}] must be a table")
    if not named:
        raise ConfigError("at least one [test.<name>] section is required")

    if test_name is None:
        if default is not None:
            if default not in named:
                raise ConfigError(f"test.default {default!r} is not defined")
            test_name = default
        elif len(named) == 1:
            test_name = next(iter(named))
        else:
            raise ConfigError("multiple tests defined; set test.default or name one")
    if test_name not in named:
        raise ConfigError(f"test {test_name!r} is not defined "
                          f"(available: {sorted(named)})")

    return _validate_test(
        worktree_root, test_name, named[test_name],
        config_digest=hashlib.sha256(raw).hexdigest(),
    )


def _validate_test(worktree_root: Path, name: str, section: dict,
                   *, config_digest: str) -> TestSpec:
    unknown = set(section) - {
        "command", "cwd", "timeout_seconds", "env", "postgres", "check",
    }
    if unknown:
        raise ConfigError(f"[test.{name}] unknown keys: {sorted(unknown)}")

    command = section.get("command")
    checks_raw = section.get("check")
    if command is not None and checks_raw is not None:
        raise ConfigError(
            f"[test.{name}] must declare either command or check, not both")
    if command is None and checks_raw is None:
        raise ConfigError(f"[test.{name}] requires command or at least one check")
    checked_command = None
    if command is not None:
        checked_command = _validate_command(f"[test.{name}].command", command)

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

    checks = ()
    if checks_raw is not None:
        checks = _validate_checks(worktree_root, name, cwd, checks_raw)

    return TestSpec(name=name, command=checked_command, cwd=cwd,
                    timeout_seconds=timeout, env=env, postgres=postgres,
                    checks=checks, config_digest=config_digest)


def _validate_command(label: str, command) -> tuple[str, ...]:
    if isinstance(command, str):
        raise ConfigError(f"{label} must be an argv array, never a shell string")
    if (not isinstance(command, list) or not command
            or not all(isinstance(a, str) and a for a in command)):
        raise ConfigError(f"{label} must be a non-empty array of non-empty strings")
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


def _validate_checks(worktree_root: Path, test_name: str, default_cwd: Path,
                     raw) -> tuple[CheckSpec, ...]:
    if not isinstance(raw, list) or not raw:
        raise ConfigError(f"[test.{test_name}].check must be a non-empty array")
    if len(raw) > 256:
        raise ConfigError(f"[test.{test_name}].check exceeds 256 checks")
    checks: list[CheckSpec] = []
    seen: set[str] = set()
    allowed = {
        "name", "command", "cwd", "env", "after", "requires",
        "completion", "on_failure", "produces",
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
        on_failure = item.get("on_failure", "continue")
        if on_failure not in ("continue", "stop"):
            raise ConfigError(f"{label}.on_failure must be 'continue' or 'stop'")
        produces_raw = _validate_string_list(
            f"{label}.produces", item.get("produces", []))
        if len(produces_raw) > 16:
            raise ConfigError(f"{label}.produces exceeds 16 paths")
        produces = tuple(_validate_artifact_path(
            f"{label}.produces", value) for value in produces_raw)
        checks.append(CheckSpec(
            name=check_name,
            command=_validate_command(f"{label}.command", item.get("command")),
            cwd=cwd,
            env=_validate_env(f"{label}.env", item.get("env", {})),
            after=after,
            requires=requires,
            completion=completion,
            on_failure=on_failure,
            produces=produces,
        ))

    names = {check.name for check in checks}
    for check in checks:
        missing = (set(check.after) | set(check.requires)) - names
        if missing:
            raise ConfigError(
                f"[test.{test_name}.check.{check.name}] unknown dependencies: "
                f"{sorted(missing)}")
        if check.name in check.after or check.name in check.requires:
            raise ConfigError(
                f"[test.{test_name}.check.{check.name}] cannot depend on itself")
    _validate_acyclic(test_name, checks)
    return tuple(checks)


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
