"""Canonical paths and instance configuration.

Precedence: process environment > instance env file > defaults.
Instance file locations (first match wins): $DEVCOORDINATOR2_INSTANCE_ENV,
./.env in the current working directory's repository (development),
/etc/devcoordinator2/instance.env (installed). See
docs/instance-configuration.md.
"""

from __future__ import annotations

import json
import os
import re
import stat
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
    "DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE": "",
}

INSTALLED_ENV_PATH = Path("/etc/devcoordinator2/instance.env")
_REPOSITORY_ID_RE = re.compile(r"r[0-9a-f]{16}$")
_MAX_ALLOWLIST_BYTES = 65536


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


def _compose_env_authorizations(path: Path | None) -> frozenset[tuple[str, str]]:
    if path is None:
        return frozenset()
    if not path.is_absolute():
        raise ValueError("DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE must be absolute")
    try:
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
            raise ValueError("Compose environment allowlist must be a regular non-symlink file")
        if info.st_size > _MAX_ALLOWLIST_BYTES:
            raise ValueError("Compose environment allowlist exceeds 65536 bytes")
        if info.st_mode & 0o022:
            raise ValueError("Compose environment allowlist must not be group/world writable")
        if info.st_uid not in (0, os.geteuid()):
            raise ValueError("Compose environment allowlist has an unexpected owner")
        raw = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ValueError(f"Compose environment allowlist not found: {path}") from exc
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read Compose environment allowlist: {exc}") from exc
    if not isinstance(raw, dict) or set(raw) != {"schema", "authorizations"} \
            or raw.get("schema") != 1 or not isinstance(raw.get("authorizations"), list):
        raise ValueError("Compose environment allowlist must be schema 1 authorizations")
    entries = raw["authorizations"]
    if len(entries) > 256:
        raise ValueError("Compose environment allowlist has more than 256 entries")
    result = set()
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {"repository_id", "path"}:
            raise ValueError("Compose environment authorization has unknown or missing keys")
        repository_id, relative = entry["repository_id"], entry["path"]
        if not isinstance(repository_id, str) or not _REPOSITORY_ID_RE.fullmatch(
                repository_id):
            raise ValueError("Compose environment authorization repository_id is invalid")
        if not isinstance(relative, str) or not relative or len(relative) > 512 \
                or "\\" in relative or "\0" in relative:
            raise ValueError("Compose environment authorization path is invalid")
        parsed = Path(relative)
        if parsed.is_absolute() or any(part in ("", ".", "..") for part in parsed.parts) \
                or parsed.as_posix() != relative:
            raise ValueError(
                "Compose environment authorization path must be normalized relative")
        pair = (repository_id, relative)
        if pair in result:
            raise ValueError("Compose environment allowlist contains a duplicate entry")
        result.add(pair)
    return frozenset(result)


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
    compose_env_allowlist_file: Path | None = None
    compose_env_authorizations: frozenset[tuple[str, str]] = frozenset()

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
        """World-readable publication directory; the state dir itself stays
        closed (0751) so only this document is exposed to the edge."""
        return self.state_dir / "public" / "routes.json"

    @property
    def deploy_unit_prefix(self) -> str:
        return self.unit_prefix.replace("-test", "") + "-deploy"

    def compose_env_authorized(self, repository_id: str, relative_path: str) -> bool:
        return (repository_id, relative_path) in self.compose_env_authorizations


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
    allowlist_value = get("DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE")
    allowlist_path = Path(allowlist_value) if allowlist_value else None
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
        compose_env_allowlist_file=allowlist_path,
        compose_env_authorizations=_compose_env_authorizations(allowlist_path),
    )


def test_dir(worktree_root: Path) -> Path:
    """Repository-local current-test directory for a worktree."""
    return worktree_root / ".devcoordinator" / "test" / "current"
