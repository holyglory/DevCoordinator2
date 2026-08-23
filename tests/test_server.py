import json
import os
import socket
import subprocess
import threading
from pathlib import Path

import pytest

from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.handlers import build_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Server
from devcoordinator2.paths import InstanceConfig


@pytest.fixture
def running_server(tmp_path: Path):
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock",
        state_dir=tmp_path / "state",
        unit_prefix="devcoordinator2-dev",
        slice_name="devcoordinator2-tests.slice",
        client_group="",
    )
    db = Database(config.database_path)
    registry = Registry(db)
    server = Server(config.socket_path, build_handlers(config, registry))
    server.bind()
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield config
    server.shutdown()
    db.close()


def _call(sock_path: Path, payload) -> dict:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.connect(str(sock_path))
        raw = payload if isinstance(payload, bytes) else (
            json.dumps(payload) + "\n").encode()
        s.sendall(raw)
        s.shutdown(socket.SHUT_WR)
        chunks = b""
        while True:
            part = s.recv(65536)
            if not part:
                break
            chunks += part
        return json.loads(chunks)


def _request(command: str, args=None, **extra) -> dict:
    return {"protocol": 1, "id": "req-1", "command": command,
            "args": args or {}, **extra}


def test_ping(running_server):
    resp = _call(running_server.socket_path, _request("ping"))
    assert resp["ok"] is True
    assert resp["id"] == "req-1"
    assert resp["result"]["schema_version"] == 4


def test_malformed_json(running_server):
    resp = _call(running_server.socket_path, b"this is not json\n")
    assert resp["ok"] is False
    assert resp["error"]["code"] == "protocol_invalid"


def test_oversize_request(running_server):
    resp = _call(running_server.socket_path, b"x" * 70000 + b"\n")
    assert resp["error"]["code"] == "request_too_large"


def test_unknown_command(running_server):
    resp = _call(running_server.socket_path, _request("nonsense.command"))
    assert resp["error"]["code"] == "command_unknown"


def test_unknown_args_rejected(running_server):
    resp = _call(running_server.socket_path,
                 _request("ping", {"surprise": True}))
    assert resp["error"]["code"] == "args_invalid"


def test_repository_not_found(running_server, tmp_path):
    resp = _call(running_server.socket_path,
                 _request("repository.register", {"path": str(tmp_path)}))
    assert resp["error"]["code"] == "repository_not_found"


def test_peercred_recorded_and_body_identity_ignored(running_server, tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    # A body attempting to assert identity is rejected as an unknown field.
    bad = _request("repository.register", {"path": str(repo)})
    bad["uid"] = 0
    resp = _call(running_server.socket_path, bad)
    assert resp["error"]["code"] == "protocol_invalid"

    resp = _call(running_server.socket_path,
                 _request("repository.register", {"path": str(repo)}))
    assert resp["ok"] is True
    # The recorded registrant uid must be the kernel peer uid of this process.
    db = Database(running_server.database_path)
    rows = db.query("SELECT registered_by_uid FROM repositories")
    db.close()
    assert rows[0]["registered_by_uid"] == os.getuid()


def test_concurrent_connections(running_server):
    results = []

    def call(i):
        results.append(_call(running_server.socket_path, _request("ping")))

    threads = [threading.Thread(target=call, args=(i,)) for i in range(16)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert len(results) == 16
    assert all(r["ok"] for r in results)
