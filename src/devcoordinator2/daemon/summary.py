"""summary.json: strict result schema 2 and atomic replacement.

The summary is the single authoritative record of a test run. Every write is
a complete document replaced atomically (temp file + fsync + os.replace), so
readers always see either the previous or the new complete state.
"""

from __future__ import annotations

import json
import os
import tempfile
import threading
from pathlib import Path
from typing import Any

RESULT_SCHEMA_VERSION = 2

TERMINAL_STATUSES = frozenset(
    {"passed", "failed", "timed-out", "cancelled", "interrupted", "superseded"}
)
STATUSES = TERMINAL_STATUSES | {"running"}

_FIELDS = (
    "schema_version", "run_id", "test", "status", "started_at", "finished_at",
    "duration_seconds", "exit_code", "stdout_bytes_observed",
    "stderr_bytes_observed", "caller_uid", "client",
    "proof", "selection", "origin_run_id", "requested_tier", "readiness_eligible",
)


def build(run_id: str, test: str, status: str, started_at: str,
          caller_uid: int, client: str, *, finished_at: str | None = None,
          duration_seconds: float | None = None, exit_code: int | None = None,
          stdout_observed: int = 0, stderr_observed: int = 0,
          proof: str = "complete", selection: tuple[str, ...] = (),
          origin_run_id: str | None = None,
          requested_tier: str = "release") -> dict[str, Any]:
    assert status in STATUSES, status
    assert proof in ("complete", "selected", "retry"), proof
    assert requested_tier in ("development", "pre-merge", "release"), requested_tier
    return {
        "schema_version": RESULT_SCHEMA_VERSION,
        "run_id": run_id,
        "test": test,
        "status": status,
        "started_at": started_at,
        "finished_at": finished_at,
        "duration_seconds": duration_seconds,
        "exit_code": exit_code,
        "stdout_bytes_observed": stdout_observed,
        "stderr_bytes_observed": stderr_observed,
        "caller_uid": caller_uid,
        "client": client,
        "proof": proof,
        "selection": list(selection),
        "origin_run_id": origin_run_id,
        "requested_tier": requested_tier,
        "readiness_eligible": proof == "complete" and requested_tier == "release",
    }


def write_atomic(path: Path, summary: dict[str, Any],
                 owner: tuple[int, int] | None = None) -> None:
    missing = [f for f in _FIELDS if f not in summary]
    assert not missing, f"summary missing fields: {missing}"
    payload = json.dumps(summary, indent=2).encode("utf-8")
    fd, tmp_name = tempfile.mkstemp(dir=path.parent, prefix=".summary-")
    try:
        os.write(fd, payload)
        os.fsync(fd)
        os.fchmod(fd, 0o644)
        if owner is not None:
            os.fchown(fd, owner[0], owner[1])
    finally:
        os.close(fd)
    os.replace(tmp_name, path)
    dir_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)


def write_atomic_at(dir_fd: int, summary: dict[str, Any],
                    owner: tuple[int, int] | None = None) -> None:
    """Atomically replace summary.json inside an already-open directory fd.

    Binding the write to the run's own directory (not a path) means a write
    that races a supersession lands in the unlinked old directory and fails
    with ENOENT instead of clobbering the successor run's summary.
    """
    missing = [f for f in _FIELDS if f not in summary]
    assert not missing, f"summary missing fields: {missing}"
    payload = json.dumps(summary, indent=2).encode("utf-8")
    tmp_name = f".summary-{os.getpid()}-{threading.get_ident()}"
    fd = os.open(tmp_name, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644,
                 dir_fd=dir_fd)
    try:
        os.write(fd, payload)
        os.fsync(fd)
        os.fchmod(fd, 0o644)
        if owner is not None:
            os.fchown(fd, owner[0], owner[1])
    finally:
        os.close(fd)
    os.rename(tmp_name, "summary.json", src_dir_fd=dir_fd, dst_dir_fd=dir_fd)
    os.fsync(dir_fd)


def read(path: Path) -> dict[str, Any] | None:
    """Return a complete valid summary or None (missing/partial/corrupt)."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeDecodeError):
        return None
    if not isinstance(data, dict):
        return None
    if data.get("schema_version") != RESULT_SCHEMA_VERSION:
        return None
    if any(f not in data for f in _FIELDS):
        return None
    if data.get("status") not in STATUSES:
        return None
    if data.get("proof") not in ("complete", "selected", "retry") \
            or data.get("requested_tier") not in (
                "development", "pre-merge", "release") \
            or not isinstance(data.get("selection"), list) \
            or not all(isinstance(name, str) for name in data["selection"]) \
            or not isinstance(data.get("readiness_eligible"), bool) \
            or data["readiness_eligible"] != (
                data["proof"] == "complete" and data["requested_tier"] == "release"):
        return None
    origin = data.get("origin_run_id")
    if (data["proof"] == "complete" and (data["selection"] or origin is not None)) \
            or (data["proof"] == "selected"
                and (not data["selection"] or origin is not None)) \
            or (data["proof"] == "retry"
                and (len(data["selection"]) != 1
                     or not isinstance(origin, str) or not origin)):
        return None
    return data
