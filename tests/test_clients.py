import io
import json
import subprocess
import threading
from pathlib import Path

import pytest

from devcoordinator2.client import cli
from devcoordinator2.client.mcp_server import McpServer
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.handlers import build_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Server
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
    server = Server(config.socket_path, build_handlers(config, Registry(db)))
    server.bind()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    yield type("Live", (), {"config": config, "repo": repo})
    server.shutdown()
    db.close()


def test_cli_ping_and_register_roundtrip(live, capsys):
    rc = cli.main(["ping"])
    assert rc == 0
    response = json.loads(capsys.readouterr().out)
    assert response["ok"] is True
    assert response["result"]["schema_version"] == 5

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
    assert {"test_start", "test_status", "test_output", "test_stop", "repository_list",
            "deployment_apply", "deployment_status", "deployment_stop", "deployment_logs",
            "health_containers"} <= tools
    call_result = replies[2]["result"]
    assert call_result["isError"] is False
    inner = json.loads(call_result["content"][0]["text"])
    assert inner["ok"] is True
    assert replies[3]["error"]["code"] == -32602
    assert replies[4]["error"]["code"] == -32601


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
