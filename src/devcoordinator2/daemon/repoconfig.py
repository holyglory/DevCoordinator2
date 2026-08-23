""".devcoordinator.toml loading and strict validation (docs/repository-config.md).

Unknown keys are rejected everywhere so typos never silently change meaning.
Only the Phase 1 [test.*] schema exists; [deployment.*] is reserved.
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

CONFIG_NAME = ".devcoordinator.toml"
MAX_CONFIG_BYTES = 262144
TEST_NAME_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,31}$")
TIMEOUT_MIN, TIMEOUT_MAX, TIMEOUT_DEFAULT = 1, 21600, 600
POSTGRES_IMAGE_RE = re.compile(r"postgres:[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
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
class TestSpec:
    name: str
    command: tuple[str, ...]
    cwd: Path  # resolved absolute, proven inside the worktree
    timeout_seconds: int
    env: dict[str, str]
    postgres: PostgresSpec | None = None


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

    unknown = set(data) - {"schema", "test"}
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

    return _validate_test(worktree_root, test_name, named[test_name])


def _validate_test(worktree_root: Path, name: str, section: dict) -> TestSpec:
    unknown = set(section) - {"command", "cwd", "timeout_seconds", "env", "postgres"}
    if unknown:
        raise ConfigError(f"[test.{name}] unknown keys: {sorted(unknown)}")

    command = section.get("command")
    if isinstance(command, str):
        raise ConfigError(f"[test.{name}] command must be an argv array, "
                          "never a shell string")
    if (not isinstance(command, list) or not command
            or not all(isinstance(a, str) and a for a in command)):
        raise ConfigError(f"[test.{name}] command must be a non-empty array "
                          "of non-empty strings")

    cwd_raw = section.get("cwd", ".")
    if not isinstance(cwd_raw, str):
        raise ConfigError(f"[test.{name}] cwd must be a string")
    if Path(cwd_raw).is_absolute():
        raise ConfigError(f"[test.{name}] cwd must be repository-relative")
    root = worktree_root.resolve()
    cwd = (root / cwd_raw).resolve()
    if cwd != root and root not in cwd.parents:
        raise ConfigError(f"[test.{name}] cwd escapes the repository")

    timeout = section.get("timeout_seconds", TIMEOUT_DEFAULT)
    if not isinstance(timeout, int) or isinstance(timeout, bool) or \
            not (TIMEOUT_MIN <= timeout <= TIMEOUT_MAX):
        raise ConfigError(f"[test.{name}] timeout_seconds must be an integer "
                          f"in [{TIMEOUT_MIN}, {TIMEOUT_MAX}]")

    env_raw = section.get("env", {})
    if not isinstance(env_raw, dict):
        raise ConfigError(f"[test.{name}] env must be a table of strings")
    env: dict[str, str] = {}
    for key, value in env_raw.items():
        if not isinstance(value, str):
            raise ConfigError(f"[test.{name}] env.{key} must be a string")
        if _SECRET_KEY_RE.search(key) and value:
            raise ConfigError(
                f"[test.{name}] env.{key} looks like a literal secret; "
                "reference secrets held outside the repository instead")
        env[key] = value

    postgres = None
    if "postgres" in section:
        postgres = _validate_postgres(name, section["postgres"])

    return TestSpec(name=name, command=tuple(command), cwd=cwd,
                    timeout_seconds=timeout, env=env, postgres=postgres)


def _validate_postgres(name: str, section) -> PostgresSpec:
    if not isinstance(section, dict):
        raise ConfigError(f"[test.{name}.postgres] must be a table")
    unknown = set(section) - {"image", "database", "user"}
    if unknown:
        raise ConfigError(f"[test.{name}.postgres] unknown keys: {sorted(unknown)}")
    image = section.get("image", POSTGRES_IMAGE_DEFAULT)
    if not isinstance(image, str) or not POSTGRES_IMAGE_RE.fullmatch(image):
        raise ConfigError(f"[test.{name}.postgres] image must be an official "
                          "'postgres:<tag>' reference")
    database = section.get("database", "test")
    user = section.get("user", "test")
    for key, value in (("database", database), ("user", user)):
        if not isinstance(value, str) or not PG_IDENT_RE.fullmatch(value):
            raise ConfigError(f"[test.{name}.postgres] {key} must match "
                              "[a-z_][a-z0-9_]{0,62}")
    return PostgresSpec(image=image, database=database, user=user)
