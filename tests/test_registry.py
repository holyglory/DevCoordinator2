import os
import subprocess
from pathlib import Path

import pytest

from devcoordinator2.daemon.db import Database, SchemaMismatch
from devcoordinator2.daemon.gitinfo import GitResolveError, resolve_worktree
from devcoordinator2.daemon.registry import (
    Registry,
    RepositoryArchiveBlocked,
    RepositoryArchived,
)


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


def test_archive_hides_repository_preserves_history_and_can_restore(repo: Path, tmp_path: Path):
    target_root = tmp_path / "target"
    target_root.mkdir()
    _git("init", "-q", cwd=target_root)
    (target_root / "f.txt").write_text("target")
    _git("add", ".", cwd=target_root)
    _git("commit", "-qm", "target", cwd=target_root)
    db = Database(tmp_path / "archive.sqlite3")
    registry = Registry(db)
    uid, gid = os.getuid(), os.getgid()
    source = registry.register(repo, caller_uid=uid, caller_gid=gid)
    target = registry.register(target_root, caller_uid=uid, caller_gid=gid)

    archived = registry.archive(
        source.repository_id, target.repository_id, "Merged skills", uid)

    assert archived["archived_at"]
    assert archived["merged_into_repository_id"] == target.repository_id
    assert [row["repository_id"] for row in registry.list_repositories()] == [
        target.repository_id
    ]
    all_rows = registry.list_repositories(include_archived=True)
    assert {row["repository_id"] for row in all_rows} == {
        source.repository_id,
        target.repository_id,
    }
    assert registry.repository_status(repo)["archived_at"]
    with pytest.raises(RepositoryArchived):
        registry.register(repo, caller_uid=uid, caller_gid=gid)
    assert db.query("SELECT event FROM repository_events")[0]["event"] == "archived"

    restored = registry.unarchive(source.repository_id, "Rollback consolidation", uid)
    assert restored["archived_at"] is None
    assert len(registry.list_repositories()) == 2
    assert [row["event"] for row in db.query(
        "SELECT event FROM repository_events ORDER BY event_id"
    )] == ["archived", "unarchived"]
    db.close()


def test_archive_refuses_open_work(repo: Path, tmp_path: Path):
    target_root = tmp_path / "target"
    target_root.mkdir()
    _git("init", "-q", cwd=target_root)
    (target_root / "f.txt").write_text("target")
    _git("add", ".", cwd=target_root)
    _git("commit", "-qm", "target", cwd=target_root)
    db = Database(tmp_path / "blocked.sqlite3")
    registry = Registry(db)
    uid, gid = os.getuid(), os.getgid()
    source = registry.register(repo, caller_uid=uid, caller_gid=gid)
    target = registry.register(target_root, caller_uid=uid, caller_gid=gid)
    with db.transaction() as conn:
        conn.execute(
            "INSERT INTO tasks(task_id,repository_id,seq,position,title,outcome,"
            " kind,status,created_at,created_by,updated_at)"
            " VALUES('p1',?,1,1,'Open work','Open work','improvement','planned',"
            " 't','fixture','t')",
            (source.repository_id,),
        )

    with pytest.raises(RepositoryArchiveBlocked, match="open planning work"):
        registry.archive(source.repository_id, target.repository_id, "Too early", uid)
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
        conn.execute(
            "INSERT INTO repositories(repository_id,root_path,display_name,"
            " registered_at,registered_by_uid,last_seen_at)"
            " VALUES('r1','/x','x','t',1,'t')"
        )
        conn.execute("UPDATE meta SET value='1' WHERE key='schema_version'")
        conn.execute("DROP TABLE deployments")
    db.close()
    db = Database(path)
    assert db.query("SELECT value FROM meta WHERE key='schema_version'")[0]["value"] == "15"
    assert db.query("SELECT repository_id FROM repositories")[0]["repository_id"] == "r1"
    assert db.query("SELECT count(*) AS n FROM deployments")[0]["n"] == 0
    tables = {row["name"] for row in db.query(
        "SELECT name FROM sqlite_master WHERE type='table'")}
    assert {"compose_completions", "compose_service_desires"}.issubset(tables)
    db.close()


def test_schema_v10_adds_persistent_elaboration_requests(tmp_path: Path):
    path = tmp_path / "db.sqlite3"
    db = Database(path)
    with db.transaction() as conn:
        conn.execute("ALTER TABLE tasks DROP COLUMN elaboration_needed")
        conn.execute("UPDATE meta SET value='10' WHERE key='schema_version'")
    db.close()
    db = Database(path)
    columns = {row["name"] for row in db.query("PRAGMA table_info(tasks)")}
    assert "elaboration_needed" in columns
    assert db.query("SELECT value FROM meta WHERE key='schema_version'")[0]["value"] == "15"
    db.close()


def test_schema_idempotent(tmp_path: Path):
    path = tmp_path / "db.sqlite3"
    Database(path).close()
    Database(path).close()


def test_schema_v12_adds_capacity_without_touching_permanent_history(tmp_path: Path):
    path = tmp_path / "db.sqlite3"
    db = Database(path)
    with db.transaction() as conn:
        conn.execute(
            "INSERT INTO repositories(repository_id,root_path,display_name,registered_at,"
            " registered_by_uid,last_seen_at)"
            " VALUES('rhistory','/history','History','t',1,'t')")
        conn.execute(
            "INSERT INTO decisions(decision_id,repository_id,seq,aspect,title,body,"
            " created_at,created_by) VALUES('nhistory','rhistory',1,'testing','Keep it',"
            " 'Permanent decision','t','fixture')")
        conn.execute("DROP TABLE test_capacity_events")
        conn.execute("DROP TABLE test_capacity_state")
        conn.execute("UPDATE meta SET value='12' WHERE key='schema_version'")
    db.close()

    db = Database(path)
    assert db.query("SELECT body FROM decisions WHERE decision_id='nhistory'")[0]["body"] \
        == "Permanent decision"
    tables = {row["name"] for row in db.query(
        "SELECT name FROM sqlite_master WHERE type='table'")}
    assert {"test_capacity_state", "test_capacity_events"} <= tables
    assert db.query("SELECT value FROM meta WHERE key='schema_version'")[0]["value"] \
        == "15"
    db.close()


def test_schema_v14_adds_visual_feedback_without_touching_plan_history(tmp_path: Path):
    path = tmp_path / "db.sqlite3"
    db = Database(path)
    with db.transaction() as conn:
        conn.execute(
            "INSERT INTO repositories(repository_id,root_path,display_name,registered_at,"
            " registered_by_uid,last_seen_at)"
            " VALUES('rvisual','/visual','Visual','t',1,'t')"
        )
        conn.execute(
            "INSERT INTO tasks(task_id,repository_id,seq,position,title,outcome,kind,status,"
            " created_at,created_by,updated_at)"
            " VALUES('pvisual','rvisual',1,1,'Keep this feedback','Permanent outcome',"
            " 'user_feedback','planned','t','fixture','t')"
        )
        conn.execute("DROP TABLE visual_feedback_events")
        conn.execute("DROP TABLE visual_feedback_comments")
        conn.execute("DROP TABLE visual_feedback")
        conn.execute("UPDATE meta SET value='14' WHERE key='schema_version'")
    db.close()

    db = Database(path)
    assert db.query("SELECT outcome FROM tasks WHERE task_id='pvisual'")[0]["outcome"] \
        == "Permanent outcome"
    tables = {row["name"] for row in db.query(
        "SELECT name FROM sqlite_master WHERE type='table'")}
    assert {"visual_feedback", "visual_feedback_comments",
            "visual_feedback_events"} <= tables
    assert db.query("SELECT value FROM meta WHERE key='schema_version'")[0]["value"] \
        == "15"
    db.close()
