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
    "DEVCOORDINATOR2_PORT_RANGE": "20000-29999",
    "DEVCOORDINATOR2_BASE_DOMAIN": "",
    "DEVCOORDINATOR2_EDGE_UID": "",
    "DEVCOORDINATOR2_ADMIN_EMAILS": "",
    "DEVCOORDINATOR2_TELEGRAM_TOKEN_FILE": "",
    "DEVCOORDINATOR2_TELEGRAM_API": "https://api.telegram.org",
    "DEVCOORDINATOR2_BUGS_DIR": "/var/lib/devcoordinator2-bugs",
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
    port_range: tuple[int, int] = (20000, 29999)
    base_domain: str = ""
    edge_uid: int | None = None          # only this peer may assert a public identity
    admin_emails: tuple[str, ...] = ()   # bootstrap administrators (instance data)
    telegram_token_file: Path | None = None
    telegram_api: str = "https://api.telegram.org"
    bugs_dir: Path = Path("/var/lib/devcoordinator2-bugs")

    @property
    def database_path(self) -> Path:
        return self.state_dir / "authority.sqlite3"

    @property
    def deployments_dir(self) -> Path:
        return self.state_dir / "deployments"

    @property
    def secrets_dir(self) -> Path:
        return self.state_dir / "secrets"

    @property
    def routes_path(self) -> Path:
        return self.state_dir / "routes.json"

    @property
    def deploy_unit_prefix(self) -> str:
        return self.unit_prefix.replace("-test", "") + "-deploy"


def load_instance_config() -> InstanceConfig:
    file_values = _instance_file_values()

    def get(key: str) -> str:
        return os.environ.get(key) or file_values.get(key) or _DEFAULTS[key]

    low, _, high = get("DEVCOORDINATOR2_PORT_RANGE").partition("-")
    try:
        port_range = (int(low), int(high))
    except ValueError as exc:
        raise ValueError("DEVCOORDINATOR2_PORT_RANGE must be 'low-high'") from exc
    if not (1024 <= port_range[0] < port_range[1] <= 65535):
        raise ValueError("DEVCOORDINATOR2_PORT_RANGE must lie within 1024-65535")
    return InstanceConfig(
        socket_path=Path(get("DEVCOORDINATOR2_SOCKET")),
        state_dir=Path(get("DEVCOORDINATOR2_STATE_DIR")),
        unit_prefix=get("DEVCOORDINATOR2_UNIT_PREFIX"),
        slice_name=get("DEVCOORDINATOR2_SLICE"),
        client_group=get("DEVCOORDINATOR2_CLIENT_GROUP"),
        port_range=port_range,
        base_domain=get("DEVCOORDINATOR2_BASE_DOMAIN").strip().strip("."),
        edge_uid=int(get("DEVCOORDINATOR2_EDGE_UID")) if get("DEVCOORDINATOR2_EDGE_UID")
        else None,
        admin_emails=tuple(e.strip().lower() for e in
                           get("DEVCOORDINATOR2_ADMIN_EMAILS").split(",") if e.strip()),
        telegram_token_file=Path(get("DEVCOORDINATOR2_TELEGRAM_TOKEN_FILE"))
        if get("DEVCOORDINATOR2_TELEGRAM_TOKEN_FILE") else None,
        telegram_api=get("DEVCOORDINATOR2_TELEGRAM_API").rstrip("/"),
        bugs_dir=Path(get("DEVCOORDINATOR2_BUGS_DIR")),
    )


def test_dir(worktree_root: Path) -> Path:
    """Repository-local current-test directory for a worktree."""
    return worktree_root / ".devcoordinator" / "test" / "current"
