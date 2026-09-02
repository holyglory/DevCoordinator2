"""Deterministic identities derived from durable ownership.

Prefix namespace is recorded in docs/database-ledger.md. Repository and
worktree IDs derive from realpaths so every caller resolves the same
identity; run IDs are time-ordered with a random suffix.
"""

from __future__ import annotations

import hashlib
import secrets
from datetime import UTC, datetime
from pathlib import Path

_REPO_NS = b"devcoordinator2.repository\0"
_WORKTREE_NS = b"devcoordinator2.worktree\0"
_OBSERVED_DEPLOYMENT_NS = b"devcoordinator2.observed-deployment\0"


def _digest(namespace: bytes, path: Path) -> str:
    canonical = str(path.resolve()).encode("utf-8")
    return hashlib.sha256(namespace + canonical).hexdigest()[:16]


def repository_id(git_common_root: Path) -> str:
    return "r" + _digest(_REPO_NS, git_common_root)


def worktree_id(worktree_root: Path) -> str:
    return "w" + _digest(_WORKTREE_NS, worktree_root)


def observed_deployment_id(repository_id: str, native_project: str) -> str:
    raw = f"{repository_id}\0{native_project}".encode()
    return "d" + hashlib.sha256(_OBSERVED_DEPLOYMENT_NS + raw).hexdigest()[:16]


def run_id(now: datetime | None = None) -> str:
    stamp = (now or datetime.now(UTC)).strftime("%Y%m%dT%H%M%SZ")
    return f"t{stamp}-{secrets.token_hex(3)}"


def task_id() -> str:
    """Plan task (schema 8): random at creation, like user IDs."""
    return "p" + secrets.token_hex(8)


def release_id() -> str:
    return "v" + secrets.token_hex(8)


def decision_id() -> str:
    return "n" + secrets.token_hex(8)


def feedback_id() -> str:
    """Screenshot-anchored feedback thread (schema 15)."""
    return "f" + secrets.token_hex(8)


def comment_id() -> str:
    """Message inside a screenshot feedback thread (schema 15)."""
    return "m" + secrets.token_hex(8)


def unit_name(prefix: str, wt_id: str, suffix: str) -> str:
    return f"{prefix}-{wt_id}-{suffix}.service"


def unit_glob(prefix: str, wt_id: str) -> str:
    """Glob matching every test unit ever launched for a worktree."""
    return f"{prefix}-{wt_id}-*.service"
