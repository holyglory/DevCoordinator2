from __future__ import annotations

import importlib.util
import json
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from devcoordinator2.ids import repository_id

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "devcoordinator2_install", ROOT / "scripts/install.py"
)
assert SPEC and SPEC.loader
install = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(install)


def test_install_skill_links_replaces_only_existing_agent_roots(tmp_path, monkeypatch):
    home = tmp_path / "home"
    codex = home / ".codex"
    claude = home / ".claude"
    codex.mkdir(parents=True)
    claude.mkdir()
    old = tmp_path / "old-skill"
    old.mkdir()
    (codex / "skills").mkdir()
    (codex / "skills" / "codex-dev-coordinator").symlink_to(old)
    release_skill = tmp_path / "release" / "skills" / "codex-dev-coordinator"
    release_skill.mkdir(parents=True)
    monkeypatch.setattr(install.pwd, "getpwnam", lambda _name: SimpleNamespace(
        pw_dir=str(home), pw_uid=1000, pw_gid=1000))
    monkeypatch.setattr(install.os, "chown", lambda *_args: None)

    links = install.install_skill_links(["developer"], release_skill)

    expected = {
        str(codex / "skills" / "codex-dev-coordinator"),
        str(claude / "skills" / "codex-dev-coordinator"),
    }
    assert set(links) == expected
    for link in expected:
        assert Path(link).is_symlink()
        assert Path(link).readlink() == release_skill


def test_install_skill_links_refuses_non_symlink(tmp_path, monkeypatch):
    home = tmp_path / "home"
    skill = home / ".codex" / "skills" / "codex-dev-coordinator"
    skill.mkdir(parents=True)
    release_skill = tmp_path / "release-skill"
    release_skill.mkdir()
    monkeypatch.setattr(install.pwd, "getpwnam", lambda _name: SimpleNamespace(
        pw_dir=str(home), pw_uid=1000, pw_gid=1000))

    with pytest.raises(RuntimeError, match="refusing to replace non-symlink"):
        install.install_skill_links(["developer"], release_skill)


def test_compose_env_authorization_binds_ignored_path_to_repository(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    (repo / ".gitignore").write_text("private/dev.env\n")
    (repo / "private").mkdir()
    (repo / "private" / "dev.env").write_text("PASSWORD=not-read-by-installer\n")

    entries = install.compose_env_authorizations([
        f"{repo}=private/dev.env",
    ])

    assert entries == [{
        "repository_id": repository_id(repo),
        "path": "private/dev.env",
    }]


def test_compose_env_authorization_rejects_tracked_or_escaping_path(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    (repo / "tracked.env").write_text("VALUE=x\n")
    with pytest.raises(ValueError, match="must be ignored"):
        install.compose_env_authorizations([f"{repo}=tracked.env"])
    with pytest.raises(ValueError, match="normalized relative"):
        install.compose_env_authorizations([f"{repo}=../outside.env"])


def test_compose_env_allowlist_merge_is_atomic_and_preserves_entries(
    tmp_path, monkeypatch,
):
    allowlist = tmp_path / "allowlist.json"
    allowlist.write_text(json.dumps({
        "schema": 1,
        "authorizations": [{"repository_id": "r" + "a" * 16, "path": "a.env"}],
    }))
    os.chmod(allowlist, 0o640)
    monkeypatch.setattr(install.os, "chown", lambda *_args: None)

    changed = install.merge_compose_env_allowlist(
        allowlist,
        [{"repository_id": "r" + "b" * 16, "path": "b.env"}],
        (0, 0),
    )

    assert changed is True
    assert json.loads(allowlist.read_text())["authorizations"] == [
        {"repository_id": "r" + "a" * 16, "path": "a.env"},
        {"repository_id": "r" + "b" * 16, "path": "b.env"},
    ]
    assert allowlist.stat().st_mode & 0o777 == 0o640


def test_ensure_env_value_appends_once_and_refuses_conflict(tmp_path):
    target = tmp_path / "instance.env"
    target.write_text("A=1\n")
    os.chmod(target, 0o640)
    assert install.ensure_env_value(target, "B", "2") is True
    assert install.ensure_env_value(target, "B", "2") is False
    assert target.read_text() == "A=1\nB=2\n"
    with pytest.raises(RuntimeError, match="different installed value"):
        install.ensure_env_value(target, "B", "3")
