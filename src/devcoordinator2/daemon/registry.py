"""Repository and worktree registration backed by the authority database."""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2 import ids
from devcoordinator2.daemon import events
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.gitinfo import WorktreeInfo, resolve_worktree


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


class RepositoryArchived(RuntimeError):
    def __init__(self, repository_id: str, merged_into_repository_id: str | None):
        self.repository_id = repository_id
        self.merged_into_repository_id = merged_into_repository_id
        suffix = (
            f"; use {merged_into_repository_id}"
            if merged_into_repository_id
            else ""
        )
        super().__init__(f"repository {repository_id} is archived{suffix}")


class RepositoryArchiveBlocked(RuntimeError):
    pass


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
                "SELECT repository_id, archived_at, merged_into_repository_id"
                " FROM repositories WHERE repository_id=?",
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
                if existing["archived_at"] is not None:
                    raise RepositoryArchived(
                        existing["repository_id"],
                        existing["merged_into_repository_id"],
                    )
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

    def list_repositories(self, *, include_archived: bool = False) -> list[dict]:
        where = "" if include_archived else " WHERE archived_at IS NULL"
        repos = self._db.query(
            "SELECT * FROM repositories" + where + " ORDER BY display_name, repository_id"
        )
        repository_ids = {row["repository_id"] for row in repos}
        worktrees = [
            row
            for row in self._db.query(
                "SELECT worktree_id, repository_id, worktree_path FROM worktrees"
            )
            if row["repository_id"] in repository_ids
        ]
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
                "archived_at": r["archived_at"],
                "archived_by_uid": r["archived_by_uid"],
                "archive_note": r["archive_note"],
                "merged_into_repository_id": r["merged_into_repository_id"],
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
        for repo in self.list_repositories(include_archived=True):
            if repo["repository_id"] == repo_id:
                repo["worktree_id"] = ids.worktree_id(info.worktree_root)
                return repo
        return None

    def registered_worktree_paths(self) -> list[Path]:
        return [Path(r["worktree_path"]) for r in self._db.query(
            "SELECT w.worktree_path FROM worktrees w"
            " JOIN repositories r ON r.repository_id=w.repository_id"
            " WHERE r.archived_at IS NULL"
        )]

    def archive(self, repository_id: str, merged_into_repository_id: str,
                note: str, actor_uid: int) -> dict:
        if repository_id == merged_into_repository_id:
            raise RepositoryArchiveBlocked("a repository cannot replace itself")
        now = _now()
        with self._db.transaction() as conn:
            source = conn.execute(
                "SELECT * FROM repositories WHERE repository_id=?", (repository_id,)
            ).fetchone()
            target = conn.execute(
                "SELECT * FROM repositories WHERE repository_id=?",
                (merged_into_repository_id,),
            ).fetchone()
            if source is None or target is None:
                raise RepositoryArchiveBlocked("source and replacement repositories must exist")
            if target["archived_at"] is not None:
                raise RepositoryArchiveBlocked("replacement repository must be active")
            if source["archived_at"] is not None:
                if source["merged_into_repository_id"] == merged_into_repository_id:
                    return dict(source)
                raise RepositoryArchiveBlocked(
                    "repository is already archived with another replacement")

            checks = (
                (
                    "open planning work",
                    "SELECT 1 FROM tasks WHERE repository_id=?"
                    " AND (status IN ('planned','in_progress')"
                    " OR elaboration_needed=1) LIMIT 1",
                ),
                (
                    "open release work",
                    "SELECT 1 FROM releases WHERE repository_id=?"
                    " AND status IN ('planned','requested') LIMIT 1",
                ),
                (
                    "active deployments",
                    "SELECT 1 FROM deployments WHERE repository_id=?"
                    " AND state NOT IN ('stopped','failed') LIMIT 1",
                ),
                (
                    "active observed deployments",
                    "SELECT 1 FROM observed_deployments WHERE repository_id=?"
                    " AND state='running' LIMIT 1",
                ),
            )
            for label, sql in checks:
                if conn.execute(sql, (repository_id,)).fetchone() is not None:
                    raise RepositoryArchiveBlocked(f"repository still has {label}")
            summary_path = (
                Path(source["root_path"]) / ".devcoordinator/test/current/summary.json"
            )
            try:
                summary = json.loads(summary_path.read_text(encoding="utf-8"))
            except (FileNotFoundError, OSError, ValueError):
                summary = {}
            if summary.get("status") == "running":
                raise RepositoryArchiveBlocked("repository still has an active test")

            conn.execute(
                "UPDATE repositories SET archived_at=?, archived_by_uid=?,"
                " archive_note=?, merged_into_repository_id=? WHERE repository_id=?",
                (now, actor_uid, note, merged_into_repository_id, repository_id),
            )
            conn.execute(
                "INSERT INTO repository_events(repository_id,event,"
                " merged_into_repository_id,actor_uid,at,note)"
                " VALUES(?,'archived',?,?,?,?)",
                (repository_id, merged_into_repository_id, actor_uid, now, note),
            )
        return dict(self._db.query(
            "SELECT * FROM repositories WHERE repository_id=?", (repository_id,)
        )[0])

    def unarchive(self, repository_id: str, note: str, actor_uid: int) -> dict:
        now = _now()
        with self._db.transaction() as conn:
            source = conn.execute(
                "SELECT * FROM repositories WHERE repository_id=?", (repository_id,)
            ).fetchone()
            if source is None:
                raise RepositoryArchiveBlocked("repository does not exist")
            if source["archived_at"] is None:
                return dict(source)
            root = Path(source["root_path"])
            if not root.is_dir():
                raise RepositoryArchiveBlocked(
                    "repository checkout must exist before unarchive")
            conn.execute(
                "UPDATE repositories SET archived_at=NULL, archived_by_uid=NULL,"
                " archive_note=NULL, merged_into_repository_id=NULL"
                " WHERE repository_id=?",
                (repository_id,),
            )
            conn.execute(
                "INSERT INTO repository_events(repository_id,event,"
                " merged_into_repository_id,actor_uid,at,note)"
                " VALUES(?,'unarchived',NULL,?,?,?)",
                (repository_id, actor_uid, now, note),
            )
        return dict(self._db.query(
            "SELECT * FROM repositories WHERE repository_id=?", (repository_id,)
        )[0])
