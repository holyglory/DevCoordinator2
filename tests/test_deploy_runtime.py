import subprocess
from pathlib import Path

import pytest

from devcoordinator2.daemon import deploy_runtime as runtime


def _result(returncode=0, stdout="", stderr=""):
    return subprocess.CompletedProcess([], returncode, stdout, stderr)


def _states(monkeypatch, values):
    monkeypatch.setattr(runtime, "compose_container_ids", lambda _project: list(values))
    monkeypatch.setattr(runtime, "container_state", lambda container_id: values[container_id])


def test_compose_state_accepts_declared_successful_finite_service(monkeypatch):
    values = {
        "finite": {"state": "stopped", "status": "exited", "exit_code": 0,
                   "compose_service": "bootstrap", "image_id": "sha256:a",
                   "started_at": "start", "finished_at": "finish"},
        "api": {"state": "running", "status": "running", "exit_code": 0,
                "compose_service": "api"},
        "worker": {"state": "running", "status": "running", "exit_code": 0,
                   "compose_service": "worker"},
    }
    _states(monkeypatch, values)
    state = runtime.compose_state("project", ("bootstrap", "api", "worker"),
                                  ("bootstrap",))
    assert state["state"] == "running"
    assert state["running"] == 2
    assert state["completion_candidates"] == [{
        "service": "bootstrap", "container_id": "finite", **values["finite"]}]


def test_compose_state_does_not_hide_stopped_running_service(monkeypatch):
    values = {
        "api": {"state": "running", "status": "running", "exit_code": 0,
                "compose_service": "api"},
        "worker": {"state": "stopped", "status": "exited", "exit_code": 0,
                   "compose_service": "worker"},
    }
    _states(monkeypatch, values)
    state = runtime.compose_state("project", ("api", "worker"))
    assert state["state"] == "failed"


def test_compose_state_distinguishes_requested_stop_from_crash(monkeypatch):
    values = {
        "api": {"state": "running", "status": "running", "exit_code": 0,
                "compose_service": "api"},
        "worker": {"state": "failed", "status": "exited", "exit_code": 137,
                   "compose_service": "worker"},
    }
    _states(monkeypatch, values)
    state = runtime.compose_state(
        "project", ("api", "worker"), (), None, {"worker": "stopped"})
    assert state["state"] == "failed"  # one required service is deliberately absent
    services = {item["name"]: item for item in state["services"]}
    assert services["worker"]["state"] == "stopped"
    assert services["worker"]["desired_state"] == "stopped"


def test_compose_state_uses_receipt_when_finite_container_is_gone(monkeypatch):
    values = {
        "api": {"state": "running", "status": "running", "exit_code": 0,
                "compose_service": "api"},
    }
    _states(monkeypatch, values)
    state = runtime.compose_state(
        "project", ("bootstrap", "api"), ("bootstrap",),
        {"bootstrap": {"service": "bootstrap", "exit_code": 0}})
    assert state["state"] == "running"


def test_compose_state_reports_failed_finite_service(monkeypatch):
    values = {
        "finite": {"state": "failed", "status": "exited", "exit_code": 2,
                   "compose_service": "bootstrap"},
        "api": {"state": "running", "status": "running", "exit_code": 0,
                "compose_service": "api"},
    }
    _states(monkeypatch, values)
    state = runtime.compose_state("project", ("bootstrap", "api"), ("bootstrap",))
    assert state["state"] == "failed"
    assert state["completion_candidates"] == []


def test_compose_up_resets_finite_service_and_builds(monkeypatch, tmp_path):
    calls = []

    def compose(_project, _files, _cwd, _env_files, args, timeout=600):
        calls.append((args, timeout))
        if args == ["config", "--services"]:
            return _result(stdout="bootstrap\napi\n")
        return _result()

    monkeypatch.setattr(runtime, "_compose", compose)
    files = (tmp_path / "compose.yml",)
    runtime.compose_up("project", files, tmp_path, (), ("bootstrap", "api"),
                       ("bootstrap",), True)
    assert calls == [
        (["config", "--services"], 120),
        (["rm", "--stop", "--force", "bootstrap"], 120),
        (["up", "--detach", "--remove-orphans", "--build", "bootstrap", "api"], 1800),
    ]


def test_compose_up_failure_retains_bounded_stdout_and_stderr(monkeypatch, tmp_path):
    responses = iter([
        _result(stdout="bootstrap\napi\n"),
        _result(),
        _result(returncode=1, stdout="build command failed", stderr="solver detail"),
    ])
    monkeypatch.setattr(runtime, "_compose", lambda *_args, **_kwargs: next(responses))
    with pytest.raises(runtime.RuntimeError_, match="build command failed\nsolver detail"):
        runtime.compose_up(
            "project", (tmp_path / "compose.yml",), tmp_path, (),
            ("bootstrap", "api"), ("bootstrap",), True)


def test_compose_start_can_exclude_finite_service(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(
        runtime, "_compose",
        lambda project, files, cwd, env_files, args, timeout=600:
        calls.append(args) or _result())
    runtime.compose_start("project", (Path("compose.yml"),), tmp_path, (), ("api",))
    assert calls == [["start", "api"]]


def test_compose_exact_start_does_not_follow_finite_dependency(monkeypatch):
    values = {
        "finite": {"compose_service": "bootstrap"},
        "api": {"compose_service": "api"},
    }
    monkeypatch.setattr(runtime, "compose_container_ids", lambda _project: list(values))
    monkeypatch.setattr(runtime, "container_state", lambda container_id: values[container_id])
    started = []
    monkeypatch.setattr(runtime, "start_container", started.append)
    runtime.compose_start_exact_services("project", ("api",))
    assert started == ["api"]


def test_compose_exact_stop_affects_only_requested_service(monkeypatch):
    values = {
        "bootstrap": {"compose_service": "bootstrap"},
        "api": {"compose_service": "api"},
        "worker": {"compose_service": "worker"},
    }
    monkeypatch.setattr(runtime, "compose_container_ids", lambda _project: list(values))
    monkeypatch.setattr(runtime, "container_state", lambda container_id: values[container_id])
    stopped = []
    monkeypatch.setattr(runtime, "stop_container", stopped.append)
    runtime.compose_stop_exact_services("project", ("worker",))
    assert stopped == ["worker"]


def test_compose_service_ready_requires_running_health(monkeypatch):
    monkeypatch.setattr(
        runtime, "compose_service_container_ids",
        lambda _project, _services: {"worker": ["worker-id"]})
    monkeypatch.setattr(
        runtime, "container_state",
        lambda _container: {"state": "running", "status": "running"})
    assert runtime.compose_service_ready("project", "worker", 10) == (
        True, "worker running")


def test_compose_service_ready_fails_closed_on_terminal_container(monkeypatch):
    monkeypatch.setattr(
        runtime, "compose_service_container_ids",
        lambda _project, _services: {"worker": ["worker-id"]})
    monkeypatch.setattr(
        runtime, "container_state",
        lambda _container: {"state": "failed", "status": "exited"})
    assert runtime.compose_service_ready("project", "worker", 10) == (
        False, "worker became terminal: exited")
