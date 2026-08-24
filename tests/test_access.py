import json
from pathlib import Path

import pytest

from devcoordinator2.daemon.access import Access, guard, public_commands
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.server import Caller
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError


@pytest.fixture
def world(tmp_path: Path):
    config = InstanceConfig(socket_path=tmp_path / "s", state_dir=tmp_path / "state",
                            unit_prefix="devcoordinator2-dev", slice_name="x.slice",
                            client_group="", base_domain="example.test",
                            admin_emails=("owner@example.test",))
    db = Database(config.database_path)
    with db.transaction() as conn:
        conn.execute("INSERT INTO repositories VALUES('r1','/x','x','t',1,'t')")
        conn.execute("INSERT INTO worktrees VALUES('w1','r1','/x','t','t')")
        for dep in ("d1", "d2"):
            conn.execute(
                "INSERT INTO deployments(deployment_id, repository_id, worktree_id, name,"
                " source, domain, spec_fingerprint, spec_json, state, created_at,"
                " created_by_uid, client, updated_at, current_generation) VALUES"
                f"('{dep}','r1','w1','{dep}','worktree','{dep}','f','{{}}','running','t',1,"
                "'other','t',1)")
            conn.execute(f"INSERT INTO domain_routes VALUES('{dep}','{dep}','api',"
                         f"2000{dep[1]},1,'t')")
    access = Access(config, db)
    yield type("W", (), {"config": config, "db": db, "access": access})
    db.close()


def local() -> Caller:
    return Caller(pid=0, uid=1000, gid=1000, client_kind="other", client_session=None)


def public(email: str) -> Caller:
    return Caller(pid=0, uid=999, gid=999, client_kind="edge", client_session=None,
                  identity=email)


def test_bootstrap_admin_and_invite_accept_grant_revoke(world):
    cmds = public_commands(world.access)
    users = cmds["user.list"]({}, local())
    assert users["owners"] == ["owner@example.test"]
    inv = cmds["user.invite"]({"email": "Dev@Example.test", "grants":
                               [{"deployment_id": "d1", "role": "operator"}]}, local())
    assert inv["email"] == "dev@example.test"
    # Only the invited identity is admitted.
    with pytest.raises(ProtocolError, match="no invitation"):
        cmds["user.accept_invitation"]({"email": "stranger@example.test"}, public("x@y"))
    accepted = cmds["user.accept_invitation"]({"email": "dev@example.test",
                                               "subject": "sub-1"}, public("dev@example.test"))
    assert accepted["accepted"] is True and accepted["administrator"] is False
    who = cmds["user.whoami"]({}, public("dev@example.test"))
    assert who["grants"] == {"d1": "operator"} and who["administrator"] is False
    # Route document carries the access section for the edge.
    doc = json.loads(world.config.routes_path.read_text())
    assert doc["access"]["owners"] == ["owner@example.test"]
    assert doc["access"]["grants"] == [{"identity": "dev@example.test", "deployment_id": "d1",
                                        "role": "operator"}]
    cmds["grant.set"]({"email": "dev@example.test", "deployment_id": "d2", "role": "viewer"},
                      local())
    cmds["grant.remove"]({"email": "dev@example.test", "deployment_id": "d1"}, local())
    doc = json.loads(world.config.routes_path.read_text())
    assert doc["access"]["grants"] == [{"identity": "dev@example.test", "deployment_id": "d2",
                                        "role": "viewer"}]
    cmds["user.remove"]({"email": "dev@example.test"}, local())
    assert json.loads(world.config.routes_path.read_text())["access"]["grants"] == []
    with pytest.raises(ProtocolError, match="no user"):
        cmds["user.remove"]({"email": "dev@example.test"}, local())


def test_guard_enforces_roles(world):
    calls = []

    def record(name):
        def handler(args, caller):
            calls.append(name)
            if name == "deployment.list":
                return {"deployments": [{"deployment_id": "d1"}, {"deployment_id": "d2"}],
                        "declared": [{"name": "x"}]}
            if name == "health.repositories":
                return {"repositories": [{"repository_id": "r1", "deployments": [
                    {"deployment_id": "d1"}, {"deployment_id": "d2"}]},
                    {"repository_id": "r9", "deployments": []}],
                    "host": {"secret": 1}}
            return {"ok": name}
        return handler

    names = ["deployment.list", "deployment.status", "deployment.start", "deployment.apply",
             "health.summary", "health.repositories", "test.start", "user.list"]
    handlers = guard({n: record(n) for n in names} | public_commands(world.access),
                     world.access, world.db)
    # Local callers are unrestricted.
    assert handlers["deployment.apply"]({}, local()) == {"ok": "deployment.apply"}
    # Unknown public identity: denied everywhere.
    with pytest.raises(ProtocolError, match="not an admitted user"):
        handlers["deployment.list"]({}, public("nobody@example.test"))
    # Admit a viewer on d1 only.
    handlers["user.invite"]({"email": "v@example.test",
                             "grants": [{"deployment_id": "d1", "role": "viewer"}]}, local())
    handlers["user.accept_invitation"]({"email": "v@example.test"}, public("v@example.test"))
    viewer = public("v@example.test")
    listed = handlers["deployment.list"]({}, viewer)
    assert [d["deployment_id"] for d in listed["deployments"]] == ["d1"]
    assert listed["declared"] == []
    assert handlers["deployment.status"]({"deployment_id": "d1"}, viewer) == {
        "ok": "deployment.status"}
    with pytest.raises(ProtocolError, match="requires viewer"):
        handlers["deployment.status"]({"deployment_id": "d2"}, viewer)
    with pytest.raises(ProtocolError, match="requires operator"):
        handlers["deployment.start"]({"deployment_id": "d1"}, viewer)
    for admin_only in ("deployment.apply", "health.summary", "test.start", "user.list"):
        with pytest.raises(ProtocolError, match="requires administrator"):
            handlers[admin_only]({}, viewer)
    repos = handlers["health.repositories"]({}, viewer)
    assert "host" not in repos
    assert [r["repository_id"] for r in repos["repositories"]] == ["r1"]
    assert [d["deployment_id"] for d in repos["repositories"][0]["deployments"]] == ["d1"]
    # Promote to operator, then revoke: effective immediately on the next call.
    handlers["grant.set"]({"email": "v@example.test", "deployment_id": "d1",
                           "role": "operator"}, local())
    assert handlers["deployment.start"]({"deployment_id": "d1"}, viewer) == {
        "ok": "deployment.start"}
    handlers["user.remove"]({"email": "v@example.test"}, local())
    with pytest.raises(ProtocolError, match="not an admitted user"):
        handlers["deployment.start"]({"deployment_id": "d1"}, viewer)
    # Administrators (bootstrapped from instance config) may do everything.
    assert handlers["user.list"]({}, public("owner@example.test"))["owners"]


def test_guard_plan_reads_follow_repository_grants(world):
    with world.db.transaction() as conn:
        conn.execute("INSERT INTO repositories VALUES('r2','/y','other','t',1,'t')")
        conn.execute("INSERT INTO tasks(task_id, repository_id, seq, position, title,"
                     " outcome, kind, status, created_at, created_by, updated_at)"
                     " VALUES('p1','r1',1,1,'Some plain task','Some plain outcome.',"
                     "'goal','planned','t','t','t')")

    def record(name):
        def handler(args, caller):
            if name == "plan.overview" and not args.get("repository_id"):
                return {"repositories": [{"repository_id": "r1"},
                                         {"repository_id": "r2"}]}
            return {"ok": name}
        return handler

    names = ["plan.overview", "task.history", "decision.tail", "decision.search",
             "task.create", "task.update", "release.create", "release.update",
             "release.request", "release.deliver", "decision.record",
             "decision.summarize"]
    handlers = guard({n: record(n) for n in names} | public_commands(world.access),
                     world.access, world.db)
    handlers["user.invite"]({"email": "v@example.test",
                             "grants": [{"deployment_id": "d1", "role": "viewer"}]},
                            local())
    handlers["user.accept_invitation"]({"email": "v@example.test"},
                                       public("v@example.test"))
    viewer = public("v@example.test")
    # The picker is filtered to repositories with a viewable deployment.
    picker = handlers["plan.overview"]({}, viewer)
    assert [r["repository_id"] for r in picker["repositories"]] == ["r1"]
    assert handlers["plan.overview"]({"repository_id": "r1"}, viewer) == {
        "ok": "plan.overview"}
    with pytest.raises(ProtocolError, match="requires viewer"):
        handlers["plan.overview"]({"repository_id": "r2"}, viewer)
    # A 'path' reference would implicitly register: public callers may not.
    with pytest.raises(ProtocolError, match="requires viewer"):
        handlers["plan.overview"]({"path": "/x"}, viewer)
    assert handlers["task.history"]({"task_id": "p1"}, viewer) == {
        "ok": "task.history"}
    with pytest.raises(ProtocolError, match="requires viewer"):
        handlers["task.history"]({"task_id": "p" + "0" * 16}, viewer)
    for read in ("decision.tail", "decision.search"):
        assert handlers[read]({"repository_id": "r1"}, viewer) == {"ok": read}
        with pytest.raises(ProtocolError, match="requires viewer"):
            handlers[read]({"repository_id": "r2"}, viewer)
    for mutation in ("task.create", "task.update", "release.create",
                     "release.update", "release.request", "release.deliver",
                     "decision.record", "decision.summarize"):
        with pytest.raises(ProtocolError, match="requires administrator"):
            handlers[mutation]({}, viewer)
    # Administrators and local callers pass through untouched.
    assert handlers["release.request"]({}, public("owner@example.test")) == {
        "ok": "release.request"}
    assert handlers["task.create"]({}, local()) == {"ok": "task.create"}
