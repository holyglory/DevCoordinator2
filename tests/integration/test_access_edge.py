"""Phase 5: edge-asserted identity over the real daemon socket, role
enforcement, invitation admission, and route-document access section."""

from __future__ import annotations

import json
import pwd

from integration.helpers import ROOT_ONLY, _call, _request, _write_config, call_as

pytestmark = ROOT_ONLY

TOML = '''schema = 2
[deployment.svc]
components = ["api"]
domain = "svc"
[deployment.svc.component.api]
type = "process"
command = ["python3", "serve.py"]
port = true
route = true
health = { tcp = true, timeout_seconds = 30 }
'''
SERVE = ("import os, http.server as h\n"
         "h.HTTPServer(('127.0.0.1', int(os.environ['PORT'])),"
         " h.SimpleHTTPRequestHandler).serve_forever()\n")


def _edge_request(world, edge_uid: int, command: str, args: dict, identity: str) -> dict:
    request = _request(command, args)
    request["client"] = {"kind": "edge", "identity": identity}
    return call_as(edge_uid, pwd.getpwuid(edge_uid).pw_gid, world.daemon.socket_path, request)


def test_edge_identity_trust_roles_and_revocation(world):
    # Run the daemon with an edge uid (nobody) and a bootstrap administrator.
    edge_uid = pwd.getpwnam("nobody").pw_uid
    world.daemon.stop()
    env = dict(world.daemon.env)
    env["DEVCOORDINATOR2_EDGE_UID"] = str(edge_uid)
    env["DEVCOORDINATOR2_ADMIN_EMAILS"] = "owner@example.test"
    env["DEVCOORDINATOR2_BASE_DOMAIN"] = "example.test"
    world.daemon.env = env
    world.daemon.start()
    (world.repo / "serve.py").write_text(SERVE)
    _write_config(world.repo, world.caller, TOML)
    applied = _call(world, "deployment.apply", {"path": str(world.repo), "name": "svc"})
    assert applied["ok"], applied
    dep_id = applied["result"]["deployment_id"]

    # A non-edge uid cannot assert an identity at all.
    spoof = _request("user.whoami", {})
    spoof["client"] = {"kind": "edge", "identity": "owner@example.test"}
    resp = call_as(world.caller.pw_uid, world.caller.pw_gid, world.daemon.socket_path, spoof)
    assert resp["error"]["code"] == "permission_denied"

    # The edge uid can; the bootstrap administrator is recognised.
    who = _edge_request(world, edge_uid, "user.whoami", {}, "owner@example.test")
    assert who["ok"] and who["result"]["administrator"] is True

    # Unknown identity: denied; invite, accept via the edge, then viewer rights.
    denied = _edge_request(world, edge_uid, "deployment.list", {}, "dev@example.test")
    assert denied["error"]["code"] == "permission_denied"
    inv = _call(world, "user.invite", {"email": "dev@example.test",
                                       "grants": [{"deployment_id": dep_id, "role": "viewer"}]})
    assert inv["ok"], inv
    acc = _edge_request(world, edge_uid, "user.accept_invitation",
                        {"email": "dev@example.test", "subject": "s1"}, "dev@example.test")
    assert acc["ok"] and acc["result"]["accepted"] is True
    listed = _edge_request(world, edge_uid, "deployment.list", {}, "dev@example.test")
    assert [d["deployment_id"] for d in listed["result"]["deployments"]] == [dep_id]
    status = _edge_request(world, edge_uid, "deployment.status",
                           {"deployment_id": dep_id},
                           "dev@example.test")
    assert status["ok"], status
    assert status["result"]["state"] == "running"
    stop = _edge_request(world, edge_uid, "deployment.stop",
                         {"deployment_id": dep_id}, "dev@example.test")
    assert stop["error"]["code"] == "permission_denied"  # viewer cannot operate
    assert _edge_request(world, edge_uid, "health.summary", {}, "dev@example.test")[
        "error"]["code"] == "permission_denied"
    repos = _edge_request(world, edge_uid, "health.repositories", {}, "dev@example.test")
    assert repos["ok"] and "host" not in repos["result"]

    # Route document carries owners + grants; promotion to operator enables stop.
    doc = json.loads((world.base / "state" / "public" / "routes.json").read_text())
    assert doc["access"]["owners"] == ["owner@example.test"]
    assert doc["access"]["grants"] == [{"identity": "dev@example.test", "deployment_id": dep_id,
                                        "role": "viewer"}]
    assert doc["routes"][0]["domain"] == "svc.example.test"
    assert doc["routes"][0]["auth"] == "authenticated"
    assert _call(world, "grant.set", {"email": "dev@example.test", "deployment_id": dep_id,
                                      "role": "operator"})["ok"]
    stop = _edge_request(world, edge_uid, "deployment.stop",
                         {"deployment_id": dep_id}, "dev@example.test")
    assert stop["ok"] and stop["result"]["state"] == "stopped"
    # Revocation is effective on the next request and in the route document.
    assert _call(world, "user.remove", {"email": "dev@example.test"})["ok"]
    again = _edge_request(world, edge_uid, "deployment.status",
                          {"deployment_id": dep_id},
                          "dev@example.test")
    assert again["error"]["code"] == "permission_denied"
    doc = json.loads((world.base / "state" / "public" / "routes.json").read_text())
    assert doc["access"]["grants"] == []
    _call(world, "deployment.remove", {"path": str(world.repo), "name": "svc",
                                       "delete_data": True})
