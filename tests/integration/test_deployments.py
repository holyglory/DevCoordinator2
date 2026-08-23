"""Phase 3: heterogeneous deployment (HTTP process, worker, dedicated
PostgreSQL, Docker cache) through real systemd units and containers."""

from __future__ import annotations

import json
import subprocess
import threading
import time
import urllib.request

from integration.helpers import ROOT_ONLY, UNIT_PREFIX, _call, _write_config

pytestmark = ROOT_ONLY

SERVER = '''import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
VERSION = "__VERSION__"
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({"version": VERSION, "generation": os.environ.get("DC2_GENERATION"),
                           "has_db": "DATABASE_URL" in os.environ,
                           "cache_port": os.environ.get("DC2_PORT_CACHE"),
                           "port": os.environ.get("PORT")}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
'''

TOML = '''schema = 1
[deployment.web]
source = ["checkout", "worktree"]
domain = { checkout = "app", worktree = "app-dev" }
components = ["db", "cache", "api", "worker"]

[deployment.web.component.db]
type = "postgres"
image = "postgres:16-alpine"
database = "app"
user = "app"

[deployment.web.component.cache]
type = "docker"
image = "valkey/valkey:9.1.0-alpine"
port = 6379

[deployment.web.component.api]
type = "process"
command = ["python3", "server.py"]
port = true
route = true
health = { path = "/healthz", timeout_seconds = 30 }
depends_on = ["db", "cache"]

[deployment.web.component.worker]
type = "process"
command = ["sleep", "3600"]
depends_on = ["db"]
'''


def _git(world, *args):
    """Run git as the caller (the repository owner), like an agent would."""
    caller = world.caller
    subprocess.run(["setpriv", f"--reuid={caller.pw_uid}", f"--regid={caller.pw_gid}",
                    "--init-groups", "--", "git", *args], cwd=world.repo, check=True,
                   capture_output=True,
                   env={"PATH": "/usr/bin:/bin", "HOME": str(world.base),
                        "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                        "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"})


def _setup(world, version="v1", commit=True):
    (world.repo / "server.py").write_text(SERVER.replace("__VERSION__", version))
    _write_config(world.repo, world.caller, TOML)
    if commit:
        _git(world, "add", ".")
        _git(world, "commit", "-qm", f"app {version}")


def _get(port: int) -> dict:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=5) as resp:
        return json.loads(resp.read())


def _comp(status: dict, name: str) -> dict:
    return next(c for c in status["components"] if c["name"] == name)


def _routes(world) -> dict:
    return json.loads((world.base / "state" / "public" / "routes.json").read_text())


def _units(prefix: str) -> list[str]:
    out = subprocess.run(["systemctl", "list-units", "--all", "--plain", "--no-legend",
                          f"{prefix}*.service"], capture_output=True, text=True).stdout
    return [ln.split()[0] for ln in out.splitlines() if ln.split()]


def test_worktree_apply_stop_start_reapply_remove(world):
    _setup(world)
    resp = _call(world, "deployment.apply", {"path": str(world.repo), "name": "web@worktree"})
    assert resp["ok"], resp
    status = resp["result"]
    assert status["state"] == "running" and status["current_generation"] == 1
    assert status["previous_generation"] is None  # worktree keeps no previous
    api = _comp(status, "api")
    assert api["state"] == "running" and api["health"] == "healthy" and api["port"]
    body = _get(api["port"])
    assert body == {"version": "v1", "generation": "1", "has_db": True,
                    "cache_port": str(_comp(status, "cache")["port"]), "port": str(api["port"])}
    # Route document: domain -> route port, atomic and checksummed.
    doc = _routes(world)
    route = next(r for r in doc["routes"] if r["deployment_id"] == status["deployment_id"])
    assert route["label"] == "app-dev" and route["port"] == api["port"]
    assert doc["payload_sha256"]
    # Dedicated PostgreSQL is reachable with the injected credentials and persists.
    env_file = next((world.base / "state" / "deployments" / status["deployment_id"] / "env")
                    .glob("api-g1.env"))
    url = next(ln for ln in env_file.read_text().splitlines()
               if ln.startswith("DATABASE_URL=")).split("=", 1)[1].strip('"')
    subprocess.run(["psql", url, "-v", "ON_ERROR_STOP=1", "-c",
                    "create table keep(x int); insert into keep values (7)"], check=True,
                   capture_output=True)
    db_id = _comp(status, "db")["binding"]["identity"]
    labels = json.loads(subprocess.run(["docker", "inspect", "--format",
                                        "{{json .Config.Labels}}", db_id],
                                       capture_output=True, text=True).stdout)
    assert labels["devcoordinator2.purpose"] == "permanent"
    assert labels["devcoordinator2.data"] == "persistent"
    assert labels["devcoordinator2.caller_uid"] == str(world.caller.pw_uid)

    # Stop: units gone, containers stopped but present, route port withdrawn.
    stopped = _call(world, "deployment.stop",
                    {"path": str(world.repo), "name": "web@worktree"})
    assert stopped["ok"], stopped
    assert stopped["result"]["state"] == "stopped"
    assert _units(UNIT_PREFIX + "-deploy") == []
    assert _routes(world)["routes"] == []
    state = subprocess.run(["docker", "inspect", "--format", "{{.State.Status}}", db_id],
                           capture_output=True, text=True).stdout.strip()
    assert state == "exited"
    # Start: same database container and data survive.
    started = _call(world, "deployment.start",
                    {"path": str(world.repo), "name": "web@worktree"})
    assert started["ok"], started
    assert started["result"]["state"] == "running"
    assert _comp(started["result"], "db")["binding"]["identity"] == db_id
    out = subprocess.run(["psql", url, "-tA", "-c", "select x from keep"],
                         capture_output=True, text=True, check=True).stdout.strip()
    assert out == "7"
    assert _get(_comp(started["result"], "api")["port"])["version"] == "v1"

    # Component-level restart of the worker only.
    one = _call(world, "deployment.restart", {"path": str(world.repo), "name": "web@worktree",
                                              "component": "worker"})
    assert one["ok"], one
    assert _comp(one["result"], "worker")["state"] == "running"

    # Live edit + reapply: new generation, new port, old unit retired, route moved.
    old_port = _comp(started["result"], "api")["port"]
    old_unit = _comp(started["result"], "api")["binding"]["identity"]
    _setup(world, version="v2", commit=False)
    again = _call(world, "deployment.apply", {"path": str(world.repo), "name": "web@worktree"})
    assert again["ok"], again
    assert again["result"]["current_generation"] == 2
    new_port = _comp(again["result"], "api")["port"]
    assert new_port != old_port
    assert _get(new_port)["version"] == "v2"
    assert old_unit not in _units(UNIT_PREFIX + "-deploy")
    assert _routes(world)["routes"][0]["port"] == new_port
    assert _comp(again["result"], "db")["binding"]["identity"] == db_id  # stable component kept

    # Logs are bounded and real.
    logs = _call(world, "deployment.logs", {"path": str(world.repo), "name": "web@worktree",
                                            "component": "db", "tail_lines": 20})
    assert logs["ok"] and "database system is ready" in logs["result"]["tail"]

    # Inventory classifies our containers as managed-permanent; others unmanaged.
    inv = _call(world, "health.containers", {})["result"]
    ours = [c for c in inv["containers"] if c["id"] == db_id]
    assert ours and ours[0]["classification"] == "managed-permanent"
    assert ours[0]["component"] == "db"
    assert inv["counts"]["unmanaged"] >= 0

    # Remove without data deletion keeps the volume; with deletion removes it.
    removed = _call(world, "deployment.remove", {"path": str(world.repo),
                                                 "name": "web@worktree"})
    assert removed["ok"], removed
    vol = f"devcoordinator2-{status['deployment_id']}-db-pgdata"
    assert subprocess.run(["docker", "volume", "inspect", vol],
                          capture_output=True).returncode == 0
    assert _units(UNIT_PREFIX + "-deploy") == []
    subprocess.run(["docker", "volume", "rm", vol], check=True, capture_output=True)


def test_checkout_generations_and_rollback(world):
    _setup(world, version="v1")
    first = _call(world, "deployment.apply",
                  {"path": str(world.repo), "name": "web@checkout"})
    assert first["ok"], first
    dep_id = first["result"]["deployment_id"]
    gen1 = world.base / "state" / "deployments" / dep_id / "gen-1"
    assert (gen1 / "server.py").exists()
    assert _get(_comp(first["result"], "api")["port"])["version"] == "v1"
    _setup(world, version="v2")
    second = _call(world, "deployment.apply",
                   {"path": str(world.repo), "name": "web@checkout"})
    assert second["ok"], second
    assert second["result"]["current_generation"] == 2, second["result"]
    assert second["result"]["previous_generation"] == 1
    assert _get(_comp(second["result"], "api")["port"])["version"] == "v2"
    # Worktree edits never affect the checkout deployment.
    (world.repo / "server.py").write_text(SERVER.replace("__VERSION__", "dirty"))
    assert _get(_comp(second["result"], "api")["port"])["version"] == "v2"
    back = _call(world, "deployment.rollback",
                 {"path": str(world.repo), "name": "web@checkout"})
    assert back["ok"], back
    assert back["result"]["rolled_back_from"] == 2
    assert back["result"]["current_generation"] == 3
    assert _get(_comp(back["result"], "api")["port"])["version"] == "v1"
    # Only current + previous generation directories are retained.
    dep_dir = world.base / "state" / "deployments" / dep_id
    gens = sorted(p.name for p in dep_dir.glob("gen-*"))
    assert len(gens) <= 2
    _call(world, "deployment.remove", {"path": str(world.repo), "name": "web@checkout",
                                       "delete_data": True})
    vol = f"devcoordinator2-{dep_id}-db-pgdata"
    assert subprocess.run(["docker", "volume", "inspect", vol],
                          capture_output=True).returncode != 0


def test_failed_component_is_degraded_and_busy_is_immediate(world):
    broken = TOML.replace('command = ["python3", "server.py"]', 'command = ["false"]')
    (world.repo / "server.py").write_text(SERVER)
    _write_config(world.repo, world.caller, broken)
    resp = _call(world, "deployment.apply", {"path": str(world.repo), "name": "web@worktree"})
    assert resp["ok"] is False
    assert resp["error"]["code"] == "deployment_apply_failed"
    detail = json.loads(resp["error"]["detail"])
    assert any(c["name"] == "api" and c["state"] != "running" for c in detail["components"])
    status = _call(world, "deployment.status",
                   {"path": str(world.repo), "name": "web@worktree"})
    assert status["result"]["state"] in ("degraded", "failed")
    assert status["result"]["route_port"] is None  # never route to an unhealthy candidate
    assert "queued" not in json.dumps(resp).lower()

    # Busy: two concurrent applies — exactly one proceeds, the other gets busy now.
    _write_config(world.repo, world.caller, TOML)
    results = []

    def go():
        results.append(_call(world, "deployment.apply",
                             {"path": str(world.repo), "name": "web@worktree"}))

    threads = [threading.Thread(target=go) for _ in range(2)]
    for t in threads:
        t.start()
        time.sleep(0.3)
    for t in threads:
        t.join()
    codes = sorted(r.get("error", {}).get("code", "ok") for r in results)
    assert codes == ["busy", "ok"], results
    _call(world, "deployment.remove", {"path": str(world.repo), "name": "web@worktree",
                                       "delete_data": True})
