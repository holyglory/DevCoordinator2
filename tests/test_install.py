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


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def make_live_checkout(tmp_path: Path) -> tuple[Path, Path]:
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "--bare", "-q", str(remote)], check=True)
    root = tmp_path / "live"
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    git(root, "config", "user.name", "fixture")
    git(root, "config", "user.email", "fixture@example.invalid")
    (root / "tracked.txt").write_text("one\n")
    git(root, "add", "tracked.txt")
    git(root, "commit", "-q", "-m", "initial")
    git(root, "branch", "-M", "main")
    git(root, "remote", "add", "origin", str(remote))
    git(root, "push", "-q", "-u", "origin", "main")
    subprocess.run(
        ["git", "-C", str(remote), "symbolic-ref", "HEAD", "refs/heads/main"],
        check=True,
    )
    return root, remote


def test_live_checkout_requires_clean_current_main(tmp_path):
    root, remote = make_live_checkout(tmp_path)
    head = git(root, "rev-parse", "HEAD")
    assert install.validate_live_checkout(root, fetch=True) == head

    (root / "dirty.txt").write_text("dirty\n")
    with pytest.raises(RuntimeError, match="must be clean"):
        install.validate_live_checkout(root, fetch=False)
    (root / "dirty.txt").unlink()

    git(root, "checkout", "-q", "-b", "feature")
    with pytest.raises(RuntimeError, match="must be on main"):
        install.validate_live_checkout(root, fetch=False)
    git(root, "checkout", "-q", "main")

    other = tmp_path / "other"
    subprocess.run(["git", "clone", "-q", str(remote), str(other)], check=True)
    git(other, "config", "user.name", "fixture")
    git(other, "config", "user.email", "fixture@example.invalid")
    (other / "tracked.txt").write_text("two\n")
    git(other, "add", "tracked.txt")
    git(other, "commit", "-q", "-m", "advance")
    git(other, "push", "-q", "origin", "main")
    with pytest.raises(RuntimeError, match="must exactly match fetched origin/main"):
        install.validate_live_checkout(root, fetch=True)


def test_install_skill_links_replaces_only_existing_agent_roots(tmp_path, monkeypatch):
    home = tmp_path / "home"
    codex = home / ".codex"
    claude = home / ".claude"
    codex.mkdir(parents=True)
    claude.mkdir()
    old = tmp_path / "releases" / "old" / "skills" / "codex-dev-coordinator"
    old.mkdir(parents=True)
    (codex / "skills").mkdir()
    (codex / "skills" / "codex-dev-coordinator").symlink_to(old)
    skills_root = tmp_path / "source" / "skills"
    for skill in install.MANAGED_SKILLS:
        (skills_root / skill).mkdir(parents=True)
    monkeypatch.setattr(install.pwd, "getpwnam", lambda _name: SimpleNamespace(
        pw_dir=str(home), pw_uid=1000, pw_gid=1000))
    monkeypatch.setattr(install.os, "chown", lambda *_args: None)

    links, retired = install.install_skill_links(["developer"], skills_root)

    expected = {
        str(root / "skills" / skill)
        for root in (codex, claude)
        for skill in install.MANAGED_SKILLS
    }
    assert set(links) == expected
    assert retired == [str(codex / "skills" / "codex-dev-coordinator")]
    for link in expected:
        assert Path(link).is_symlink()
        assert Path(link).readlink() == skills_root / Path(link).name


def test_install_skill_links_refuses_non_symlink(tmp_path, monkeypatch):
    home = tmp_path / "home"
    skill = home / ".codex" / "skills" / "dev-coordinator"
    skill.mkdir(parents=True)
    skills_root = tmp_path / "source" / "skills"
    for name in install.MANAGED_SKILLS:
        (skills_root / name).mkdir(parents=True)
    monkeypatch.setattr(install.pwd, "getpwnam", lambda _name: SimpleNamespace(
        pw_dir=str(home), pw_uid=1000, pw_gid=1000))

    with pytest.raises(RuntimeError, match="refusing to replace non-symlink"):
        install.install_skill_links(["developer"], skills_root)


def test_install_policy_links_use_universal_source(tmp_path, monkeypatch):
    home = tmp_path / "home"
    (home / ".codex").mkdir(parents=True)
    (home / ".claude").mkdir()
    policy = tmp_path / "source" / "reference" / "universal" / "AGENTS.md"
    policy.parent.mkdir(parents=True)
    policy.write_text("# Universal\n")
    monkeypatch.setattr(
        install.pwd,
        "getpwnam",
        lambda _name: SimpleNamespace(pw_dir=str(home), pw_uid=1000, pw_gid=1000),
    )

    links = install.install_policy_links(["developer"], policy)

    assert set(links) == {
        str(home / ".codex" / "AGENTS.md"),
        str(home / ".claude" / "CLAUDE.md"),
    }
    assert (home / ".codex" / "AGENTS.md").readlink() == policy
    assert (home / ".claude" / "CLAUDE.md").readlink() == policy


def test_compose_env_authorization_binds_ignored_path_to_repository(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    (repo / ".gitignore").write_text("private/dev.env\n")
    (repo / "private").mkdir()
    (repo / "private" / "dev.env").write_text(
        "PASSWORD=not-read-by-installer\n")  # public-artifact-guard: allow text-secret

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


def test_install_restart_path_restarts_live_checkout_services(monkeypatch):
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


def test_unit_files_and_cli_source_resolve_the_checkout(tmp_path):
    root = tmp_path / "source"
    deploy = root / "deploy"
    deploy.mkdir(parents=True)
    (deploy / "devcoordinator2.service").write_text(
        "Environment=PYTHONPATH=/home/DevCoordinator2/src\n"
    )
    (deploy / "devcoordinator2-edge.service").write_text(
        "ExecStart=/usr/bin/node /home/DevCoordinator2/edge/devcoordinator2-edge.mjs\n"
    )

    assert f"PYTHONPATH={root}/src" in install.unit_daemon(root)
    assert f"{root}/edge/devcoordinator2-edge.mjs" in install.unit_edge(root, False)


def test_set_env_value_replaces_live_source_path(tmp_path):
    target = tmp_path / "edge.env"
    target.write_text("EDGE_CONSOLE_DIR=/opt/devcoordinator2/current/console\n")
    assert install.set_env_value(target, "EDGE_CONSOLE_DIR", "/home/DevCoordinator2/console")
    assert target.read_text() == "EDGE_CONSOLE_DIR=/home/DevCoordinator2/console\n"
    assert not install.set_env_value(
        target, "EDGE_CONSOLE_DIR", "/home/DevCoordinator2/console")


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
