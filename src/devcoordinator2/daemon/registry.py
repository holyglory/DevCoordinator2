"""Repository and worktree registration backed by the authority database."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2 import ids
from devcoordinator2.daemon import events
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.gitinfo import WorktreeInfo, resolve_worktree


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


@dataclass(frozen=True)
class Registration:
    repository_id: str
    worktree_id: str
    root_path: str
    worktree_path: str
    display_name: str
    newly_registered: bool


class Registry:
    def __init__(self, db: Database):
        self._db = db

    def register(self, path: Path, caller_uid: int,
                 caller_gid: int) -> Registration:
        """Resolve and upsert; implicit on first test.start, explicit via API."""
        info: WorktreeInfo = resolve_worktree(path, run_as=(caller_uid, caller_gid))
        repo_id = ids.repository_id(info.repository_root)
        wt_id = ids.worktree_id(info.worktree_root)
        display_name = info.repository_root.name
        now = _now()
        with self._db.transaction() as conn:
            existing = conn.execute(
                "SELECT repository_id FROM repositories WHERE repository_id=?",
                (repo_id,),
            ).fetchone()
            if existing is None:
                conn.execute(
                    "INSERT INTO repositories(repository_id, root_path, display_name,"
                    " registered_at, registered_by_uid, last_seen_at)"
                    " VALUES(?,?,?,?,?,?)",
                    (repo_id, str(info.repository_root), display_name, now,
                     caller_uid, now),
                )
            else:
                conn.execute(
                    "UPDATE repositories SET last_seen_at=? WHERE repository_id=?",
                    (now, repo_id),
                )
            wt_existing = conn.execute(
                "SELECT worktree_id FROM worktrees WHERE worktree_id=?", (wt_id,)
            ).fetchone()
            if wt_existing is None:
                conn.execute(
                    "INSERT INTO worktrees(worktree_id, repository_id, worktree_path,"
                    " registered_at, last_seen_at) VALUES(?,?,?,?,?)",
                    (wt_id, repo_id, str(info.worktree_root), now, now),
                )
            else:
                conn.execute(
                    "UPDATE worktrees SET last_seen_at=? WHERE worktree_id=?",
                    (now, wt_id),
                )
        if existing is None or wt_existing is None:
            events.publish("repository.registered", repository_id=repo_id,
                           worktree_id=wt_id, display_name=display_name,
                           caller_uid=caller_uid)
        return Registration(
            repository_id=repo_id,
            worktree_id=wt_id,
            root_path=str(info.repository_root),
            worktree_path=str(info.worktree_root),
            display_name=display_name,
            newly_registered=existing is None,
        )

    def list_repositories(self) -> list[dict]:
        repos = self._db.query(
            "SELECT * FROM repositories ORDER BY display_name, repository_id"
        )
        worktrees = self._db.query(
            "SELECT worktree_id, repository_id, worktree_path FROM worktrees"
        )
        by_repo: dict[str, list[dict]] = {}
        for wt in worktrees:
            by_repo.setdefault(wt["repository_id"], []).append(
                {"worktree_id": wt["worktree_id"], "worktree_path": wt["worktree_path"]}
            )
        return [
            {
                "repository_id": r["repository_id"],
                "root_path": r["root_path"],
                "display_name": r["display_name"],
                "registered_at": r["registered_at"],
                "last_seen_at": r["last_seen_at"],
                "worktrees": by_repo.get(r["repository_id"], []),
            }
            for r in repos
        ]

    def repository_status(self, path: Path,
                          run_as: tuple[int, int] | None = None) -> dict | None:
        try:
            info = resolve_worktree(path, run_as=run_as)
        except Exception:
            return None
        repo_id = ids.repository_id(info.repository_root)
        for repo in self.list_repositories():
            if repo["repository_id"] == repo_id:
                repo["worktree_id"] = ids.worktree_id(info.worktree_root)
                return repo
        return None

    def registered_worktree_paths(self) -> list[Path]:
        return [Path(r["worktree_path"]) for r in self._db.query(
            "SELECT worktree_path FROM worktrees"
        )]
