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
    assert resp["result"]["schema_version"] == 12


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


def test_repository_archive_filters_lists_and_preserves_history(running_server, tmp_path):
    def create_repo(name):
        root = tmp_path / name
        root.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        subprocess.run(["git", "config", "user.name", "fixture"], cwd=root, check=True)
        subprocess.run(
            ["git", "config", "user.email", "fixture@example.invalid"],
            cwd=root,
            check=True,
        )
        (root / "f.txt").write_text(name)
        subprocess.run(["git", "add", "f.txt"], cwd=root, check=True)
        subprocess.run(["git", "commit", "-q", "-m", name], cwd=root, check=True)
        return root

    source = create_repo("source")
    target = create_repo("target")
    source_id = _call(
        running_server.socket_path,
        _request("repository.register", {"path": str(source)}),
    )["result"]["repository_id"]
    target_id = _call(
        running_server.socket_path,
        _request("repository.register", {"path": str(target)}),
    )["result"]["repository_id"]

    archived = _call(
        running_server.socket_path,
        _request(
            "repository.archive",
            {
                "repository_id": source_id,
                "merged_into_repository_id": target_id,
                "note": "Merged into target",
            },
        ),
    )
    assert archived["ok"] and archived["result"]["archived_at"]
    active = _call(running_server.socket_path, _request("repository.list"))
    assert [row["repository_id"] for row in active["result"]["repositories"]] == [
        target_id
    ]
    all_rows = _call(
        running_server.socket_path,
        _request("repository.list", {"include_archived": True}),
    )
    assert {row["repository_id"] for row in all_rows["result"]["repositories"]} == {
        source_id,
        target_id,
    }
    refused = _call(
        running_server.socket_path,
        _request("repository.register", {"path": str(source)}),
    )
    assert refused["error"]["code"] == "repository_archived"

    restored = _call(
        running_server.socket_path,
        _request(
            "repository.unarchive",
            {"repository_id": source_id, "note": "Restore for rollback"},
        ),
    )
    assert restored["ok"] and restored["result"]["archived_at"] is None
    db = Database(running_server.database_path)
    assert [row["event"] for row in db.query(
        "SELECT event FROM repository_events WHERE repository_id=? ORDER BY event_id",
        (source_id,),
    )] == ["archived", "unarchived"]
    db.close()


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


def test_socket_world_connectable(running_server):
    """DC2-2026-08-24-OPEN-LOCAL-ACCESS: mode 0666 so any local account —
    including sandboxed clients whose namespaces drop the client group —
    connects without ACL setup."""
    assert os.stat(running_server.socket_path).st_mode & 0o777 == 0o666
