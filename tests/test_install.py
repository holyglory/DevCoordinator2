from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace

import pytest

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
