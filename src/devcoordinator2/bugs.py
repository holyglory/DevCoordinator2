"""Open bug registry: bounded atomic JSON records in a directory, independent
of the daemon and its database so intake works during any outage. Closing
removes the record; there is no closed-history store. CLI, MCP, daemon, and
Console all use this module on the same directory."""

from __future__ import annotations

import json
import os
import re
import secrets
import tempfile
from datetime import UTC, datetime
from pathlib import Path

DEFAULT_DIR = Path("/var/lib/devcoordinator2-bugs")
LIMITS = {"component": 64, "summary": 200, "expected": 2000, "actual": 2000,
          "steps": 4000}
_FORBIDDEN = re.compile(r"(password|secret|token|api[_-]?key|authorization:\s*bearer)",
                        re.IGNORECASE)
_PRIVATE_PATH = re.compile(r"/home/[^/\s]+/|/etc/devcoordinator2/|/var/lib/devcoordinator2/")


class BugError(Exception):
    pass


def bugs_dir() -> Path:
    return Path(os.environ.get("DEVCOORDINATOR2_BUGS_DIR") or DEFAULT_DIR)


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _clean(field: str, value, required: bool = True) -> str:
    if value is None or value == "":
        if required:
            raise BugError(f"'{field}' is required")
        return ""
    if not isinstance(value, str):
        raise BugError(f"'{field}' must be a string")
    text = value.strip()
    if len(text) > LIMITS[field]:
        raise BugError(f"'{field}' exceeds {LIMITS[field]} characters; keep records atomic,"
                       " reference logs by path instead of pasting them")
    if _FORBIDDEN.search(text):
        raise BugError(f"'{field}' looks like it contains a secret")
    if _PRIVATE_PATH.search(text):
        raise BugError(f"'{field}' contains a private host path; describe the location"
                       " abstractly")
    return text


def _validate_correlations(raw) -> dict[str, str]:
    if raw is None:
        return {}
    if not isinstance(raw, dict):
        raise BugError("'correlations' must be an object")
    allowed = {"run_id", "deployment_id", "component", "repository_id", "call_id"}
    out = {}
    for key, value in raw.items():
        if key not in allowed or not isinstance(value, str) or len(value) > 128:
            raise BugError(f"correlation {key!r} invalid (allowed: {sorted(allowed)})")
        out[key] = value
    return out


def _write(path: Path, record: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".bug-")
    try:
        os.write(fd, json.dumps(record, indent=2).encode())
        os.fsync(fd)
        os.fchmod(fd, 0o664)
    finally:
        os.close(fd)
    os.replace(tmp, path)


def report(*, component: str, summary: str, expected: str, actual: str, steps: str,
           correlations: dict | None = None, reporter: str = "",
           directory: Path | None = None) -> dict:
    directory = directory or bugs_dir()
    record = {
        "component": _clean("component", component), "summary": _clean("summary", summary),
        "expected": _clean("expected", expected), "actual": _clean("actual", actual),
        "steps": _clean("steps", steps), "correlations": _validate_correlations(correlations),
    }
    # Recurrence of an open bug (same component + summary) increments the
    # count instead of creating a duplicate record.
    for existing in list_open(directory):
        if existing["component"] == record["component"] \
                and existing["summary"] == record["summary"]:
            existing["occurrences"] += 1
            existing["last_seen_at"] = _now()
            existing["correlations"] = {**existing["correlations"], **record["correlations"]}
            _write(directory / f"{existing['bug_id']}.json", existing)
            return {**existing, "duplicate": True}
    bug_id = "b" + secrets.token_hex(6)
    record.update(bug_id=bug_id, opened_at=_now(), last_seen_at=_now(), occurrences=1,
                  reporter=reporter[:128])
    _write(directory / f"{bug_id}.json", record)
    return {**record, "duplicate": False}


def list_open(directory: Path | None = None) -> list[dict]:
    directory = directory or bugs_dir()
    out = []
    if not directory.is_dir():
        return out
    for path in sorted(directory.glob("b*.json")):
        try:
            data = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(data, dict) and data.get("bug_id") == path.stem:
            out.append(data)
    return out


def close(bug_id: str, directory: Path | None = None) -> dict:
    directory = directory or bugs_dir()
    if not re.fullmatch(r"b[0-9a-f]{12}", bug_id or ""):
        raise BugError("invalid bug id")
    path = directory / f"{bug_id}.json"
    try:
        data = json.loads(path.read_text())
        path.unlink()
    except FileNotFoundError:
        raise BugError(f"no open bug {bug_id}") from None
    return {"bug_id": bug_id, "closed": True, "component": data.get("component"),
            "summary": data.get("summary")}
