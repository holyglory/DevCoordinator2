import os
import subprocess
from pathlib import Path

import pytest

from devcoordinator2.daemon.db import Database, SchemaMismatch
from devcoordinator2.daemon.gitinfo import GitResolveError, resolve_worktree
from devcoordinator2.daemon.registry import Registry


def _git(*args: str, cwd: Path):
    subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True,
        env={"PATH": "/usr/bin:/bin", "HOME": str(cwd),
             "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
             "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"},
    )


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    root.mkdir()
    _git("init", "-q", cwd=root)
    (root / "f.txt").write_text("x")
    _git("add", ".", cwd=root)
    _git("commit", "-qm", "init", cwd=root)
    return root


def test_resolve_worktree_and_linked_worktree(repo: Path, tmp_path: Path):
    info = resolve_worktree(repo / "f.txt")
    assert info.repository_root == repo.resolve()
    assert info.worktree_root == repo.resolve()

    wt = tmp_path / "wt"
    _git("worktree", "add", "-q", str(wt), cwd=repo)
    info2 = resolve_worktree(wt)
    assert info2.repository_root == repo.resolve()
    assert info2.worktree_root == wt.resolve()


def test_resolve_rejects_non_repo(tmp_path: Path):
    with pytest.raises(GitResolveError):
        resolve_worktree(tmp_path)


def test_register_idempotent_and_worktree_shares_repo(repo: Path, tmp_path: Path):
    db = Database(tmp_path / "db.sqlite3")
    reg = Registry(db)
    uid, gid = os.getuid(), os.getgid()
    first = reg.register(repo, caller_uid=uid, caller_gid=gid)
    assert first.newly_registered
    again = reg.register(repo, caller_uid=uid, caller_gid=gid)
    assert not again.newly_registered
    assert again.repository_id == first.repository_id

    wt = tmp_path / "wt2"
    _git("worktree", "add", "-q", str(wt), cwd=repo)
    linked = reg.register(wt, caller_uid=uid, caller_gid=gid)
    assert linked.repository_id == first.repository_id
    assert linked.worktree_id != first.worktree_id

    repos = reg.list_repositories()
    assert len(repos) == 1
    assert len(repos[0]["worktrees"]) == 2
    assert repos[0]["registered_at"]

    status = reg.repository_status(wt)
    assert status is not None
    assert status["worktree_id"] == linked.worktree_id
    db.close()


def test_schema_newer_than_daemon_refused(tmp_path: Path):
    path = tmp_path / "db.sqlite3"
    db = Database(path)
    with db.transaction() as conn:
        conn.execute("UPDATE meta SET value='999' WHERE key='schema_version'")
    db.close()
    with pytest.raises(SchemaMismatch):
        Database(path)


def test_schema_v1_upgrades_in_place_preserving_repositories(tmp_path: Path):
    path = tmp_path / "db.sqlite3"
    db = Database(path)
    with db.transaction() as conn:
        conn.execute("INSERT INTO repositories VALUES('r1','/x','x','t',1,'t')")
        conn.execute("UPDATE meta SET value='1' WHERE key='schema_version'")
        conn.execute("DROP TABLE deployments")
    db.close()
    db = Database(path)
    assert db.query("SELECT value FROM meta WHERE key='schema_version'")[0]["value"] == "9"
    assert db.query("SELECT repository_id FROM repositories")[0]["repository_id"] == "r1"
    assert db.query("SELECT count(*) AS n FROM deployments")[0]["n"] == 0
    tables = {row["name"] for row in db.query(
        "SELECT name FROM sqlite_master WHERE type='table'")}
    assert {"compose_completions", "compose_service_desires"}.issubset(tables)
    db.close()


def test_schema_idempotent(tmp_path: Path):
    path = tmp_path / "db.sqlite3"
    Database(path).close()
    Database(path).close()
