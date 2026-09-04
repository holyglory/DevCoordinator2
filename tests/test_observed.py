import json
from pathlib import Path

import pytest

from devcoordinator2.daemon import inventory, observed, routes
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.handlers import _deployment_handlers
from devcoordinator2.daemon.server import Caller
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError


@pytest.fixture
def db(tmp_path: Path):
    database = Database(tmp_path / "authority.sqlite3")
    with database.transaction() as conn:
        conn.execute(
            "INSERT INTO repositories(repository_id,root_path,display_name,"
            " registered_at,registered_by_uid,last_seen_at)"
            " VALUES('r1','/repo','repo','t',1000,'t')")
        conn.execute("INSERT INTO worktrees VALUES('w1','r1','/repo','t','t')")
    yield database
    database.close()


def _import(db: Database):
    deployment = {
        "deployment_id": "d1111111111111111", "repository_id": "r1",
        "name": "legacy-stack", "native_project": "legacy-stack",
        "state": "running", "health": "healthy", "evidence": {"exact": True},
    }
    container = {
        "container_id": "a" * 64, "deployment_id": deployment["deployment_id"],
        "repository_id": "r1", "name": "legacy-stack-app-1", "image": "app:1",
        "compose_service": "app", "status": "Up (healthy)", "health": "healthy",
    }
    route = {"domain": "app", "deployment_id": deployment["deployment_id"],
             "component": "app", "port": 5001, "public": True,
             "evidence": {"probe": "ok"}}
    observed.replace_current(db, [deployment], [container], [route], "2026-08-23T00:00:00Z")
    return deployment, container


def test_observed_projection_status_inventory_and_route(db, tmp_path, monkeypatch):
    deployment, container = _import(db)
    listed = observed.list_deployments(db)
    assert listed[0]["observed_only"] is True and listed[0]["route_port"] == 5001
    status = observed.status(db, deployment["deployment_id"])
    assert status["observed_only"] is True
    assert status["components"][0]["owned"] is False
    assert status["components"][0]["binding"]["identity"] == container["container_id"]

    monkeypatch.setattr(inventory, "_all_containers", lambda: [{
        "ID": container["container_id"], "Names": container["name"],
        "Image": container["image"], "State": "running", "Status": "Up (healthy)",
        "CreatedAt": "now", "Labels": "com.docker.compose.project=legacy-stack",
    }])
    rows = inventory.containers(db, "devcoordinator2")
    assert rows[0]["classification"] == "observed-current"
    assert rows[0]["repository_id"] == "r1"
    assert inventory.summary(rows)["observed-current"] == 1

    path = tmp_path / "routes.json"
    document = routes.publish(db, path, "example.test")
    assert document["routes"] == [{
        "deployment_id": deployment["deployment_id"], "component": "app",
        "label": "app", "domain": "app.example.test", "port": 5001,
        "scheme": "http", "auth": "public", "generation": None,
    }]
    assert json.loads(path.read_text())["routes"] == document["routes"]


class NeverCalled:
    def __getattr__(self, name):
        raise AssertionError(f"managed deployment method {name} must not be used")


def _handlers(db, tmp_path):
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock",
        state_dir=tmp_path / "state",
        unit_prefix="devcoordinator2-test",
        slice_name="devcoordinator2-tests.slice",
        client_group="",
    )
    return _deployment_handlers(config, NeverCalled(), db)


CALLER = Caller(pid=1, uid=1000, gid=1000, client_kind="human", client_session=None)


def test_observed_deployment_rejects_configuration_authority(db, tmp_path):
    deployment, _ = _import(db)
    handlers = _handlers(db, tmp_path)
    result = handlers["deployment.status"](
        {"deployment_id": deployment["deployment_id"]}, CALLER)
    assert result["observed_only"] is True
    for command in ("deployment.apply", "deployment.rollback", "deployment.remove"):
        with pytest.raises(ProtocolError) as exc:
            handlers[command]({"deployment_id": deployment["deployment_id"]}, CALLER)
        assert exc.value.code == "observed_only"


def test_observed_lifecycle_acts_on_exact_recorded_containers(db, tmp_path, monkeypatch):
    """DC2-2026-08-24-OBSERVED-LIFECYCLE: start/stop/restart drive docker on
    the exact recorded IDs; nothing is recreated and state is re-inspected."""
    deployment, container = _import(db)
    calls = []
    live = {"state": "running", "status": "running"}
    monkeypatch.setattr(observed.rt, "container_state", lambda cid: dict(live))
    monkeypatch.setattr(observed.rt, "restart_container",
                        lambda cid: calls.append(("restart", cid)))

    def stop(cid):
        calls.append(("stop", cid))
        live.update(state="stopped", status="exited (0)")
    monkeypatch.setattr(observed.rt, "stop_container", stop)
    monkeypatch.setattr(observed, "_container_health",
                        lambda cid, state: "healthy" if state == "running" else "none")

    handlers = _handlers(db, tmp_path)
    result = handlers["deployment.restart"](
        {"deployment_id": deployment["deployment_id"]}, CALLER)
    assert calls == [("restart", container["container_id"])]
    assert result["state"] == "running"

    result = handlers["deployment.stop"](
        {"deployment_id": deployment["deployment_id"], "component": "app"}, CALLER)
    assert calls[-1] == ("stop", container["container_id"])
    assert result["state"] == "stopped"
    assert result["components"][0]["state"] == "stopped"

    with pytest.raises(ProtocolError) as exc:
        handlers["deployment.start"](
            {"deployment_id": deployment["deployment_id"], "component": "nope"}, CALLER)
    assert exc.value.code == "args_invalid"


def test_observed_missing_container_reported_not_recreated(db, tmp_path, monkeypatch):
    deployment, _ = _import(db)
    monkeypatch.setattr(observed.rt, "container_state",
                        lambda cid: {"state": "missing", "status": "missing"})
    monkeypatch.setattr(observed.rt, "start_container",
                        lambda cid: pytest.fail("must not start a missing container"))
    monkeypatch.setattr(observed, "_container_health", lambda cid, state: "none")
    with pytest.raises(ProtocolError) as exc:
        observed.control(db, "start", deployment["deployment_id"], None)
    assert "no longer exists" in exc.value.message


def test_observed_logs_read_recorded_container(db, tmp_path, monkeypatch):
    deployment, container = _import(db)
    monkeypatch.setattr(observed.rt, "container_logs",
                        lambda cid, tail: f"logs of {cid[:8]} tail={tail}")
    handlers = _handlers(db, tmp_path)
    result = handlers["deployment.logs"](
        {"deployment_id": deployment["deployment_id"], "component": "app",
         "tail_lines": 50}, CALLER)
    assert result["observed_only"] is True
    assert f"logs of {container['container_id'][:8]} tail=50" in result["tail"]


def test_observed_set_domain_update_create_clear_and_conflict(db, tmp_path):
    deployment, _ = _import(db)
    dep_id = deployment["deployment_id"]
    handlers = _handlers(db, tmp_path)

    result = handlers["deployment.set_domain"](
        {"deployment_id": dep_id, "domain": "renamed"}, CALLER)
    assert result["domain"] == "renamed" and result["route_port"] == 5001

    result = handlers["deployment.set_domain"](
        {"deployment_id": dep_id, "domain": None}, CALLER)
    assert result["domain"] is None
    assert not db.query("SELECT 1 FROM observed_routes")

    with pytest.raises(ProtocolError) as exc:
        handlers["deployment.set_domain"](
            {"deployment_id": dep_id, "domain": "again"}, CALLER)
    assert "port" in exc.value.message

    result = handlers["deployment.set_domain"](
        {"deployment_id": dep_id, "domain": "again", "port": 8080, "public": True}, CALLER)
    assert result["domain"] == "again" and result["route_port"] == 8080

    with db.transaction() as conn:
        conn.execute("INSERT INTO deployments(deployment_id, repository_id, worktree_id,"
                     " name, source, domain, spec_fingerprint, spec_json, state,"
                     " created_at, created_by_uid, client, updated_at)"
                     " VALUES('d9999999999999999','r1','w1','web','worktree','taken',"
                     "'fp','{}','running','t',1000,'human','t')")
        conn.execute("INSERT INTO domain_routes VALUES('taken','d9999999999999999',"
                     "'web',20001,1,'t')")
    with pytest.raises(ProtocolError) as exc:
        handlers["deployment.set_domain"](
            {"deployment_id": dep_id, "domain": "taken"}, CALLER)
    assert "already routed" in exc.value.message

    with pytest.raises(ProtocolError) as exc:
        handlers["deployment.set_domain"](
            {"deployment_id": dep_id, "domain": "Bad_Label"}, CALLER)
    assert exc.value.code == "args_invalid"


def test_managed_domain_override_wins_until_cleared(db, tmp_path):
    from devcoordinator2.daemon import deploy_state
    spec_json = json.dumps({"domain": "declared",
                            "components": [{"name": "web", "route": True}]})
    with db.transaction() as conn:
        conn.execute("INSERT INTO deployments(deployment_id, repository_id, worktree_id,"
                     " name, source, domain, spec_fingerprint, spec_json, state,"
                     " current_generation, created_at, created_by_uid, client, updated_at)"
                     " VALUES('d8888888888888888','r1','w1','web','worktree','declared',"
                     "'fp',?,'running',3,'t',1000,'human','t')", (spec_json,))
        conn.execute("INSERT INTO domain_routes VALUES('declared','d8888888888888888',"
                     "'web',20001,3,'t')")
    handlers = _handlers(db, tmp_path)
    result = handlers["deployment.set_domain"](
        {"deployment_id": "d8888888888888888", "domain": "better"}, CALLER)
    assert result["domain"] == "better" and result["domain_source"] == "override"
    assert db.query("SELECT domain FROM domain_routes WHERE deployment_id="
                    "'d8888888888888888'")[0]["domain"] == "better"
    row = db.query("SELECT * FROM deployments WHERE deployment_id="
                   "'d8888888888888888'")[0]
    assert row["domain_override"] == "better" and row["domain"] == "better"

    class Spec:
        @staticmethod
        def domain_for(source):
            return "declared"
    assert deploy_state.effective_domain(dict(row), Spec, "worktree") == "better"

    result = handlers["deployment.set_domain"](
        {"deployment_id": "d8888888888888888", "domain": None}, CALLER)
    assert result["domain"] == "declared"
    assert result["domain_source"] == "configuration"
    assert db.query("SELECT domain FROM domain_routes WHERE deployment_id="
                    "'d8888888888888888'")[0]["domain"] == "declared"


def test_managed_set_domain_routes_via_implicit_single_port_component(db, tmp_path):
    """A deployment that never declared route = true but has exactly one
    port-leasing process/docker component gets its route created atomically."""
    spec_json = json.dumps({"domain": None, "components": [
        {"name": "app", "type": "process", "wants_port": True, "route": False},
        {"name": "worker", "type": "process", "wants_port": False, "route": False}]})
    with db.transaction() as conn:
        conn.execute("INSERT INTO deployments(deployment_id, repository_id, worktree_id,"
                     " name, source, domain, spec_fingerprint, spec_json, state,"
                     " current_generation, created_at, created_by_uid, client, updated_at)"
                     " VALUES('d7777777777777777','r1','w1','web','worktree',NULL,"
                     "'fp',?,'running',1,'t',1000,'human','t')", (spec_json,))
        conn.execute("INSERT INTO port_assignments VALUES(20000,'d7777777777777777',"
                     "'app',1,'t')")
    handlers = _handlers(db, tmp_path)
    result = handlers["deployment.set_domain"](
        {"deployment_id": "d7777777777777777", "domain": "para"}, CALLER)
    assert result["domain"] == "para"
    route = db.query("SELECT * FROM domain_routes WHERE deployment_id="
                     "'d7777777777777777'")[0]
    assert route["component"] == "app" and route["port"] == 20000
    assert route["domain"] == "para"


def test_managed_set_domain_without_routable_component_persists_nothing(db, tmp_path):
    spec_json = json.dumps({"domain": None, "components": [
        {"name": "db", "type": "postgres", "wants_port": True, "route": False}]})
    with db.transaction() as conn:
        conn.execute("INSERT INTO deployments(deployment_id, repository_id, worktree_id,"
                     " name, source, domain, spec_fingerprint, spec_json, state,"
                     " created_at, created_by_uid, client, updated_at)"
                     " VALUES('d6666666666666666','r1','w1','data','worktree',NULL,"
                     "'fp',?,'running','t',1000,'human','t')", (spec_json,))
    handlers = _handlers(db, tmp_path)
    with pytest.raises(ProtocolError) as exc:
        handlers["deployment.set_domain"](
            {"deployment_id": "d6666666666666666", "domain": "para"}, CALLER)
    assert "no routable component" in exc.value.message
    row = db.query("SELECT domain, domain_override FROM deployments"
                   " WHERE deployment_id='d6666666666666666'")[0]
    assert row["domain"] is None and row["domain_override"] is None
    assert not db.query("SELECT 1 FROM domain_routes WHERE deployment_id="
                        "'d6666666666666666'")


_V6_OBSERVED_DDL = """
CREATE TABLE observed_deployments (
  observed_deployment_id TEXT PRIMARY KEY,
  repository_id          TEXT NOT NULL REFERENCES repositories(repository_id),
  name                   TEXT NOT NULL,
  native_project         TEXT NOT NULL UNIQUE,
  state                  TEXT NOT NULL CHECK(state IN ('running', 'degraded')),
  health                 TEXT NOT NULL CHECK(health IN ('healthy', 'unhealthy', 'unknown')),
  source                 TEXT NOT NULL,
  evidence_json          TEXT NOT NULL,
  observed_at            TEXT NOT NULL,
  imported_at            TEXT NOT NULL,
  UNIQUE(repository_id, native_project)
);
CREATE TABLE observed_containers (
  container_id           TEXT PRIMARY KEY,
  observed_deployment_id TEXT NOT NULL
    REFERENCES observed_deployments(observed_deployment_id) ON DELETE CASCADE,
  repository_id          TEXT NOT NULL REFERENCES repositories(repository_id),
  name                   TEXT NOT NULL,
  image                  TEXT NOT NULL,
  compose_service        TEXT NOT NULL,
  state                  TEXT NOT NULL CHECK(state = 'running'),
  status                 TEXT NOT NULL,
  health                 TEXT NOT NULL
    CHECK(health IN ('healthy', 'unhealthy', 'starting', 'unknown')),
  observed_at            TEXT NOT NULL
);
CREATE INDEX observed_containers_deployment
  ON observed_containers(observed_deployment_id);
CREATE INDEX observed_containers_repository
  ON observed_containers(repository_id);
"""


def test_schema7_relaxes_observed_checks_preserving_rows(tmp_path):
    import sqlite3
    path = tmp_path / "authority.sqlite3"
    conn = sqlite3.connect(path)
    conn.executescript(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);"
        "INSERT INTO meta VALUES('schema_version','6');"
        "CREATE TABLE repositories (repository_id TEXT PRIMARY KEY, root_path TEXT NOT"
        " NULL UNIQUE, display_name TEXT NOT NULL, registered_at TEXT NOT NULL,"
        " registered_by_uid INTEGER NOT NULL, last_seen_at TEXT NOT NULL);"
        "INSERT INTO repositories(repository_id,root_path,display_name,"
        "registered_at,registered_by_uid,last_seen_at)"
        " VALUES('r1','/repo','repo','t',1000,'t');"
        + _V6_OBSERVED_DDL +
        "INSERT INTO observed_deployments VALUES('d1111111111111111','r1','s','s',"
        "'running','healthy','import','{}','t','t');"
        "INSERT INTO observed_containers VALUES('" + "a" * 64 + "','d1111111111111111',"
        "'r1','s-app-1','app:1','app','running','Up','healthy','t');")
    conn.commit()
    conn.close()
    db = Database(path)
    assert db.query("SELECT name FROM observed_deployments")[0]["name"] == "s"
    assert db.query("SELECT compose_service FROM observed_containers"
                    )[0]["compose_service"] == "app"
    with db.transaction() as conn2:
        conn2.execute("UPDATE observed_deployments SET state='stopped'")
        conn2.execute("UPDATE observed_containers SET state='stopped'")
    indexes = {r["name"] for r in db.query(
        "SELECT name FROM sqlite_master WHERE type='index'"
        " AND tbl_name='observed_containers'")}
    assert {"observed_containers_deployment", "observed_containers_repository"} <= indexes
    db.close()


def test_current_projection_replaces_disappeared_resources(db):
    _import(db)
    observed.replace_current(db, [], [], [], "2026-08-23T01:00:00Z")
    assert not db.query("SELECT 1 FROM observed_deployments")
    assert not db.query("SELECT 1 FROM observed_containers")
    assert not db.query("SELECT 1 FROM observed_routes")
