"""Helpers for the test lifecycle: repository-local bookkeeping files,
container cleanup, and the cross-worktree listing."""

from __future__ import annotations

import json
import logging
import math
import os
import re
import stat
from pathlib import Path
from pathlib import PurePosixPath

from devcoordinator2.daemon import docker_cli, securefs, summary
from devcoordinator2.daemon.db import Database
from devcoordinator2.paths import test_dir
from devcoordinator2.protocol import ProtocolError

CONTAINERS_FILE = "containers.json"
ENV_FILE = "env"
PLAN_FILE = "check-plan.json"
REPORT_FILE = "check-report.json"
HISTORY_SCHEMA = 2
HISTORY_CAP = 1000
EVIDENCE_SCHEMA = 2
EVIDENCE_CAP = 50
HISTORY_FIELDS = (
    "run_id", "test", "status", "started_at", "finished_at",
    "duration_seconds", "exit_code",
)
log = logging.getLogger("devcoordinator2.tests")
_DIGEST_RE = re.compile(r"[0-9a-f]{64}$")
_FINGERPRINT_RE = re.compile(r"sha256:[0-9a-f]{64}$")
_IDENTITY_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_CHECK_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,63}$")
_CHECK_STATES = frozenset({
    "pending", "running", "passed", "failed", "not_meaningful",
    "cancelled", "unsafe", "reused", "invalidated", "timed_out",
})
_FAILURE_STATES = _CHECK_STATES - {"pending", "running", "passed", "reused"}
_LOG_PHASES = frozenset({"executor", "check", "discovery", "case"})
_LOG_STREAMS = frozenset({"stdout", "stderr"})
_ERROR_CATEGORIES = frozenset({
    "assertion", "compiler", "panic", "exception", "stack_frame", "timeout",
    "cancellation", "process_exit", "browser_console", "network_request",
    "structured_evidence_invalid", "log_storage", "source_changed", "artifact",
    "dependency", "internal",
})
_TERMINATION_REASONS = frozenset({
    "deadline_exceeded", "user_cancelled", "superseded", "run_cancelled",
    "daemon_interrupted", "unsafe_stop",
})
_DIAGNOSTIC_ORIGINS = frozenset({
    "explicit_event", "junit", "playwright_json", "rust_json", "executor",
})
_VALUE_TYPES = frozenset({"null", "boolean", "number", "string", "json"})


def _plain_int(value, *, minimum: int = 0, maximum: int | None = None) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) \
        and value >= minimum and (maximum is None or value <= maximum)


def _valid_exit(value) -> bool:
    if not isinstance(value, dict) or set(value) != {"code", "signal"}:
        return False
    code, signal = value["code"], value["signal"]
    if code is not None and not _plain_int(code, minimum=-(1 << 31), maximum=(1 << 31) - 1):
        return False
    if signal is not None and not _plain_int(signal, minimum=1, maximum=127):
        return False
    return code is None or signal is None


def _valid_log_ref(value) -> bool:
    if not isinstance(value, dict) or set(value) != {
            "run_id", "check", "phase", "case", "stream"}:
        return False
    run_id, check, phase = value["run_id"], value["check"], value["phase"]
    case, stream = value["case"], value["stream"]
    if not isinstance(run_id, str) or not _IDENTITY_RE.fullmatch(run_id) \
            or phase not in _LOG_PHASES or stream not in _LOG_STREAMS:
        return False
    if phase == "executor":
        return check is None and case is None
    if not isinstance(check, str) or not _CHECK_RE.fullmatch(check):
        return False
    if phase in ("check", "discovery"):
        return case is None
    return isinstance(case, str) and _IDENTITY_RE.fullmatch(case) is not None


def _valid_stream(value) -> bool:
    if not isinstance(value, dict) or set(value) != {
            "log_ref", "bytes", "lines", "sha256", "first_write_epoch_ms",
            "last_write_epoch_ms", "complete"}:
        return False
    size, lines = value["bytes"], value["lines"]
    first, last = value["first_write_epoch_ms"], value["last_write_epoch_ms"]
    if not _valid_log_ref(value["log_ref"]) \
            or not _plain_int(size) or not _plain_int(lines) \
            or not isinstance(value["sha256"], str) \
            or not _DIGEST_RE.fullmatch(value["sha256"]) \
            or not isinstance(value["complete"], bool):
        return False
    if lines > size or (size == 0 and lines != 0):
        return False
    if (first is None) != (last is None):
        return False
    if size == 0:
        return first is None
    return _plain_int(first) and _plain_int(last) and last >= first


def _valid_source(value) -> bool:
    if value is None:
        return True
    if not isinstance(value, dict) or set(value) != {"file", "line", "column"}:
        return False
    path = value["file"]
    if not isinstance(path, str) or not path or len(path.encode()) > 512 \
            or "\\" in path or "\0" in path:
        return False
    parsed = PurePosixPath(path)
    if parsed.is_absolute() or any(part in ("", ".", "..") for part in parsed.parts) \
            or parsed.as_posix() != path:
        return False
    return _plain_int(value["line"], minimum=1) \
        and (value["column"] is None or _plain_int(value["column"], minimum=1))


def _valid_diagnostic_value(value) -> bool:
    if value is None:
        return True
    if not isinstance(value, dict) or set(value) != {
            "type", "preview", "byte_count", "sha256", "truncated", "redacted"}:
        return False
    preview = value["preview"]
    if value["type"] not in _VALUE_TYPES or not _plain_int(value["byte_count"]) \
            or not isinstance(value["sha256"], str) \
            or not _DIGEST_RE.fullmatch(value["sha256"]) \
            or not isinstance(value["truncated"], bool) \
            or not isinstance(value["redacted"], bool):
        return False
    if preview is not None and (
            not isinstance(preview, str) or len(preview.encode()) > 256
            or any(ord(character) < 32 or ord(character) == 127
                   for character in preview)):
        return False
    return not value["redacted"] or preview is None


def _valid_failure(value) -> bool:
    if not isinstance(value, dict) or set(value) != {
            "check", "case", "status", "exit", "termination_reason", "source",
            "error_category", "expected", "actual", "fingerprint", "occurrences",
            "log_refs", "origin"}:
        return False
    check, case = value["check"], value["case"]
    if check is not None and (not isinstance(check, str) or not _CHECK_RE.fullmatch(check)):
        return False
    if case is not None and (
            check is None or not isinstance(case, str) or not case
            or len(case.encode()) > 256
            or any(ord(character) < 32 or ord(character) == 127
                   for character in case)):
        return False
    refs = value["log_refs"]
    return value["status"] in _FAILURE_STATES \
        and _valid_exit(value["exit"]) \
        and (value["termination_reason"] is None
             or value["termination_reason"] in _TERMINATION_REASONS) \
        and _valid_source(value["source"]) \
        and value["error_category"] in _ERROR_CATEGORIES \
        and _valid_diagnostic_value(value["expected"]) \
        and _valid_diagnostic_value(value["actual"]) \
        and isinstance(value["fingerprint"], str) \
        and _FINGERPRINT_RE.fullmatch(value["fingerprint"]) is not None \
        and _plain_int(value["occurrences"], minimum=1) \
        and isinstance(refs, list) and len(refs) <= 16 \
        and all(_valid_log_ref(ref) for ref in refs) \
        and len({json.dumps(ref, sort_keys=True) for ref in refs}) == len(refs) \
        and value["origin"] in _DIAGNOSTIC_ORIGINS


def _valid_optional_timestamp(value) -> bool:
    return value is None or (
        isinstance(value, str) and 1 <= len(value) <= 64
        and "\n" not in value and "\r" not in value
    )


def _valid_duration(value) -> bool:
    return value is None or (
        isinstance(value, int | float) and not isinstance(value, bool)
        and math.isfinite(value) and value >= 0
    )


def _valid_artifact(value) -> bool:
    if not isinstance(value, dict) or set(value) != {"path", "size", "sha256"}:
        return False
    path = value["path"]
    if not isinstance(path, str) or not path or len(path.encode()) > 256 \
            or "\\" in path or "\0" in path:
        return False
    parsed = PurePosixPath(path)
    return not parsed.is_absolute() \
        and all(part not in ("", ".", "..") for part in parsed.parts) \
        and parsed.as_posix() == path \
        and _plain_int(value["size"]) \
        and isinstance(value["sha256"], str) \
        and _DIGEST_RE.fullmatch(value["sha256"]) is not None


def _valid_case_report(value, check_name: str) -> bool:
    if not isinstance(value, dict) or set(value) != {
            "id", "status", "exit", "duration_ms", "streams"}:
        return False
    case_id, streams = value["id"], value["streams"]
    if not isinstance(case_id, str) or not _IDENTITY_RE.fullmatch(case_id) \
            or value["status"] not in _CHECK_STATES or not _valid_exit(value["exit"]) \
            or not _plain_int(value["duration_ms"]) \
            or not isinstance(streams, list) or len(streams) > 2 \
            or not all(_valid_stream(stream) for stream in streams):
        return False
    refs = [stream["log_ref"] for stream in streams]
    return all(ref["check"] == check_name and ref["phase"] == "case"
               and ref["case"] == case_id for ref in refs) \
        and len({ref["stream"] for ref in refs}) == len(refs)


def _valid_check_report(value) -> bool:
    if not isinstance(value, dict) or set(value) != {
            "name", "tier", "role", "status", "started_at", "finished_at",
            "duration_seconds", "exit", "artifacts", "streams", "case_count",
            "cases", "cases_truncated"}:
        return False
    name, streams, cases = value["name"], value["streams"], value["cases"]
    if not isinstance(name, str) or not _CHECK_RE.fullmatch(name) \
            or value["tier"] not in ("development", "pre-merge", "release") \
            or value["role"] not in ("work", "preflight") \
            or value["status"] not in _CHECK_STATES \
            or not _valid_optional_timestamp(value["started_at"]) \
            or not _valid_optional_timestamp(value["finished_at"]) \
            or not _valid_duration(value["duration_seconds"]) \
            or not _valid_exit(value["exit"]) \
            or not isinstance(value["artifacts"], list) \
            or len(value["artifacts"]) > 16 \
            or not all(_valid_artifact(item) for item in value["artifacts"]) \
            or not isinstance(streams, list) or len(streams) > 2 \
            or not all(_valid_stream(stream) for stream in streams) \
            or not _plain_int(value["case_count"], maximum=4096) \
            or not isinstance(cases, list) or len(cases) > 128 \
            or not isinstance(value["cases_truncated"], bool) \
            or not all(_valid_case_report(case, name) for case in cases):
        return False
    refs = [stream["log_ref"] for stream in streams]
    if not all(ref["check"] == name and ref["case"] is None
               and ref["phase"] in ("check", "discovery") for ref in refs):
        return False
    if len({ref["stream"] for ref in refs}) != len(refs):
        return False
    return value["case_count"] >= len(cases) \
        and value["cases_truncated"] == (value["case_count"] > len(cases))


def _read_json_at(dir_fd: int, name: str, max_bytes: int = 2 * 1024 * 1024) \
        -> dict | None:
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=dir_fd)
    except (FileNotFoundError, OSError):
        return None
    try:
        details = os.fstat(fd)
        if not stat.S_ISREG(details.st_mode) or details.st_size > max_bytes:
            return None
        chunks = []
        remaining = max_bytes + 1
        while remaining:
            block = os.read(fd, min(65536, remaining))
            if not block:
                break
            chunks.append(block)
            remaining -= len(block)
        payload = b"".join(chunks)
        if len(payload) > max_bytes:
            return None
        value = json.loads(payload)
    except (OSError, json.JSONDecodeError, UnicodeDecodeError):
        return None
    finally:
        os.close(fd)
    return value if isinstance(value, dict) else None


def _write_json_at(dir_fd: int, name: str, document: dict,
                   owner: tuple[int, int], mode: int = 0o600) -> None:
    payload = (json.dumps(document, separators=(",", ":"), sort_keys=True)
               + "\n").encode()
    if len(payload) > 2 * 1024 * 1024:
        raise ProtocolError("test_start_failed", "governed-check plan is too large")
    tmp_name = f".{name}-{os.getpid()}"
    fd = os.open(tmp_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 mode, dir_fd=dir_fd)
    try:
        written = 0
        while written < len(payload):
            written += os.write(fd, payload[written:])
        os.fsync(fd)
        os.fchmod(fd, mode)
        os.fchown(fd, owner[0], owner[1])
    finally:
        os.close(fd)
    os.replace(tmp_name, name, src_dir_fd=dir_fd, dst_dir_fd=dir_fd)
    os.fsync(dir_fd)


def write_check_plan(dir_fd: int, document: dict,
                     owner: tuple[int, int]) -> None:
    _write_json_at(dir_fd, PLAN_FILE, document, owner)


def read_check_report(dir_fd: int) -> dict | None:
    document = _read_json_at(dir_fd, REPORT_FILE)
    if document is None or set(document) != {
            "schema", "run_id", "test", "requested_tier", "readiness_eligible",
            "proof", "selection", "origin_run_id", "status", "started_at",
            "finished_at", "duration_seconds", "source_digest", "config_digest",
            "source_changed", "capacity", "counts", "checks", "failure_index",
            "failure_index_truncated"} \
            or document.get("schema") != 2 \
            or document.get("proof") not in ("complete", "selected", "retry") \
            or document.get("status") not in ("running", "passed", "failed") \
            or not isinstance(document.get("run_id"), str) \
            or not _IDENTITY_RE.fullmatch(document["run_id"]) \
            or not isinstance(document.get("test"), str) \
            or not _CHECK_RE.fullmatch(document["test"]) \
            or not isinstance(document.get("selection"), list) \
            or len(document["selection"]) > 256 \
            or not all(isinstance(name, str) and _CHECK_RE.fullmatch(name)
                       for name in document["selection"]) \
            or document.get("requested_tier") not in (
                "development", "pre-merge", "release") \
            or not isinstance(document.get("readiness_eligible"), bool) \
            or document["readiness_eligible"] != (
                document["proof"] == "complete"
                and document["requested_tier"] == "release") \
            or not isinstance(document.get("source_digest"), str) \
            or not _DIGEST_RE.fullmatch(document["source_digest"]) \
            or not isinstance(document.get("config_digest"), str) \
            or not _DIGEST_RE.fullmatch(document["config_digest"]) \
            or not isinstance(document.get("source_changed"), bool) \
            or not _valid_optional_timestamp(document.get("started_at")) \
            or not _valid_optional_timestamp(document.get("finished_at")) \
            or not _valid_duration(document.get("duration_seconds")) \
            or not isinstance(document.get("failure_index_truncated"), bool) \
            or not isinstance(document.get("counts"), dict):
        return None
    capacity = document.get("capacity")
    if not isinstance(capacity, dict) or set(capacity) != {
            "learned_capacity", "effective_capacity", "capacity_wait_count"}:
        return None
    for key in ("learned_capacity", "effective_capacity"):
        value = capacity[key]
        if value is not None and (
                not isinstance(value, int) or isinstance(value, bool) or value < 1):
            return None
    if not isinstance(capacity["capacity_wait_count"], int) \
            or isinstance(capacity["capacity_wait_count"], bool) \
            or capacity["capacity_wait_count"] < 0:
        return None
    origin_run_id = document.get("origin_run_id")
    if origin_run_id is not None and not isinstance(origin_run_id, str):
        return None
    proof = document["proof"]
    selection = document["selection"]
    if (proof == "complete" and (selection or origin_run_id is not None)) \
            or (proof == "selected" and (not selection or origin_run_id is not None)) \
            or (proof == "retry" and (len(selection) != 1
                                       or not isinstance(origin_run_id, str)
                                       or not origin_run_id)):
        return None
    checks = document.get("checks")
    counts = document["counts"]
    if set(counts) != _CHECK_STATES | {"pending", "running"} \
            or any(not isinstance(value, int) or value < 0 or value > 4096
                   for value in counts.values()):
        return None
    if not isinstance(checks, list) or len(checks) > 256:
        return None
    names = set()
    for row in checks:
        if not _valid_check_report(row) or row["name"] in names:
            return None
        names.add(row["name"])
    failures = document.get("failure_index")
    if not isinstance(failures, list) or len(failures) > 128:
        return None
    if not all(_valid_failure(row) for row in failures):
        return None
    return document


def history_entry(doc: dict) -> dict:
    return {field: doc.get(field) for field in HISTORY_FIELDS}


def read_history(worktree_root: Path) -> list[dict]:
    """Read the bounded repository-local terminal test history."""
    payload = securefs.read_test_history(worktree_root)
    if payload is None:
        return []
    try:
        document = json.loads(payload)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise securefs.SecureFsError("test history is not valid JSON") from exc
    if not isinstance(document, dict) or document.get("schema") != HISTORY_SCHEMA:
        return []  # strict cutover: old history is unavailable, never translated
    if not isinstance(document.get("runs"), list):
        raise securefs.SecureFsError("test history contains invalid runs")
    runs = document["runs"]
    if len(runs) > HISTORY_CAP or any(
            not isinstance(run, dict)
            or set(run) != set(HISTORY_FIELDS)
            or not isinstance(run.get("run_id"), str)
            or run.get("status") not in summary.TERMINAL_STATUSES
            for run in runs):
        raise securefs.SecureFsError("test history contains invalid runs")
    return runs


def record_history(worktree_root: Path, doc: dict,
                   owner: tuple[int, int]) -> None:
    """Record one terminal result, deduplicated and bounded to 1,000 runs."""
    if doc.get("status") not in summary.TERMINAL_STATUSES:
        return
    runs = read_history(worktree_root)
    entry = history_entry(doc)
    runs = [run for run in runs if run["run_id"] != entry["run_id"]]
    runs.append(entry)
    document = {"schema": HISTORY_SCHEMA, "runs": runs[-HISTORY_CAP:]}
    securefs.write_test_history(
        worktree_root,
        (json.dumps(document, separators=(",", ":"), sort_keys=True) + "\n").encode(),
        owner)


def _evidence_row(report: dict) -> dict:
    checks = []
    for row in report.get("checks", []):
        if not isinstance(row, dict):
            continue
        checks.append({
            "name": row.get("name"),
            "status": row.get("status"),
            "duration_seconds": row.get("duration_seconds"),
            "exit": row.get("exit"),
            "artifacts": row.get("artifacts", []),
            "streams": row.get("streams", []),
        })
    return {
        "run_id": report.get("run_id"),
        "test": report.get("test"),
        "proof": report.get("proof"),
        "status": report.get("status"),
        "source_digest": report.get("source_digest"),
        "config_digest": report.get("config_digest"),
        "requested_tier": report.get("requested_tier"),
        "readiness_eligible": report.get("readiness_eligible"),
        "selection": report.get("selection", []),
        "checks": checks,
    }


def read_evidence(worktree_root: Path) -> list[dict]:
    payload = securefs.read_test_evidence(worktree_root)
    if payload is None:
        return []
    try:
        document = json.loads(payload)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise securefs.SecureFsError("test evidence is not valid JSON") from exc
    if not isinstance(document, dict) or document.get("schema") != EVIDENCE_SCHEMA:
        return []  # strict cutover: old evidence cannot authorize a retry
    if not isinstance(document.get("runs"), list):
        raise securefs.SecureFsError("test evidence contains invalid runs")
    runs = document["runs"]
    if len(runs) > EVIDENCE_CAP or any(
            not isinstance(run, dict)
            or not isinstance(run.get("run_id"), str)
            or run.get("proof") not in ("complete", "selected", "retry")
            or run.get("status") not in ("passed", "failed")
            or run.get("requested_tier") not in (
                "development", "pre-merge", "release")
            or not isinstance(run.get("readiness_eligible"), bool)
            or not isinstance(run.get("checks"), list)
            for run in runs):
        raise securefs.SecureFsError("test evidence contains invalid runs")
    return runs


def record_evidence(worktree_root: Path, report: dict,
                    owner: tuple[int, int]) -> None:
    if report.get("status") not in ("passed", "failed"):
        return
    row = _evidence_row(report)
    if not isinstance(row["run_id"], str) or row["proof"] not in (
            "complete", "selected", "retry"):
        return
    runs = [run for run in read_evidence(worktree_root)
            if run["run_id"] != row["run_id"]]
    runs.append(row)
    payload = (json.dumps({"schema": EVIDENCE_SCHEMA,
                           "runs": runs[-EVIDENCE_CAP:]},
                          separators=(",", ":"), sort_keys=True) + "\n").encode()
    securefs.write_test_evidence(worktree_root, payload, owner)


def find_evidence(worktree_root: Path, run_id: str) -> dict | None:
    return next((row for row in reversed(read_evidence(worktree_root))
                 if row["run_id"] == run_id), None)


def list_current(db: Database, runs: dict) -> list[dict]:
    """One current/most-recent run per registered worktree (no log text)."""
    out = []
    rows = db.query(
        "SELECT w.worktree_id, w.worktree_path, w.repository_id, r.display_name"
        " FROM worktrees w JOIN repositories r ON r.repository_id = w.repository_id"
        " WHERE r.archived_at IS NULL"
        " ORDER BY r.display_name, w.worktree_path")
    for row in rows:
        path = test_dir(Path(row["worktree_path"])) / "summary.json"
        doc = summary.read(path)
        if doc is None:
            continue
        handle = runs.get(row["worktree_id"])
        live = handle is not None and handle.final_status is None
        if live and doc["status"] == "running":
            doc["stdout_bytes_observed"] = handle.out.counts.observed
            doc["stderr_bytes_observed"] = handle.err.counts.observed
        doc.update(worktree_id=row["worktree_id"], worktree_path=row["worktree_path"],
                   repository_id=row["repository_id"], display_name=row["display_name"],
                   summary_path=str(path))
        out.append(doc)
    return out


def _write_containers(dir_fd: int, containers: list[str],
                      owner: tuple[int, int]) -> None:
    """Record exact owned container IDs beside the summary for recovery."""
    payload = json.dumps({"containers": containers}).encode()
    fd = os.open(CONTAINERS_FILE, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644,
                 dir_fd=dir_fd)
    try:
        os.write(fd, payload)
        os.fchown(fd, owner[0], owner[1])
    finally:
        os.close(fd)


def _write_env_file(dir_fd: int, env: dict[str, str],
                    owner: tuple[int, int]) -> None:
    """systemd EnvironmentFile syntax, caller-owned, mode 0600."""
    lines = []
    for key, value in env.items():
        if not key.isidentifier():
            raise ProtocolError("repository_config_invalid",
                                f"invalid environment variable name {key!r}")
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        lines.append(f'{key}="{escaped}"')
    fd = os.open(ENV_FILE, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600,
                 dir_fd=dir_fd)
    try:
        os.write(fd, ("\n".join(lines) + "\n").encode())
        os.fchmod(fd, 0o600)
        os.fchown(fd, owner[0], owner[1])
    finally:
        os.close(fd)


def _remove_containers(container_ids: list[str]) -> None:
    for container_id in list(container_ids):
        try:
            docker_cli.remove_exact(container_id)
        except docker_cli.DockerError as exc:
            log.error("container cleanup failed for %s: %s", container_id, exc)
