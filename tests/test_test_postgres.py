from __future__ import annotations

import subprocess

import pytest

from devcoordinator2.daemon import docker_cli, test_postgres
from devcoordinator2.daemon.repoconfig import PostgresSpec


def follower(lines: list[str]) -> subprocess.Popen:
    code = "import sys;sys.stderr.write(" + repr("".join(f"{line}\n" for line in lines)) \
        + ");sys.stderr.flush()"
    return subprocess.Popen(
        ["/usr/bin/python3", "-c", code], stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def test_postgres_readiness_uses_exact_log_events_then_one_tcp_verification(monkeypatch):
    ready = "database system is ready to accept connections"
    process = follower([ready, "temporary server stopped", ready])
    calls = []
    monkeypatch.setattr(docker_cli, "follow_logs", lambda _container: process)
    monkeypatch.setattr(
        docker_cli, "exec_ok",
        lambda container, argv: calls.append((container, argv)) or True)

    test_postgres._wait_ready("c" * 64, PostgresSpec(
        image="postgres:16-alpine", database="app", user="app"))

    assert len(calls) == 1
    assert calls[0][0] == "c" * 64
    assert calls[0][1][:3] == ["pg_isready", "-h", "127.0.0.1"]


def test_postgres_missing_second_readiness_event_fails(monkeypatch):
    process = follower(["database system is ready to accept connections"])
    monkeypatch.setattr(docker_cli, "follow_logs", lambda _container: process)
    monkeypatch.setattr(docker_cli, "exec_ok", lambda *_args: True)

    with pytest.raises(docker_cli.DockerError, match="ended before readiness"):
        test_postgres._wait_ready("c" * 64, PostgresSpec(
            image="postgres:16-alpine", database="app", user="app"))
