"""Thin authority and process bridge for Rust-owned governed-test logs.

The Rust log store owns capture, indexing, diagnostics, queries, and pruning.
This module deliberately knows only how to validate the public selector,
resolve its registered repository, persist the host retention policy, and
exchange bounded schema-2 JSON with the release executor.
"""

from __future__ import annotations

import json
import os
import re
import signal
import stat
import subprocess
import threading
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from devcoordinator2 import ids
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.gitinfo import GitResolveError, resolve_worktree
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.summary import read as read_summary
from devcoordinator2.paths import test_dir
from devcoordinator2.protocol import ProtocolError

DEFAULT_MAX_AGE_SECONDS = 86_400
DEFAULT_CASE_DEPTH = 3
MAX_AGE_SECONDS = 315_360_000
MAX_CASE_DEPTH = 65_535
MAX_BRIDGE_BYTES = 65_536
MAX_CURSOR_BYTES = 4_096
MAX_SEARCH_BYTES = 4_096
MAX_CATALOG_LIMIT = 100
MAX_MATCHES = 100
MAX_CONTEXT_LINES = 100
MAX_CONTENT_BYTES = 48 * 1024
MAX_COORDINATE = (1 << 63) - 1

_EXECUTOR_BINARY = (
    Path(__file__).resolve().parents[3] / "target" / "release"
    / "devcoordinator2-executor"
)
_RUN_ID_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_CHECK_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,63}$")
_CASE_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_PHASES = frozenset({"executor", "check", "discovery", "case"})
_STREAMS = frozenset({"stdout", "stderr"})
_QUERY_OPERATIONS = frozenset({
    "catalog", "tail", "search", "range", "failure_context",
})
_BRIDGE_ERROR_MESSAGES = {
    "args_invalid": "The log request is invalid.",
    "test_log_unavailable": "Governed test logs are unavailable.",
    "log_not_found": "The selected governed test log does not exist.",
    "log_expired": "The selected governed test log has expired.",
    "cursor_stale": "The log cursor no longer identifies this stream snapshot.",
    "structured_evidence_invalid": "Structured test evidence is invalid.",
}


@dataclass(frozen=True)
class _BridgeResult:
    returncode: int
    stdout: bytes
    stderr_oversized: bool


def _run_bridge(argv: list[str], payload: bytes) -> _BridgeResult:
    """Drain both child pipes fully while retaining only one bounded reply."""
    process = subprocess.Popen(
        argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        start_new_session=True, env={"PATH": "/usr/bin:/bin"},
    )
    if process.stdin is None or process.stdout is None or process.stderr is None:
        process.kill()
        raise OSError("Rust log bridge pipes are unavailable")
    stdout = bytearray()
    stderr = bytearray()
    drain_failed = threading.Event()

    def drain(pipe, target: bytearray) -> None:
        try:
            while True:
                block = pipe.read(65_536)
                if not block:
                    break
                remaining = MAX_BRIDGE_BYTES + 1 - len(target)
                if remaining > 0:
                    target.extend(block[:remaining])
        except OSError:
            drain_failed.set()
        finally:
            pipe.close()

    threads = [
        threading.Thread(target=drain, args=(process.stdout, stdout), daemon=True),
        threading.Thread(target=drain, args=(process.stderr, stderr), daemon=True),
    ]
    for thread in threads:
        thread.start()
    try:
        try:
            process.stdin.write(payload)
            process.stdin.close()
        except (BrokenPipeError, OSError):
            try:
                process.stdin.close()
            except OSError:
                pass
        try:
            returncode = process.wait(timeout=60)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=2)
            raise
    finally:
        for thread in threads:
            thread.join(timeout=5)
    if any(thread.is_alive() for thread in threads) or drain_failed.is_set():
        raise OSError("Rust log bridge output drain failed")
    return _BridgeResult(
        returncode=returncode,
        stdout=bytes(stdout),
        stderr_oversized=len(stderr) > MAX_BRIDGE_BYTES,
    )


def _now_iso() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _positive_int(value: Any, label: str, maximum: int) -> int:
    if not isinstance(value, int) or isinstance(value, bool) \
            or not 1 <= value <= maximum:
        raise ProtocolError(
            "args_invalid", f"'{label}' must be an integer in 1..{maximum}")
    return value


def _nonnegative_int(value: Any, label: str, maximum: int) -> int:
    if not isinstance(value, int) or isinstance(value, bool) \
            or not 0 <= value <= maximum:
        raise ProtocolError(
            "args_invalid", f"'{label}' must be an integer in 0..{maximum}")
    return value


def _optional_text(value: Any, label: str, maximum: int) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > maximum:
        raise ProtocolError(
            "args_invalid", f"'{label}' must be a non-empty string of at most {maximum} bytes")
    return value


def validate_log_request(operation: str, args: dict[str, Any]) -> dict[str, Any]:
    """Return the strict Rust request body without the daemon-only path."""
    if operation not in _QUERY_OPERATIONS:
        raise ProtocolError("args_invalid", "unknown test log operation")

    common = {"run_id", "check", "phase", "case", "stream", "cursor"}
    extras = {
        "catalog": {"limit"},
        "tail": {"lines", "max_bytes"},
        "search": {"text", "max_matches", "context_lines", "max_bytes"},
        "range": {
            "line_start", "line_end", "byte_start", "byte_end", "max_bytes",
        },
        "failure_context": {"limit", "context_lines", "max_bytes"},
    }[operation]
    unknown = set(args) - common - extras
    if unknown:
        raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")

    run_id = _optional_text(args.get("run_id"), "run_id", 128)
    if run_id is not None and not _RUN_ID_RE.fullmatch(run_id):
        raise ProtocolError("args_invalid", "'run_id' is invalid")
    check = _optional_text(args.get("check"), "check", 64)
    if check is not None and not _CHECK_RE.fullmatch(check):
        raise ProtocolError("args_invalid", "'check' is invalid")
    case = _optional_text(args.get("case"), "case", 128)
    if case is not None and not _CASE_RE.fullmatch(case):
        raise ProtocolError("args_invalid", "'case' is invalid")
    phase = args.get("phase")
    if phase is not None and phase not in _PHASES:
        raise ProtocolError(
            "args_invalid", "'phase' must be executor, check, discovery, or case")
    stream = args.get("stream")
    if stream is not None and stream not in _STREAMS:
        raise ProtocolError("args_invalid", "'stream' must be stdout or stderr")
    cursor = _optional_text(args.get("cursor"), "cursor", MAX_CURSOR_BYTES)

    if phase == "executor" and (check is not None or case is not None):
        raise ProtocolError("args_invalid", "executor logs have no check or case")
    if phase in ("check", "discovery") and (check is None or case is not None):
        raise ProtocolError(
            "args_invalid", f"phase {phase} requires check and forbids case")
    if phase == "case" and check is None:
        raise ProtocolError("args_invalid", "phase case requires check")
    if phase == "case" and case is None and operation != "catalog":
        raise ProtocolError(
            "args_invalid", f"test log {operation} requires one exact case")
    if case is not None and phase != "case":
        raise ProtocolError("args_invalid", "'case' requires phase case")
    if operation in ("tail", "search", "range", "failure_context"):
        if phase is None or stream is None:
            raise ProtocolError(
                "args_invalid", f"test log {operation} requires phase and stream")

    selector = {
        key: value for key, value in {
            "run_id": run_id,
            "check": check,
            "phase": phase,
            "case": case,
            "stream": stream,
        }.items() if value is not None
    }
    options: dict[str, Any] = {}
    if cursor is not None:
        options["cursor"] = cursor

    if operation == "catalog":
        options["limit"] = _positive_int(
            args.get("limit", MAX_CATALOG_LIMIT), "limit", MAX_CATALOG_LIMIT)
    elif operation == "tail":
        options["lines"] = _positive_int(args.get("lines", 50), "lines", 5_000)
        options["max_bytes"] = _positive_int(
            args.get("max_bytes", 32_768), "max_bytes", MAX_CONTENT_BYTES)
    elif operation == "search":
        text = _optional_text(args.get("text"), "text", MAX_SEARCH_BYTES)
        if text is None:
            raise ProtocolError("args_invalid", "'text' is required")
        options.update({
            "text": text,
            "max_matches": _positive_int(
                args.get("max_matches", 20), "max_matches", MAX_MATCHES),
            "context_lines": _nonnegative_int(
                args.get("context_lines", 2), "context_lines", MAX_CONTEXT_LINES),
            "max_bytes": _positive_int(
                args.get("max_bytes", 32_768), "max_bytes", MAX_CONTENT_BYTES),
        })
    elif operation == "range":
        line_values = (args.get("line_start"), args.get("line_end"))
        byte_values = (args.get("byte_start"), args.get("byte_end"))
        has_lines = any(value is not None for value in line_values)
        has_bytes = any(value is not None for value in byte_values)
        incomplete_lines = has_lines and any(value is None for value in line_values)
        incomplete_bytes = has_bytes and any(value is None for value in byte_values)
        if has_lines == has_bytes or incomplete_lines or incomplete_bytes:
            raise ProtocolError(
                "args_invalid", "range requires exactly one complete line or byte interval")
        if has_lines:
            start = _positive_int(line_values[0], "line_start", MAX_COORDINATE)
            end = _positive_int(line_values[1], "line_end", MAX_COORDINATE)
            if end < start:
                raise ProtocolError("args_invalid", "line_end must not precede line_start")
            options.update(line_start=start, line_end=end)
        else:
            start = byte_values[0]
            end = byte_values[1]
            valid_start = isinstance(start, int) and not isinstance(start, bool) \
                and 0 <= start <= MAX_COORDINATE
            valid_end = isinstance(end, int) and not isinstance(end, bool) \
                and 0 <= end <= MAX_COORDINATE
            if not valid_start or not valid_end or end < start:
                raise ProtocolError(
                    "args_invalid", "byte range must be a zero-based increasing interval")
            options.update(byte_start=start, byte_end=end)
        options["max_bytes"] = _positive_int(
            args.get("max_bytes", MAX_CONTENT_BYTES), "max_bytes", MAX_CONTENT_BYTES)
    else:
        options.update({
            "limit": _positive_int(args.get("limit", 20), "limit", MAX_MATCHES),
            "context_lines": _nonnegative_int(
                args.get("context_lines", 2), "context_lines", MAX_CONTEXT_LINES),
            "max_bytes": _positive_int(
                args.get("max_bytes", 32_768), "max_bytes", MAX_CONTENT_BYTES),
        })
    return {"schema": 2, "operation": operation,
            "selector": selector, "options": options}


class TestLogService:
    def __init__(self, db: Database, registry: Registry,
                 executor_binary: Path = _EXECUTOR_BINARY, bridge_runner=None):
        self._db = db
        self._registry = registry
        self._executor_binary = executor_binary
        self._bridge_runner = bridge_runner or _run_bridge
        self._wake = threading.Event()
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def retention(self) -> dict[str, Any]:
        row = self._db.query("SELECT * FROM test_log_retention_state WHERE singleton=1")[0]
        return {
            "max_age_seconds": row["max_age_seconds"],
            "case_depth": row["case_depth"],
            "defaults": {
                "max_age_seconds": DEFAULT_MAX_AGE_SECONDS,
                "case_depth": DEFAULT_CASE_DEPTH,
            },
            "updated_at": row["updated_at"],
            "updated_by": row["updated_by"],
            "last_cleanup_at": row["last_cleanup_at"],
            "last_cleanup_error_code": row["last_cleanup_error_code"],
        }

    def set_retention(self, max_age_seconds: Any, case_depth: Any,
                      actor: str) -> dict[str, Any]:
        age = _positive_int(max_age_seconds, "max_age_seconds", MAX_AGE_SECONDS)
        depth = _positive_int(case_depth, "case_depth", MAX_CASE_DEPTH)
        before = self.retention()
        if before["max_age_seconds"] != age or before["case_depth"] != depth:
            now = _now_iso()
            with self._db.transaction() as conn:
                conn.execute(
                    "UPDATE test_log_retention_state SET max_age_seconds=?, case_depth=?,"
                    " updated_at=?, updated_by=? WHERE singleton=1",
                    (age, depth, now, actor),
                )
                conn.execute(
                    "INSERT INTO test_log_retention_events(at,actor,"
                    " previous_max_age_seconds,max_age_seconds,previous_case_depth,case_depth)"
                    " VALUES(?,?,?,?,?,?)",
                    (now, actor, before["max_age_seconds"], age,
                     before["case_depth"], depth),
                )
        self.request_cleanup()
        return {**self.retention(), "cleanup_requested": True}

    def query(self, operation: str, path: Path, args: dict[str, Any], caller) -> dict:
        worktree, repository_id = self._resolve(path, caller)
        request = validate_log_request(operation, args)
        request["repository_id"] = repository_id
        if operation == "catalog":
            retention = self.retention()
            request["options"].update(
                max_age_seconds=retention["max_age_seconds"],
                case_depth=retention["case_depth"],
            )
        return self._invoke("log-query", worktree, request)

    def start(self) -> None:
        if self._thread is not None:
            return
        self._thread = threading.Thread(
            target=self._maintenance_loop, daemon=True, name="test-log-retention")
        self._thread.start()
        self.request_cleanup()

    def shutdown(self) -> None:
        self._stop.set()
        self._wake.set()
        if self._thread is not None:
            self._thread.join(timeout=5)

    def request_cleanup(self) -> None:
        self._wake.set()

    def notify_run_finished(self, _worktree: Path, _run_id: str) -> None:
        self.request_cleanup()

    def run_maintenance_once(self) -> dict[str, Any]:
        settings = self.retention()
        removed = 0
        retained_active = 0
        next_expiry_at: str | None = None
        errors: list[dict[str, str]] = []
        error_count = 0
        skipped_missing_worktrees = 0
        for row in self._db.query(
                "SELECT worktree_path,repository_id FROM worktrees ORDER BY worktree_id"):
            worktree = Path(row["worktree_path"])
            try:
                details = worktree.lstat()
            except FileNotFoundError:
                skipped_missing_worktrees += 1
                continue
            except OSError:
                details = None
            if details is None or stat.S_ISLNK(details.st_mode) \
                    or not stat.S_ISDIR(details.st_mode):
                error_count += 1
                if len(errors) < 64:
                    errors.append({
                        "repository_id": row["repository_id"],
                        "code": "test_log_unavailable",
                    })
                continue
            active_run_id = None
            current = read_summary(test_dir(worktree) / "summary.json")
            if current is not None and current.get("status") == "running":
                active_run_id = current.get("run_id")
            request = {
                "schema": 2,
                "repository_id": row["repository_id"],
                "max_age_seconds": settings["max_age_seconds"],
                "case_depth": settings["case_depth"],
                "active_run_id": active_run_id,
            }
            try:
                result = self._invoke("log-prune", worktree, request)
                removed += int(result.get("removed_leaf_folders", 0))
                retained_active += int(result.get("retained_active", 0))
                candidate = result.get("next_expiry_at")
                if isinstance(candidate, str) and (
                        next_expiry_at is None or candidate < next_expiry_at):
                    next_expiry_at = candidate
            except ProtocolError as exc:
                error_count += 1
                if len(errors) < 64:
                    errors.append({"repository_id": row["repository_id"], "code": exc.code})
        now = _now_iso()
        error_code = errors[0]["code"] if errors else None
        with self._db.transaction() as conn:
            conn.execute(
                "UPDATE test_log_retention_state SET last_cleanup_at=?,"
                " last_cleanup_error_code=? WHERE singleton=1",
                (now, error_code),
            )
        return {
            "removed_leaf_folders": removed,
            "retained_active": retained_active,
            "next_expiry_at": next_expiry_at,
            "skipped_missing_worktrees": skipped_missing_worktrees,
            "errors": errors,
            "errors_truncated": error_count > len(errors),
        }

    def _maintenance_loop(self) -> None:
        wait_seconds: float | None = None
        while not self._stop.is_set():
            self._wake.wait(wait_seconds)
            self._wake.clear()
            if self._stop.is_set():
                return
            result = self.run_maintenance_once()
            if result["errors"]:
                wait_seconds = 60.0
                continue
            next_expiry = result.get("next_expiry_at")
            if not isinstance(next_expiry, str):
                wait_seconds = None
                continue
            try:
                deadline = datetime.fromisoformat(next_expiry.replace("Z", "+00:00"))
                wait_seconds = max(
                    0.1, (deadline - datetime.now(UTC)).total_seconds())
            except ValueError:
                wait_seconds = 60.0

    def _resolve(self, path: Path, caller) -> tuple[Path, str]:
        if caller.identity is not None:
            requested = os.path.normpath(str(path))
            rows = self._db.query(
                "SELECT worktree_path,repository_id FROM worktrees WHERE worktree_path=?",
                (requested,),
            )
            if not rows:
                raise ProtocolError(
                    "repository_not_found", "no registered worktree matches this request")
            return Path(rows[0]["worktree_path"]), rows[0]["repository_id"]
        try:
            info = resolve_worktree(path, run_as=(caller.uid, caller.gid))
        except GitResolveError as exc:
            raise ProtocolError("repository_not_found", str(exc)) from exc
        repository_id = ids.repository_id(info.repository_root)
        if not self._db.query(
                "SELECT 1 FROM worktrees WHERE worktree_id=? AND repository_id=?",
                (ids.worktree_id(info.worktree_root), repository_id)):
            raise ProtocolError(
                "repository_not_found", "the worktree is not registered")
        return info.worktree_root, repository_id

    def _invoke(self, command: str, worktree: Path,
                request: dict[str, Any]) -> dict[str, Any]:
        if not self._executor_binary.is_file() \
                or not os.access(self._executor_binary, os.X_OK):
            raise ProtocolError(
                "test_log_unavailable", _BRIDGE_ERROR_MESSAGES["test_log_unavailable"])
        payload = json.dumps(
            request, separators=(",", ":"), sort_keys=True).encode("utf-8")
        if len(payload) > MAX_BRIDGE_BYTES:
            raise ProtocolError("args_invalid", "The log request is too large.")
        try:
            proc = self._bridge_runner(
                [str(self._executor_binary), command, "--worktree", str(worktree),
                 "--request", "-"], payload)
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise ProtocolError(
                "test_log_unavailable", _BRIDGE_ERROR_MESSAGES["test_log_unavailable"]
            ) from exc
        if len(proc.stdout) > MAX_BRIDGE_BYTES or proc.stderr_oversized:
            raise ProtocolError(
                "test_log_unavailable", "The Rust log bridge returned an oversized response.")
        try:
            response = json.loads(proc.stdout)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise ProtocolError(
                "test_log_unavailable", "The Rust log bridge returned invalid JSON.") from exc
        if not isinstance(response, dict) or response.get("schema") != 2 \
                or not isinstance(response.get("ok"), bool):
            raise ProtocolError(
                "test_log_unavailable", "The Rust log bridge returned the wrong schema.")
        if not response["ok"]:
            error = response.get("error")
            code = error.get("code") if isinstance(error, dict) else None
            if code not in _BRIDGE_ERROR_MESSAGES:
                code = "test_log_unavailable"
            raise ProtocolError(code, _BRIDGE_ERROR_MESSAGES[code])
        result = response.get("result")
        if proc.returncode != 0 or not isinstance(result, dict):
            raise ProtocolError(
                "test_log_unavailable", "The Rust log bridge returned an invalid result.")
        return result
