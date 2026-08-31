"""Helpers for the test lifecycle: repository-local bookkeeping files,
container cleanup, and the cross-worktree listing."""

from __future__ import annotations

import json
import logging
import os
from pathlib import Path

from devcoordinator2.daemon import docker_cli, securefs, summary
from devcoordinator2.daemon.db import Database
from devcoordinator2.paths import test_dir
from devcoordinator2.protocol import ProtocolError

CONTAINERS_FILE = "containers.json"
ENV_FILE = "env"
HISTORY_SCHEMA = 1
HISTORY_CAP = 1000
HISTORY_FIELDS = (
    "run_id", "test", "status", "started_at", "finished_at",
    "duration_seconds", "exit_code",
)
log = logging.getLogger("devcoordinator2.tests")


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
