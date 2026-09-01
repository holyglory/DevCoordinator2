"""Helpers for the test lifecycle: repository-local bookkeeping files,
container cleanup, and the cross-worktree listing."""

from __future__ import annotations

import json
import logging
import os
import re
import stat
from pathlib import Path

from devcoordinator2.daemon import docker_cli, securefs, summary
from devcoordinator2.daemon.db import Database
from devcoordinator2.paths import test_dir
from devcoordinator2.protocol import ProtocolError

CONTAINERS_FILE = "containers.json"
ENV_FILE = "env"
PLAN_FILE = "check-plan.json"
REPORT_FILE = "check-report.json"
HISTORY_SCHEMA = 1
HISTORY_CAP = 1000
EVIDENCE_SCHEMA = 1
EVIDENCE_CAP = 50
HISTORY_FIELDS = (
    "run_id", "test", "status", "started_at", "finished_at",
    "duration_seconds", "exit_code",
)
log = logging.getLogger("devcoordinator2.tests")
_DIGEST_RE = re.compile(r"[0-9a-f]{64}$")
_CHECK_STATES = frozenset({
    "pending", "running", "passed", "failed", "not_meaningful",
    "cancelled", "unsafe", "reused",
})


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
    if document is None or document.get("schema") != 1 \
            or document.get("proof") not in ("complete", "diagnostic") \
            or document.get("status") not in ("running", "passed", "failed") \
            or not isinstance(document.get("run_id"), str) \
            or not isinstance(document.get("test"), str) \
            or not isinstance(document.get("selection"), list) \
            or len(document["selection"]) > 256 \
            or not all(isinstance(name, str) for name in document["selection"]) \
            or not isinstance(document.get("source_digest"), str) \
            or not _DIGEST_RE.fullmatch(document["source_digest"]) \
            or not isinstance(document.get("config_digest"), str) \
            or not _DIGEST_RE.fullmatch(document["config_digest"]) \
            or (document.get("unsafe_reason") is not None
                and (not isinstance(document["unsafe_reason"], str)
                     or len(document["unsafe_reason"]) > 512)) \
            or not isinstance(document.get("counts"), dict):
        return None
    checks = document.get("checks")
    counts = document["counts"]
    if set(counts) != _CHECK_STATES | {"pending", "running"} \
            or any(not isinstance(value, int) or value < 0 or value > 256
                   for value in counts.values()):
        return None
    if not isinstance(checks, list) or len(checks) > 256:
        return None
    names = set()
    for row in checks:
        if not isinstance(row, dict) or not isinstance(row.get("name"), str) \
                or row["name"] in names or row.get("status") not in _CHECK_STATES \
                or (row.get("reason") is not None
                    and (not isinstance(row["reason"], str)
                         or len(row["reason"]) > 512)) \
                or (row.get("exit_code") is not None
                    and not isinstance(row["exit_code"], int)) \
                or (row.get("duration_seconds") is not None
                    and not isinstance(row["duration_seconds"], int | float)) \
                or row.get("output_ref") != f"checks/{row.get('name')}":
            return None
        for stream in ("stdout", "stderr"):
            observed = row.get(f"{stream}_bytes_observed")
            retained = row.get(f"{stream}_bytes_retained")
            truncated = row.get(f"{stream}_truncated")
            if (observed is not None and (not isinstance(observed, int) or observed < 0)) \
                    or (retained is not None
                        and (not isinstance(retained, int) or retained < 0)) \
                    or (observed is not None and retained is not None
                        and retained > observed) \
                    or (truncated is not None and not isinstance(truncated, bool)):
                return None
        names.add(row["name"])
        artifacts = row.get("artifacts")
        if not isinstance(artifacts, list) or len(artifacts) > 16:
            return None
        for artifact in artifacts:
            if not isinstance(artifact, dict) \
                    or not isinstance(artifact.get("path"), str) \
                    or len(artifact["path"]) > 256 \
                    or not isinstance(artifact.get("size"), int) \
                    or artifact["size"] < 0 \
                    or not isinstance(artifact.get("sha256"), str) \
                    or not _DIGEST_RE.fullmatch(artifact["sha256"]):
                return None
    failures = document.get("failure_index")
    if not isinstance(failures, list) or len(failures) > 129:
        return None
    for row in failures:
        if not isinstance(row, dict) \
                or row.get("status") not in _CHECK_STATES \
                or (row.get("check") is not None
                    and not isinstance(row["check"], str)) \
                or (row.get("reason") is not None
                    and (not isinstance(row["reason"], str)
                         or len(row["reason"]) > 512)):
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
    if not isinstance(document, dict) or document.get("schema") != HISTORY_SCHEMA \
            or not isinstance(document.get("runs"), list):
        raise securefs.SecureFsError("test history has the wrong schema")
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
            "exit_code": row.get("exit_code"),
            "artifacts": row.get("artifacts", []),
            "stdout_bytes_observed": row.get("stdout_bytes_observed"),
            "stdout_bytes_retained": row.get("stdout_bytes_retained"),
            "stdout_truncated": row.get("stdout_truncated"),
            "stderr_bytes_observed": row.get("stderr_bytes_observed"),
            "stderr_bytes_retained": row.get("stderr_bytes_retained"),
            "stderr_truncated": row.get("stderr_truncated"),
        })
    return {
        "run_id": report.get("run_id"),
        "test": report.get("test"),
        "proof": report.get("proof"),
        "status": report.get("status"),
        "source_digest": report.get("source_digest"),
        "config_digest": report.get("config_digest"),
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
    if not isinstance(document, dict) or document.get("schema") != EVIDENCE_SCHEMA \
            or not isinstance(document.get("runs"), list):
        raise securefs.SecureFsError("test evidence has the wrong schema")
    runs = document["runs"]
    if len(runs) > EVIDENCE_CAP or any(
            not isinstance(run, dict)
            or not isinstance(run.get("run_id"), str)
            or run.get("proof") not in ("complete", "diagnostic")
            or run.get("status") not in ("passed", "failed")
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
            "complete", "diagnostic"):
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
