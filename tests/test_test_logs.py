from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.handlers import build_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller
from devcoordinator2.daemon.test_logs import TestLogService as _TestLogService
from devcoordinator2.daemon.test_logs import _run_bridge, validate_log_request
from devcoordinator2.daemon.tests_lifecycle import TestLifecycle as _TestLifecycle
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError


def _caller(*, identity=None) -> Caller:
    return Caller(pid=os.getpid(), uid=os.getuid(), gid=os.getgid(),
                  client_kind="edge" if identity else "codex",
                  client_session=None, identity=identity)


@pytest.fixture
def world(tmp_path: Path):
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock", state_dir=tmp_path / "state",
        unit_prefix="dc2-test", slice_name="dc2-tests.slice", client_group="",
    )
    db = Database(config.database_path)
    registry = Registry(db)
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    registration = registry.register(repo, os.getuid(), os.getgid())
    binary = tmp_path / "devcoordinator2-executor"
    binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    binary.chmod(0o755)
    service = _TestLogService(db, registry, binary)
    yield SimpleNamespace(config=config, db=db, registry=registry, repo=repo,
                          registration=registration, service=service)
    service.shutdown()
    db.close()


def test_schema14_retention_defaults_and_append_only_changes(world):
    assert world.service.retention()["max_age_seconds"] == 86_400
    assert world.service.retention()["case_depth"] == 3

    changed = world.service.set_retention(7_200, 5, "owner@example.test")
    assert changed["max_age_seconds"] == 7_200
    assert changed["case_depth"] == 5
    assert changed["cleanup_requested"] is True
    assert world.db.query(
        "SELECT COUNT(*) AS count FROM test_log_retention_events")[0]["count"] == 1

    world.service.set_retention(7_200, 5, "owner@example.test")
    assert world.db.query(
        "SELECT COUNT(*) AS count FROM test_log_retention_events")[0]["count"] == 1
    event = dict(world.db.query("SELECT * FROM test_log_retention_events")[0])
    assert (event["previous_max_age_seconds"], event["max_age_seconds"]) == (86_400, 7_200)
    assert (event["previous_case_depth"], event["case_depth"]) == (3, 5)


@pytest.mark.parametrize("args", [
    {"phase": "case", "check": "unit", "stream": "stderr"},
    {"phase": "case", "case": "a", "stream": "stderr"},
    {"phase": "executor", "check": "unit", "stream": "stdout"},
    {"phase": "check", "check": "unit", "case": "a", "stream": "stdout"},
])
def test_selector_relationships_are_strict(args):
    with pytest.raises(ProtocolError, match=r"requires|forbids|no check"):
        validate_log_request("tail", args)


def test_context_lines_rejects_boolean_values():
    with pytest.raises(ProtocolError, match="context_lines"):
        validate_log_request("failure_context", {"context_lines": False})


def test_query_defaults_and_literal_search_are_forwarded_as_typed_json(world, monkeypatch):
    captured = {}

    def fake_run(argv, payload):
        captured["argv"] = argv
        captured["request"] = json.loads(payload)
        result = {"schema": 2, "ok": True, "result": {
            "matches": [{"line_start": 8, "line_end": 8, "byte_start": 50,
                         "byte_end": 62, "text": "[literal].*"}],
            "next_cursor": None,
        }}
        return SimpleNamespace(returncode=0, stdout=json.dumps(result).encode(),
                               stderr_oversized=False)

    world.service._bridge_runner = fake_run
    result = world.service.query("search", world.repo, {
        "check": "unit", "phase": "check", "stream": "stderr",
        "text": "[literal].*",
    }, _caller(identity="owner@example.test"))
    assert result["matches"][0]["line_start"] == 8
    assert captured["argv"][1:] == [
        "log-query", "--worktree", str(world.repo), "--request", "-"]
    request = captured["request"]
    assert request["schema"] == 2 and request["operation"] == "search"
    assert request["repository_id"] == world.registration.repository_id
    assert request["selector"] == {
        "check": "unit", "phase": "check", "stream": "stderr"}
    assert request["options"]["text"] == "[literal].*"
    assert request["options"]["max_matches"] == 20


def test_bridge_never_relays_stderr_or_unknown_error_prose(world, monkeypatch):
    response = {"schema": 2, "ok": False,
                "error": {"code": "not-a-public-code", "message": "hostile raw text"}}
    world.service._bridge_runner = lambda *a, **k: SimpleNamespace(
        returncode=2, stdout=json.dumps(response).encode(),
        stderr_oversized=True)
    with pytest.raises(ProtocolError) as caught:
        world.service.query(
            "catalog", world.repo, {}, _caller(identity="owner@example.test"))
    assert caught.value.code == "test_log_unavailable"
    assert "hostile" not in caught.value.message
    assert "stack trace" not in caught.value.message


def test_public_identity_must_use_one_exact_registered_worktree(world, monkeypatch):
    world.service._bridge_runner = lambda *a, **k: SimpleNamespace(
        returncode=0, stdout=b'{"schema":2,"ok":true,"result":{"entries":[]}}',
        stderr_oversized=False)
    assert world.service.query(
        "catalog", world.repo, {}, _caller(identity="owner@example.test")) == {"entries": []}
    with pytest.raises(ProtocolError, match="registered worktree"):
        world.service.query(
            "catalog", world.repo / "subdirectory", {},
            _caller(identity="owner@example.test"))


def test_catalog_injects_the_persisted_retention_policy(world):
    captured = {}

    def fake_run(_argv, payload):
        captured.update(json.loads(payload))
        return SimpleNamespace(
            returncode=0,
            stdout=b'{"schema":2,"ok":true,"result":{"entries":[]}}',
            stderr_oversized=False,
        )

    world.service.set_retention(7_200, 5, "owner")
    world.service._bridge_runner = fake_run
    world.service.query(
        "catalog", world.repo, {}, _caller(identity="owner@example.test"))
    assert captured["options"]["max_age_seconds"] == 7_200
    assert captured["options"]["case_depth"] == 5


def test_maintenance_includes_archived_registered_worktrees(world):
    with world.db.transaction() as connection:
        connection.execute(
            "UPDATE repositories SET archived_at='2026-09-02T00:00:00Z'"
            " WHERE repository_id=?", (world.registration.repository_id,))
    calls = []

    def fake_run(argv, payload):
        calls.append((argv, json.loads(payload)))
        response = {"schema": 2, "ok": True, "result": {
            "removed_leaf_folders": 0, "retained_active": 0,
            "next_expiry_at": None,
        }}
        return SimpleNamespace(returncode=0, stdout=json.dumps(response).encode(),
                               stderr_oversized=False)

    world.service._bridge_runner = fake_run
    result = world.service.run_maintenance_once()
    assert result["errors"] == []
    assert len(calls) == 1
    assert calls[0][1]["repository_id"] == world.registration.repository_id


def test_bridge_drains_oversized_streams_without_buffering_them_all(tmp_path):
    program = tmp_path / "noisy_bridge.py"
    program.write_text(
        "import sys\nsys.stdin.buffer.read()\n"
        "sys.stdout.buffer.write(b'x' * 200000)\n"
        "sys.stderr.buffer.write(b'y' * 200000)\n",
        encoding="utf-8",
    )
    result = _run_bridge([sys.executable, str(program)], b"{}")
    assert len(result.stdout) == 65_537
    assert result.stderr_oversized is True


def test_handlers_replace_test_output_and_validate_retention(world):
    handlers = build_handlers(
        world.config, world.registry, test_logs=world.service)
    assert "test.output" not in handlers
    assert {"test.log.catalog", "test.log.tail", "test.log.search",
            "test.log.range", "test.log.failure_context",
            "test.log.retention.get", "test.log.retention.set"} <= set(handlers)
    with pytest.raises(ProtocolError, match="required"):
        handlers["test.log.retention.set"](
            {"max_age_seconds": 3600}, _caller())
    with pytest.raises(ProtocolError, match="phase and stream"):
        handlers["test.log.tail"]({"path": str(world.repo)}, _caller())


def test_public_status_projection_removes_paths_and_arbitrary_reasons():
    document = {
        "summary_path": "/private/repo/summary.json",
        "check_report_path": "/private/repo/check-report.json",
        "unsafe_reason": "arbitrary failure prose",
        "termination_reason": "operator wrote arbitrary text",
        "checks": [{"name": "unit", "reason": "traceback"}],
        "failure_index": [{"check": "unit", "case": "a", "reason": "assertion"}],
    }
    _TestLifecycle._sanitize_public_result(document)
    encoded = json.dumps(document)
    assert "/private" not in encoded
    assert "arbitrary" not in encoded
    assert "traceback" not in encoded
    assert "assertion" not in encoded
    assert document["failure_index"][0]["case"] == "a"


def test_failure_projection_is_structured_and_has_no_parallel_diagnostic_index():
    failure = {
        "check": "unit", "case": "parser-17", "status": "failed",
        "exit": {"code": 1, "signal": None}, "termination_reason": None,
        "source": {"file": "src/parser.rs", "line": 81, "column": 9},
        "error_category": "assertion", "expected": None, "actual": None,
        "fingerprint": "sha256:" + "a" * 64, "occurrences": 2,
        "log_refs": [{"run_id": "t20260902T010203Z-abcdef", "check": "unit",
                      "phase": "case", "case": "parser-17", "stream": "stderr"}],
        "origin": "junit", "reason": "must never escape",
    }
    projected = _TestLifecycle._report_projection({
        "checks": [], "failure_index": [failure], "failure_index_truncated": False,
        "counts": {}, "capacity": {},
    })
    assert projected["failure_index"][0]["fingerprint"] == failure["fingerprint"]
    assert projected["failure_index"][0]["origin"] == "junit"
    assert "reason" not in projected["failure_index"][0]
    assert "diagnostic_index" not in projected
