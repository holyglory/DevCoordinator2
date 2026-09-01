from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from devcoordinator2.daemon import test_admission
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
    with pytest.raises(RuntimeError, match="immutable release already exists"):
        install.install_release("test-release")


def test_install_release_rejects_path_escape_identifier(tmp_path, monkeypatch):
    monkeypatch.setattr(install, "OPT", tmp_path / "opt")
    with pytest.raises(ValueError, match="path-safe"):
        install.install_release("../escape", activate=False)


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


def test_release_can_stage_without_switch_and_activate_atomically(tmp_path, monkeypatch):
    root = tmp_path / "source"
    (root / "console").mkdir(parents=True)
    (root / "console" / "app.js").write_text("new")
    opt = tmp_path / "opt"
    monkeypatch.setattr(install, "ROOT", root)
    monkeypatch.setattr(install, "OPT", opt)
    monkeypatch.setattr(install, "RELEASE_ITEMS", ("console",))

    staged = install.install_release("next", activate=False)
    assert staged.exists()
    assert not (opt / "current").exists()
    assert install.activate_release(staged) is None
    assert (opt / "current").resolve() == staged


def test_activation_guard_rolls_back_on_parent_failure_and_commits_on_success(
        tmp_path, monkeypatch):
    opt = tmp_path / "opt"
    old = opt / "releases" / "old"
    new = opt / "releases" / "new"
    old.mkdir(parents=True)
    new.mkdir()
    (opt / "current").symlink_to(old)
    monkeypatch.setattr(install, "OPT", opt)

    rollback_guard = install.start_activation_guard(
        old, new, restart_services=False)
    install.activate_release(new)
    install.finish_activation_guard(rollback_guard, commit=False)
    assert (opt / "current").resolve() == old

    commit_guard = install.start_activation_guard(
        old, new, restart_services=False)
    install.activate_release(new)
    install.finish_activation_guard(commit_guard, commit=True)
    assert (opt / "current").resolve() == new


def test_new_daemon_drain_uses_activity_receipt_without_fencing_socket(
        tmp_path, monkeypatch):
    runtime = tmp_path / "run"
    runtime.mkdir()
    socket_path = runtime / "daemon.sock"
    socket_path.write_text("live")
    admission = test_admission.TestAdmission(runtime)
    admission.reset()
    monkeypatch.setattr(
        install, "_coordinator_runtime", lambda: (test_admission, None))

    with install.drain_active_tests(
            socket_path=socket_path, runtime_dir=runtime,
            unit_prefix="tests", daemon_running=True):
        assert socket_path.exists()
        assert (runtime / test_admission.DRAIN_FILE).exists()
        with pytest.raises(test_admission.TestsDraining):
            with admission.start_guard():
                pass
    assert not (runtime / test_admission.DRAIN_FILE).exists()


def test_first_upgrade_fences_old_socket_until_legacy_tests_finish(
        tmp_path, monkeypatch):
    runtime = tmp_path / "run"
    runtime.mkdir()
    socket_path = runtime / "daemon.sock"
    socket_path.write_text("old-socket")
    waited = []
    monkeypatch.setattr(
        install, "_coordinator_runtime", lambda: (test_admission, None))
    monkeypatch.setattr(install, "_wait_legacy_tests", lambda prefix: waited.append(prefix))

    with install.drain_active_tests(
            socket_path=socket_path, runtime_dir=runtime,
            unit_prefix="legacy-tests", daemon_running=True):
        assert not socket_path.exists()
        assert (runtime / "daemon.pre-drain.sock").read_text() == "old-socket"
        socket_path.write_text("new-socket")
    assert waited == ["legacy-tests"]
    assert socket_path.read_text() == "new-socket"
    assert not (runtime / "daemon.pre-drain.sock").exists()


def test_aborted_first_upgrade_restores_old_socket(tmp_path, monkeypatch):
    runtime = tmp_path / "run"
    runtime.mkdir()
    socket_path = runtime / "daemon.sock"
    socket_path.write_text("old-socket")
    monkeypatch.setattr(
        install, "_coordinator_runtime", lambda: (test_admission, None))
    monkeypatch.setattr(install, "_wait_legacy_tests", lambda _prefix: None)

    with pytest.raises(RuntimeError, match="install failed"):
        with install.drain_active_tests(
                socket_path=socket_path, runtime_dir=runtime,
                unit_prefix="legacy-tests", daemon_running=True):
            raise RuntimeError("install failed")
    assert socket_path.read_text() == "old-socket"
    assert not (runtime / "daemon.pre-drain.sock").exists()


def test_legacy_drain_waits_for_terminal_atomic_summary(tmp_path):
    current = tmp_path / "repo" / ".devcoordinator" / "test" / "current"
    current.mkdir(parents=True)
    summary = current / "summary.json"
    summary.write_text(json.dumps({"status": "running"}))
    completed = threading.Event()

    def wait():
        install._wait_legacy_summaries([current], test_admission)
        completed.set()

    thread = threading.Thread(target=wait)
    thread.start()
    replacement = current / ".summary-new"
    replacement.write_text(json.dumps({"status": "passed"}))
    os.replace(replacement, summary)
    thread.join(10)
    assert completed.is_set()
