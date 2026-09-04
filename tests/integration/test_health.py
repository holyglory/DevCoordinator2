"""Phase 4: live measurement of a real deployment, reconciliation, history."""

from __future__ import annotations

import subprocess
import time

from integration.helpers import ROOT_ONLY, _call, _write_config

pytestmark = ROOT_ONLY

TOML = '''schema = 2
[deployment.svc]
components = ["db", "api"]
[deployment.svc.component.db]
type = "postgres"
image = "postgres:16-alpine"
[deployment.svc.component.api]
type = "process"
command = ["python3", "serve.py"]
port = true
health = { tcp = true, timeout_seconds = 30 }
'''


SERVE = ("import os, http.server as h\n"
         "h.HTTPServer(('127.0.0.1', int(os.environ['PORT'])),"
         " h.SimpleHTTPRequestHandler).serve_forever()\n")


def test_health_views_measure_real_workloads(world):
    (world.repo / "serve.py").write_text(SERVE)
    _write_config(world.repo, world.caller, TOML)
    subprocess.run(["git", "-C", str(world.repo), "-c", "safe.directory=*", "add", "."],
                   check=True, capture_output=True)
    resp = _call(world, "deployment.apply", {"path": str(world.repo), "name": "svc"})
    assert resp["ok"], resp
    dep_id = resp["result"]["deployment_id"]
    db_comp = next(c for c in resp["result"]["components"] if c["name"] == "db")
    db_id = db_comp["binding"]["identity"]

    # Two sampling ticks so deltas exist.
    deadline = time.monotonic() + 60
    summary = None
    while time.monotonic() < deadline:
        summary = _call(world, "health.summary", {})["result"]
        if summary["host"].get("reconciliation") and \
                summary["host"]["reconciliation"]["managed_memory"] > 0:
            break
        time.sleep(0.1)  # no public sample event; bounded observation fallback
    assert summary and summary["host"]["memory_total"] > 0
    rec = summary["host"]["reconciliation"]
    assert rec["managed_memory"] > 0  # our postgres + api are measured and attributed
    assert rec["other_memory"] >= 0 and rec["other_cpu_percent"] >= 0
    assert summary["container_counts"]["managed-permanent"] >= 1
    assert isinstance(summary["alerts"], list)
    assert summary["sampling"]["retention_days"] == 30

    repos = _call(world, "health.repositories", {})["result"]
    mine = next(r for r in repos["repositories"]
                if r["repository_id"] == resp["result"]["repository_id"])
    assert mine["memory_bytes"] > 0
    assert mine["health"] == "healthy"
    assert any(d["deployment_id"] == dep_id for d in mine["deployments"])
    assert "cpu_percent" in repos["devcoordinator"]
    assert "cpu_percent" in repos["shared_unattributed"]

    # Attribution can lag one sampling tick behind the reconciliation totals
    # (a tick may land mid-apply, before component bindings are recorded), so
    # poll for it like reconciliation above instead of reading once.
    deadline = time.monotonic() + 60
    detail = {"components": []}
    kinds = set()
    while time.monotonic() < deadline:
        detail = _call(world, "health.repository", {"path": str(world.repo)})["result"]
        kinds = {(c["kind"], c.get("component")) for c in detail["components"]}
        if ("container", "db") in kinds and ("component", "api") in kinds:
            break
        time.sleep(0.1)  # no public sample event; bounded observation fallback
    assert ("container", "db") in kinds
    assert ("component", "api") in kinds
    db_entry = next(c for c in detail["components"] if c["id"] == db_id)
    assert db_entry["memory_bytes"] > 0 and db_entry["pids"] >= 1

    # Storage tick (runs at start + every 5 min) attributes the PostgreSQL volume.
    deadline = time.monotonic() + 90
    storage = {}
    while time.monotonic() < deadline:
        storage = _call(world, "health.repositories", {})["result"]
        mine = next(r for r in storage["repositories"]
                    if r["repository_id"] == resp["result"]["repository_id"])
        if mine["storage"].get("total"):
            break
        time.sleep(0.1)  # no public sample event; bounded observation fallback
    assert mine["storage"]["checkout"] > 0
    assert mine["storage"]["postgres_data"] > 0
    host_storage = _call(world, "health.summary", {})["result"]["storage"]
    assert host_storage["fs_used"] >= host_storage["managed_repositories"]
    assert host_storage["docker_shared"] >= 0

    # PostgreSQL operational facts are content-free numbers.
    detail = _call(world, "health.repository", {"path": str(world.repo)})["result"]
    pg = next(c for c in detail["components"] if c["id"] == f"{dep_id}/db")
    assert pg["storage"]["pg_connections"] >= 1
    assert pg["storage"]["pg_wal_bytes"] > 0
    assert pg["storage"]["pg_database_bytes"] > 0

    # One-minute aggregates appear within ~75 s and are queryable per subject.
    deadline = time.monotonic() + 90
    points = []
    while time.monotonic() < deadline and not points:
        hist = _call(world, "health.history", {
            "subject_kind": "repository", "subject_id": resp["result"]["repository_id"],
            "metric": "memory_bytes", "minutes": 10})
        points = hist["result"]["points"]
        time.sleep(0.1)  # no public sample event; bounded observation fallback
    assert points and points[0]["avg"] > 0 and points[0]["samples"] >= 1

    _call(world, "deployment.remove", {"path": str(world.repo), "name": "svc",
                                       "delete_data": True})
