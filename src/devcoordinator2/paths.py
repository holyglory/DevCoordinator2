"""Canonical paths and instance configuration.

Precedence: process environment > instance env file > defaults.
Instance file locations (first match wins): $DEVCOORDINATOR2_INSTANCE_ENV,
./.env in the current working directory's repository (development),
/etc/devcoordinator2/instance.env (installed). See
docs/instance-configuration.md.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

_DEFAULTS = {
    "DEVCOORDINATOR2_SOCKET": "/run/devcoordinator2/daemon.sock",
    "DEVCOORDINATOR2_STATE_DIR": "/var/lib/devcoordinator2",
    "DEVCOORDINATOR2_UNIT_PREFIX": "devcoordinator2-test",
    "DEVCOORDINATOR2_SLICE": "devcoordinator2-tests.slice",
    "DEVCOORDINATOR2_CLIENT_GROUP": "devcoordinator2-clients",
}

INSTALLED_ENV_PATH = Path("/etc/devcoordinator2/instance.env")


def _parse_env_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return values
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
            value = value[1:-1]
        if key.startswith("DEVCOORDINATOR2_"):
            values[key] = value
    return values


def _instance_file_values() -> dict[str, str]:
    explicit = os.environ.get("DEVCOORDINATOR2_INSTANCE_ENV")
    candidates = [Path(explicit)] if explicit else [Path.cwd() / ".env", INSTALLED_ENV_PATH]
    for candidate in candidates:
        if candidate.is_file():
            return _parse_env_file(candidate)
    return {}


@dataclass(frozen=True)
class InstanceConfig:
    socket_path: Path
    state_dir: Path
    unit_prefix: str
    slice_name: str
    client_group: str

    @property
    def database_path(self) -> Path:
        return self.state_dir / "authority.sqlite3"


def load_instance_config() -> InstanceConfig:
    file_values = _instance_file_values()

    def get(key: str) -> str:
        return os.environ.get(key) or file_values.get(key) or _DEFAULTS[key]

    return InstanceConfig(
        socket_path=Path(get("DEVCOORDINATOR2_SOCKET")),
        state_dir=Path(get("DEVCOORDINATOR2_STATE_DIR")),
        unit_prefix=get("DEVCOORDINATOR2_UNIT_PREFIX"),
        slice_name=get("DEVCOORDINATOR2_SLICE"),
        client_group=get("DEVCOORDINATOR2_CLIENT_GROUP"),
    )


def test_dir(worktree_root: Path) -> Path:
    """Repository-local current-test directory for a worktree."""
    return worktree_root / ".devcoordinator" / "test" / "current"
