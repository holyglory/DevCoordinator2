import io
import json
import os
import subprocess
import threading
from pathlib import Path

import pytest

from devcoordinator2.client import cli
from devcoordinator2.client.mcp_server import McpServer
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.handlers import build_handlers
from devcoordinator2.daemon.plan_api import build_plan_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Server
from devcoordinator2.daemon.test_capacity import CapacityBroker
from devcoordinator2.paths import InstanceConfig


@pytest.fixture
def live(tmp_path: Path, monkeypatch):
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock",
        state_dir=tmp_path / "state",
        unit_prefix="devcoordinator2-dev",
        slice_name="devcoordinator2-tests.slice",
        client_group="",
    )
    monkeypatch.setenv("DEVCOORDINATOR2_SOCKET", str(config.socket_path))
    monkeypatch.setenv("DEVCOORDINATOR2_STATE_DIR", str(config.state_dir))
    monkeypatch.setenv("DEVCOORDINATOR2_INSTANCE_ENV", "/nonexistent")
    db = Database(config.database_path)
    registry = Registry(db)
    capacity = CapacityBroker(db, config.capacity_socket_path, logical_cpus=4)
    handlers = build_handlers(config, registry, capacity=capacity)
    handlers.update(build_plan_handlers(config, db, registry))
    server = Server(config.socket_path, handlers)
    server.bind()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    yield type("Live", (), {"config": config, "repo": repo, "capacity": capacity})
    server.shutdown()
    db.close()


def test_cli_ping_and_register_roundtrip(live, capsys):
    rc = cli.main(["ping"])
    assert rc == 0
    response = json.loads(capsys.readouterr().out)
    assert response["ok"] is True
    assert response["result"]["schema_version"] == 14

    rc = cli.main(["repository", "register", str(live.repo)])
    assert rc == 0
    reg = json.loads(capsys.readouterr().out)
    assert reg["result"]["registered"] is True

    rc = cli.main(["repository", "list"])
    assert rc == 0
    listing = json.loads(capsys.readouterr().out)
    assert len(listing["result"]["repositories"]) == 1


def test_cli_error_exit_code(live, capsys, tmp_path):
    rc = cli.main(["test", "status", str(tmp_path)])
    assert rc == 1
    response = json.loads(capsys.readouterr().out)
    assert response["ok"] is False


def test_deployment_list_without_path_builds_empty_args():
    ns = cli.build_parser().parse_args(["deployment", "list"])
    assert cli._to_call(ns) == ("deployment.list", {})


def test_repository_archive_cli_argument_mapping():
    archive = cli.build_parser().parse_args([
        "repository", "archive", "rsource", "--into", "rtarget",
        "--note", "Merged into target",
    ])
    assert cli._to_call(archive) == ("repository.archive", {
        "repository_id": "rsource",
        "merged_into_repository_id": "rtarget",
        "note": "Merged into target",
    })
    unarchive = cli.build_parser().parse_args([
        "repository", "unarchive", "rsource", "--note", "Rollback merge",
    ])
    assert cli._to_call(unarchive) == ("repository.unarchive", {
        "repository_id": "rsource",
        "note": "Rollback merge",
    })
    listing = cli.build_parser().parse_args(["repository", "list", "--all"])
    assert cli._to_call(listing) == ("repository.list", {"include_archived": True})


def test_governed_check_cli_argument_mapping(tmp_path):
    path = str(tmp_path.absolute())
    start = cli.build_parser().parse_args([
        "test", "start", path, "--test", "complete",
        "--check", "unit", "--check", "browser",
    ])
    assert cli._to_call(start) == ("test.start", {
        "path": path, "test": "complete", "checks": ["unit", "browser"],
        "tier": "release"})
    retry = cli.build_parser().parse_args([
        "test", "retry", path, "--test", "complete",
        "--run-id", "torigin", "--check", "unit",
    ])
    assert cli._to_call(retry) == ("test.retry", {
        "path": path, "test": "complete", "run_id": "torigin", "check": "unit"})
    catalog = cli.build_parser().parse_args([
        "test", "log", "catalog", path, "--run-id", "t20260902T010203Z-abcdef",
        "--check", "unit", "--phase", "case", "--case", "parser-17",
        "--stream", "stderr",
    ])
    assert cli._to_call(catalog) == ("test.log.catalog", {
        "path": path, "run_id": "t20260902T010203Z-abcdef", "check": "unit",
        "phase": "case", "case": "parser-17", "stream": "stderr", "limit": 100})
    tail = cli.build_parser().parse_args([
        "test", "log", "tail", path, "--check", "unit", "--phase", "check",
        "--stream", "stderr",
    ])
    assert cli._to_call(tail) == ("test.log.tail", {
        "path": path, "check": "unit", "phase": "check", "stream": "stderr",
        "lines": 50, "max_bytes": 32768})
    search = cli.build_parser().parse_args([
        "test", "log", "search", path, "--check", "unit", "--phase", "check",
        "--stream", "stdout", "--text", "[literal].*",
    ])
    assert cli._to_call(search)[0] == "test.log.search"
    exact_range = cli.build_parser().parse_args([
        "test", "log", "range", path, "--check", "unit", "--phase", "check",
        "--stream", "stdout", "--line-start", "40", "--line-end", "60",
    ])
    assert cli._to_call(exact_range)[1]["line_start"] == 40
    context = cli.build_parser().parse_args([
        "test", "log", "failure-context", path, "--check", "unit",
        "--phase", "check", "--stream", "stderr",
    ])
    assert cli._to_call(context)[0] == "test.log.failure_context"
    retention = cli.build_parser().parse_args([
        "test", "log", "retention", "set", "--max-age-seconds", "7200",
        "--case-depth", "5",
    ])
    assert cli._to_call(retention) == ("test.log.retention.set", {
        "max_age_seconds": 7200, "case_depth": 5})
    stop = cli.build_parser().parse_args([
        "test", "stop", path, "--reason", "operator cancelled upgrade",
    ])
    assert cli._to_call(stop) == ("test.stop", {
        "path": path, "reason": "operator cancelled upgrade"})

    development = cli.build_parser().parse_args([
        "test", "start", path, "--tier", "development"])
    assert cli._to_call(development) == ("test.start", {
        "path": path, "tier": "development"})
    show = cli.build_parser().parse_args(["test", "capacity", "show"])
    assert cli._to_call(show) == ("test.capacity.get", {})
    set_cap = cli.build_parser().parse_args(["test", "capacity", "set", "48"])
    assert cli._to_call(set_cap) == ("test.capacity.set", {"cap": 48})
    clear = cli.build_parser().parse_args(["test", "capacity", "clear"])
    assert cli._to_call(clear) == ("test.capacity.set", {"cap": None})


def test_capacity_cli_roundtrip(live, capsys):
    assert cli.main(["test", "capacity", "show"]) == 0
    shown = json.loads(capsys.readouterr().out)["result"]
    assert shown["learned_capacity"] == 8
    assert shown["effective_capacity"] == 8
    assert shown["cap"] is None
    assert cli.main(["test", "capacity", "set", "5"]) == 0
    changed = json.loads(capsys.readouterr().out)["result"]
    assert changed["effective_capacity"] == 5
    assert changed["last_adjustment"]["reason"] == "administrator_cap_changed"
    assert cli.main(["test", "capacity", "clear"]) == 0
    cleared = json.loads(capsys.readouterr().out)["result"]
    assert cleared["cap"] is None and cleared["effective_capacity"] == 8


def test_governed_check_event_writes_bound_identity(monkeypatch):
    read_fd, write_fd = os.pipe()
    monkeypatch.setenv("DEVCOORDINATOR_EVENT_FD", str(write_fd))
    monkeypatch.setenv("DEVCOORDINATOR_RUN_ID", "trun")
    monkeypatch.setenv("DEVCOORDINATOR_CHECK_NAME", "server")
    try:
        assert cli.main(["test", "event", "passed"]) == 0
        os.close(write_fd)
        assert json.loads(os.read(read_fd, 4096)) == {
            "schema": 2, "run_id": "trun", "check": "server", "status": "passed"}
    finally:
        os.close(read_fd)


def test_deployment_set_domain_argument_mapping():
    ns = cli.build_parser().parse_args(
        ["deployment", "set-domain", "--deployment-id", "d123", "--domain", "app",
         "--port", "8080", "--component", "web", "--public"])
    assert cli._to_call(ns) == ("deployment.set_domain", {
        "deployment_id": "d123", "domain": "app", "port": 8080,
        "component": "web", "public": True})
    ns = cli.build_parser().parse_args(
        ["deployment", "set-domain", "--deployment-id", "d123", "--clear"])
    assert cli._to_call(ns) == ("deployment.set_domain",
                                {"deployment_id": "d123", "domain": None})
    with pytest.raises(SystemExit):
        cli._to_call(cli.build_parser().parse_args(
            ["deployment", "set-domain", "--deployment-id", "d123"]))
    with pytest.raises(SystemExit):
        cli._to_call(cli.build_parser().parse_args(
            ["deployment", "set-domain", "--deployment-id", "d123",
             "--domain", "app", "--clear"]))


def test_cli_daemon_unavailable(live, capsys, monkeypatch):
    monkeypatch.setenv("DEVCOORDINATOR2_SOCKET", "/nonexistent/sock")
    rc = cli.main(["ping"])
    assert rc == 2
    response = json.loads(capsys.readouterr().out)
    assert response["error"]["code"] == "daemon_unavailable"


def _mcp_session(messages: list[dict]) -> list[dict]:
    stdin = io.StringIO("".join(json.dumps(m) + "\n" for m in messages))
    stdout = io.StringIO()
    McpServer(stdin=stdin, stdout=stdout).run()
    return [json.loads(line) for line in stdout.getvalue().splitlines()]


def test_mcp_full_session(live):
    replies = _mcp_session([
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-06-18",
                    "clientInfo": {"name": "codex-cli", "version": "1"},
                    "capabilities": {}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
         "params": {"name": "repository_list", "arguments": {}}},
        {"jsonrpc": "2.0", "id": 4, "method": "tools/call",
         "params": {"name": "nope", "arguments": {}}},
        {"jsonrpc": "2.0", "id": 5, "method": "bogus/method"},
    ])
    init = replies[0]["result"]
    assert init["protocolVersion"] == "2025-06-18"
    assert init["serverInfo"]["name"] == "devcoordinator2"
    tools = {t["name"] for t in replies[1]["result"]["tools"]}
    assert {"test_start", "test_retry", "test_status", "test_log_catalog",
            "test_log_tail", "test_log_search", "test_log_range",
            "test_log_failure_context", "test_log_retention_show",
            "test_log_retention_set", "test_stop",
            "test_list", "test_capacity_show", "test_capacity_set",
            "test_capacity_clear",
            "repository_list", "repository_archive", "repository_unarchive",
            "deployment_apply", "deployment_status", "deployment_stop", "deployment_logs",
            "health_containers"} <= tools
    call_result = replies[2]["result"]
    assert call_result["isError"] is False
    inner = json.loads(call_result["content"][0]["text"])
    assert inner["ok"] is True
    assert replies[3]["error"]["code"] == -32602
    assert replies[4]["error"]["code"] == -32601


def test_cli_plan_ledger_roundtrip(live, capsys):
    def run(argv):
        rc = cli.main(argv)
        out = json.loads(capsys.readouterr().out)
        return rc, out

    rc, created = run(["task", "create", str(live.repo), "--title",
                       "Painting the button red", "--kind", "user_feedback",
                       "--estimated-loc", "10"])
    assert rc == 0 and created["result"]["status"] == "planned"
    task_id = created["result"]["task_id"]
    rc, requested = run(["task", "update", task_id, "--elaboration-needed"])
    assert rc == 0 and requested["result"]["elaboration_needed"] is True
    rc, rejected = run(["task", "update", task_id, "--elaboration-complete"])
    assert rc == 1 and rejected["error"]["code"] == "args_invalid"
    rc, clarified = run([
        "task", "update", task_id, "--title", "Make the button red",
        "--outcome", "People can clearly see the red button.",
        "--elaboration-complete"])
    assert rc == 0 and clarified["result"]["elaboration_needed"] is False
    rc, overview = run(["plan", "overview", str(live.repo)])
    assert overview["result"]["tasks"][0]["title"] == "Make the button red"
    rc, done = run(["task", "update", task_id, "--status", "done",
                    "--note", "Looks red in the app now."])
    assert rc == 0 and done["result"]["status"] == "done"
    rc, release = run(["release", "create", str(live.repo), "--name",
                       "Polish release", "--kind", "release"])
    assert rc == 0
    rc, moved = run(["task", "update", task_id, "--release-id",
                     release["result"]["release_id"]])
    assert rc == 0 and moved["result"]["release_id"] == release["result"]["release_id"]
    rc, moved = run(["task", "update", task_id, "--backlog"])
    assert rc == 0 and moved["result"]["release_id"] is None
    rc, history = run(["task", "history", task_id])
    assert [e["event"] for e in history["result"]["events"]] == \
        ["created", "elaboration_requested", "edited", "elaboration_completed",
         "status", "release_move", "release_move"]
    # Owner controls stay reachable through the CLI recovery surface.
    rc, requested = run(["release", "request", str(live.repo),
                         "--note", "Show me the current state."])
    assert rc == 0 and requested["result"]["status"] == "requested"
    rc, renamed = run(["release", "update", "--release-id",
                       requested["result"]["release_id"], "--name",
                       "First look preview"])
    assert rc == 0 and renamed["result"]["name"] == "First look preview"
    rc, recorded = run(["decision", "record", str(live.repo), "--aspect", "ui",
                        "--title", "The button is red", "--body",
                        "The owner asked for a red button after testing the app.",
                        "--ref", "REPO-BUTTON-RED"])
    assert rc == 0 and recorded["result"]["seq"] == 1
    rc, found = run(["decision", "search", str(live.repo), "--query", "red button"])
    assert [d["ref"] for d in found["result"]["decisions"]] == ["REPO-BUTTON-RED"]
    rc, tail = run(["decision", "tail", str(live.repo), "--aspect", "ui", "-n", "5"])
    assert tail["result"]["decisions"][0]["title"] == "The button is red"
    rc, picker = run(["plan", "overview", "--all"])
    assert picker["result"]["repositories"][0]["open_tasks"] == 0


def test_mcp_plan_tools_present_owner_controls_absent(live):
    replies = _mcp_session([
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-06-18",
                    "clientInfo": {"name": "codex-cli", "version": "1"},
                    "capabilities": {}}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
         "params": {"name": "task_create",
                    "arguments": {"path": str(live.repo),
                                  "title": "Ledger the missing empty state",
                                  "kind": "stub", "estimated_loc": 30}}},
    ])
    listed = {t["name"]: t for t in replies[0 + 1]["result"]["tools"]}
    tools = set(listed)
    assert {"plan_overview", "task_create", "task_update", "task_history",
            "release_create", "release_deliver", "decision_record",
            "decision_tail", "decision_search", "decision_summarize"} <= tools
    # The owner's ASAP button and chart reshaping are not agent tools.
    assert "release_request" not in tools and "release_update" not in tools
    assert listed["task_update"]["inputSchema"]["properties"][
        "elaboration_needed"]["type"] == "boolean"
    capacity_schema = listed["test_capacity_set"]["inputSchema"]
    assert capacity_schema["required"] == ["cap"]
    assert "required" not in capacity_schema["properties"]
    created = json.loads(replies[2]["result"]["content"][0]["text"])
    assert created["ok"] is True and created["result"]["status"] == "planned"


def test_mcp_unsupported_version_negotiated_down(live):
    replies = _mcp_session([
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "1999-01-01",
                    "clientInfo": {"name": "x"}, "capabilities": {}}},
    ])
    assert replies[0]["result"]["protocolVersion"] == "2025-06-18"


def test_bug_report_works_without_daemon(tmp_path, monkeypatch, capsys):
    """REQ-REL-03: intake is independent of daemon, database, and edge."""
    monkeypatch.setenv("DEVCOORDINATOR2_SOCKET", "/nonexistent/daemon.sock")
    monkeypatch.setenv("DEVCOORDINATOR2_BUGS_DIR", str(tmp_path / "bugs"))
    monkeypatch.setenv("DEVCOORDINATOR2_INSTANCE_ENV", "/nonexistent")
    rc = cli.main(["bug", "report", "--component", "api", "--summary", "crash",
                   "--expected", "ok", "--actual", "500", "--steps", "GET /"])
    assert rc == 0
    out = json.loads(capsys.readouterr().out)
    assert out["ok"] and out["result"]["notified"] is False
    bug_id = out["result"]["bug_id"]
    assert (tmp_path / "bugs" / f"{bug_id}.json").exists()
    rc = cli.main(["bug", "list"])
    assert rc == 0 and len(json.loads(capsys.readouterr().out)["result"]["bugs"]) == 1
    rc = cli.main(["bug", "close", bug_id])
    assert rc == 0 and json.loads(capsys.readouterr().out)["result"]["closed"]
