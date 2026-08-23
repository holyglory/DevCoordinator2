import json
import socket
from pathlib import Path

import pytest

from devcoordinator2.daemon import ports, routes
from devcoordinator2.daemon.db import Database


@pytest.fixture
def db(tmp_path: Path):
    database = Database(tmp_path / "db.sqlite3")
    with database.transaction() as conn:
        conn.execute("INSERT INTO repositories VALUES('r1','/x','x','t',1,'t')")
        conn.execute("INSERT INTO worktrees VALUES('w1','r1','/x','t','t')")
        conn.execute(
            "INSERT INTO deployments(deployment_id, repository_id, worktree_id, name, source,"
            " domain, spec_fingerprint, spec_json, state, created_at, created_by_uid, client,"
            " updated_at) VALUES('d1','r1','w1','web','worktree','app','f','{}','running',"
            "'t',1,'other','t')")
    yield database
    database.close()


def test_lease_is_unique_and_skips_bound_ports(db):
    low, high = 40000, 40010
    held = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    held.bind(("127.0.0.1", low))
    try:
        first = ports.lease(db, (low, high), "d1", "api", 1)
        second = ports.lease(db, (low, high), "d1", "worker", 1)
        assert first == low + 1  # low is bound on the host
        assert second == low + 2
        assert ports.assigned(db, "d1", 1) == {"api": first, "worker": second}
        ports.release(db, "d1", generation=1)
        assert ports.assigned(db, "d1", 1) == {}
    finally:
        held.close()


def test_lease_exhaustion(db):
    for _ in range(3):
        ports.lease(db, (40020, 40022), "d1", "c", 1)
    with pytest.raises(ports.PortExhausted):
        ports.lease(db, (40020, 40022), "d1", "c", 1)


def test_route_document_is_complete_checksummed_and_monotonic(db, tmp_path):
    with db.transaction() as conn:
        conn.execute("INSERT INTO domain_routes VALUES('app','d1','api',20001,1,'t')")
        conn.execute("INSERT INTO domain_routes VALUES('idle','d1','api',NULL,1,'t')")
    path = tmp_path / "routes.json"
    first = routes.publish(db, path, "example.test")
    doc = json.loads(path.read_text())
    assert doc == first
    assert doc["schema"] == 1 and doc["generation"] == 1
    assert [r["domain"] for r in doc["routes"]] == ["app.example.test"]  # no port -> not routed
    assert doc["routes"][0]["port"] == 20001
    import hashlib
    payload = {k: v for k, v in doc.items() if k not in ("schema", "payload_sha256")}
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    assert doc["payload_sha256"] == hashlib.sha256(canonical).hexdigest()
    second = routes.publish(db, path, "example.test")
    assert second["generation"] == 2
    assert not list(tmp_path.glob(".routes-*"))  # atomic replace leaves no temp files
