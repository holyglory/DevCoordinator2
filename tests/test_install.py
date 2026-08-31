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


def test_install_release_excludes_local_design_references(tmp_path, monkeypatch):
    root = tmp_path / "source"
    console = root / "console"
    references = console / "design-reference"
    references.mkdir(parents=True)
    (console / "app.js").write_text("console")
    (references / "private-mock.png").write_bytes(b"private")
    opt = tmp_path / "opt"
    monkeypatch.setattr(install, "ROOT", root)
    monkeypatch.setattr(install, "OPT", opt)
    monkeypatch.setattr(install, "RELEASE_ITEMS", ("console",))

    release = install.install_release("test-release")

    assert (release / "console" / "app.js").read_text() == "console"
    assert not (release / "console" / "design-reference").exists()


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


def test_unchanged_compose_allowlist_restores_private_owner_and_mode(
    tmp_path, monkeypatch,
):
    allowlist = tmp_path / "allowlist.json"
    entry = {"repository_id": "r" + "a" * 16, "path": "a.env"}
    allowlist.write_text(json.dumps({"schema": 1, "authorizations": [entry]}))
    os.chmod(allowlist, 0o660)
    chowns = []
    monkeypatch.setattr(
        install.os, "chown", lambda path, uid, gid: chowns.append((path, uid, gid)))

    changed = install.merge_compose_env_allowlist(allowlist, [], (0, 0))

    assert changed is True
    assert chowns == [(allowlist, 0, 0)]
    assert allowlist.stat().st_mode & 0o777 == 0o640


def test_codex_usage_source_policy_is_private_and_preserves_accounts(
    tmp_path, monkeypatch,
):
    policy = tmp_path / "codex-usage-sources.json"
    policy.write_text(json.dumps({
        "schema": 1,
        "sources": [{"uid": 1000, "codex_home": "/home/one/.codex",
                     "executable": "/home/one/.local/bin/codex"}],
    }))
    os.chmod(policy, 0o640)
    monkeypatch.setattr(install.os, "chown", lambda *_args: None)

    changed = install.merge_codex_usage_sources(
        policy,
        [{"uid": 1001, "codex_home": "/home/two/.codex",
          "executable": "/home/two/.local/bin/codex"}],
        (0, 0),
    )

    assert changed is True
    assert json.loads(policy.read_text())["sources"] == [
        {"uid": 1000, "codex_home": "/home/one/.codex",
         "executable": "/home/one/.local/bin/codex"},
        {"uid": 1001, "codex_home": "/home/two/.codex",
         "executable": "/home/two/.local/bin/codex"},
    ]
    assert policy.stat().st_mode & 0o777 == 0o600


def test_codex_usage_account_resolves_default_private_collector(tmp_path, monkeypatch):
    home = tmp_path / "home"
    (home / ".codex").mkdir(parents=True)
    executable = home / ".local" / "bin" / "codex"
    executable.parent.mkdir(parents=True)
    executable.write_text("binary")
    monkeypatch.setattr(install.pwd, "getpwnam", lambda _name: SimpleNamespace(
        pw_dir=str(home), pw_uid=1234, pw_gid=1234))

    assert install.codex_usage_sources(["developer"]) == [{
        "uid": 1234,
        "codex_home": str(home / ".codex"),
        "executable": str(executable),
    }]


def test_ensure_env_value_appends_once_and_refuses_conflict(tmp_path):
    target = tmp_path / "instance.env"
    target.write_text("A=1\n")
    os.chmod(target, 0o640)
    assert install.ensure_env_value(target, "B", "2") is True
    assert install.ensure_env_value(target, "B", "2") is False
    assert target.read_text() == "A=1\nB=2\n"
    with pytest.raises(RuntimeError, match="different installed value"):
        install.ensure_env_value(target, "B", "3")


def test_install_restart_path_activates_the_new_release(monkeypatch):
    calls = []
    monkeypatch.setattr(
        install, "run",
        lambda argv, check=True: calls.append((argv, check)) or SimpleNamespace())
    install.enable_and_restart_units()
    assert calls == [
        (["systemctl", "enable", "devcoordinator2.service"], True),
        (["systemctl", "restart", "devcoordinator2.service"], True),
        (["systemctl", "enable", "devcoordinator2-edge.service"], True),
        (["systemctl", "restart", "devcoordinator2-edge.service"], True),
    ]
