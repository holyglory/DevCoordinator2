"""summary.json: result schema 1 and atomic replacement.

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

RESULT_SCHEMA_VERSION = 1

TERMINAL_STATUSES = frozenset(
    {"passed", "failed", "timed-out", "cancelled", "interrupted", "superseded"}
)
STATUSES = TERMINAL_STATUSES | {"running"}

_FIELDS = (
    "schema_version", "run_id", "test", "status", "started_at", "finished_at",
    "duration_seconds", "exit_code", "stdout_bytes_observed",
    "stdout_bytes_retained", "stderr_bytes_observed", "stderr_bytes_retained",
    "stdout_truncated", "stderr_truncated", "caller_uid", "client",
)


def build(run_id: str, test: str, status: str, started_at: str,
          caller_uid: int, client: str, *, finished_at: str | None = None,
          duration_seconds: float | None = None, exit_code: int | None = None,
          stdout_observed: int = 0, stdout_retained: int = 0,
          stderr_observed: int = 0, stderr_retained: int = 0) -> dict[str, Any]:
    assert status in STATUSES, status
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
        "stdout_bytes_retained": stdout_retained,
        "stderr_bytes_observed": stderr_observed,
        "stderr_bytes_retained": stderr_retained,
        "stdout_truncated": stdout_observed > stdout_retained,
        "stderr_truncated": stderr_observed > stderr_retained,
        "caller_uid": caller_uid,
        "client": client,
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
    return data
