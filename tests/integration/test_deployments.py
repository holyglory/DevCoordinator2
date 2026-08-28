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
    broken = TOML.replace('command = ["python3", "server.py"]', 'command = ["false"]') \
        .replace('timeout_seconds = 30', 'timeout_seconds = 120')
    (world.repo / "server.py").write_text(SERVER)
    _write_config(world.repo, world.caller, broken)
    started_at = time.monotonic()
    resp = _call(world, "deployment.apply", {"path": str(world.repo), "name": "web@worktree"})
    elapsed = time.monotonic() - started_at
    assert resp["ok"] is False
    assert resp["error"]["code"] == "deployment_apply_failed"
    assert elapsed < 30, f"terminal unit consumed {elapsed:.1f}s of a 120s readiness deadline"
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


def test_readiness_allows_process_to_recover_within_restart_policy(world):
    restart_server = '''from pathlib import Path
import sys
counter = Path(".restart-count")
attempt = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(attempt))
if attempt < 3:
    sys.exit(1)
''' + SERVER
    (world.repo / "server.py").write_text(restart_server)
    _write_config(world.repo, world.caller, TOML)
    _git(world, "add", ".")
    _git(world, "commit", "-qm", "restart fixture")
    resp = _call(world, "deployment.apply",
                 {"path": str(world.repo), "name": "web@worktree"})
    assert resp["ok"], resp
    api = _comp(resp["result"], "api")
    assert api["state"] == "running" and api["restarts"] >= 2
    _call(world, "deployment.remove", {"path": str(world.repo), "name": "web@worktree",
                                       "delete_data": True})


COMPOSE_TOML = '''schema = 1
[deployment.stack]
source = "worktree"
domain = "stack"
components = ["compose"]

[deployment.stack.component.compose]
type = "compose"
files = ["compose.yml", "compose.route.yml"]
env_file = "compose.env"
services = ["bootstrap", "cache", "worker"]
finite_services = ["bootstrap"]
independent_services = ["worker"]
port = true
route = true
timeout_seconds = 90
'''

COMPOSE_YAML = '''services:
  bootstrap:
    image: postgres:16-alpine
    entrypoint: ["/bin/sh", "-c"]
    command: ["n=$$(cat /state/count 2>/dev/null || echo 0); expr $$n + 1 > /state/count"]
    restart: "no"
    volumes: ["state:/state"]
    labels: ["fixture=${FIXTURE_LABEL:?set fixture label}"]
  cache:
    image: valkey/valkey:9.1.0-alpine
    depends_on:
      bootstrap:
        condition: service_completed_successfully
    volumes: ["state:/state"]
  worker:
    image: postgres:16-alpine
    entrypoint: ["/bin/sh", "-c"]
    command: ["while :; do sleep 60; done"]
    depends_on:
      bootstrap:
        condition: service_completed_successfully
    volumes: ["state:/state"]
volumes:
  state:
'''

COMPOSE_ROUTE_YAML = '''services:
  cache:
    ports: ["127.0.0.1:${PORT:?Coordinator must lease PORT}:6379"]
'''


def _compose_service_id(project: str, service: str) -> str:
    proc = subprocess.run(
        ["docker", "ps", "--all", "--no-trunc", "--quiet",
         "--filter", f"label=com.docker.compose.project={project}",
         "--filter", f"label=com.docker.compose.service={service}"],
        capture_output=True, text=True, check=True)
    ids = [line for line in proc.stdout.splitlines() if line]
    assert len(ids) == 1, ids
    return ids[0]


def _compose_counter(container_id: str) -> str:
    return subprocess.run(["docker", "exec", container_id, "cat", "/state/count"],
                          capture_output=True, text=True, check=True).stdout.strip()


def test_native_compose_finite_service_receipt_and_start_semantics(world):
    _write_config(world.repo, world.caller, COMPOSE_TOML)
    (world.repo / "compose.yml").write_text(COMPOSE_YAML)
    (world.repo / "compose.route.yml").write_text(COMPOSE_ROUTE_YAML)
    (world.repo / ".gitignore").write_text("compose.env\n")
    (world.repo / "compose.env").write_text("FIXTURE_LABEL=ready\n")
    (world.repo / "marker.txt").write_text("v1\n")
    _git(world, "add", ".")
    _git(world, "commit", "-qm", "compose fixture")

    first = _call(world, "deployment.apply",
                  {"path": str(world.repo), "name": "stack@worktree"})
    assert first["ok"], first
    result = first["result"]
    component = _comp(result, "compose")
    assert result["state"] == "running"
    assert {item["name"]: item["state"] for item in component["services"]} == {
        "bootstrap": "completed", "cache": "running", "worker": "running"}
    assert component["completed_services"][0]["service"] == "bootstrap"
    assert component["completed_services"][0]["exit_code"] == 0
    project = component["binding"]["identity"]
    cache = _compose_service_id(project, "cache")
    assert _compose_counter(cache) == "1"

    unchanged = _call(world, "deployment.apply",
                      {"path": str(world.repo), "name": "stack@worktree"})
    assert unchanged["ok"] and unchanged["result"]["unchanged"] is True
    assert _compose_counter(cache) == "1"

    worker_stopped = _call(
        world, "deployment.stop",
        {"path": str(world.repo), "name": "stack@worktree",
         "component": "compose/worker"})
    assert worker_stopped["ok"] and worker_stopped["result"]["state"] == "degraded"
    stopped_component = _comp(worker_stopped["result"], "compose")
    stopped_services = {item["name"]: item for item in stopped_component["services"]}
    assert stopped_services["worker"]["state"] == "stopped"
    assert stopped_services["worker"]["independent"] is True
    assert stopped_services["cache"]["state"] == "running"
    assert worker_stopped["result"]["route_port"] == result["route_port"]
    assert _compose_counter(cache) == "1"
    worker_started = _call(
        world, "deployment.start",
        {"path": str(world.repo), "name": "stack@worktree",
         "component": "compose/worker"})
    assert worker_started["ok"] and worker_started["result"]["state"] == "running"
    assert _compose_counter(cache) == "1"

    stopped = _call(world, "deployment.stop",
                    {"path": str(world.repo), "name": "stack@worktree"})
    assert stopped["ok"] and stopped["result"]["state"] == "stopped"
    started = _call(world, "deployment.start",
                    {"path": str(world.repo), "name": "stack@worktree"})
    assert started["ok"] and started["result"]["state"] == "running"
    assert _compose_counter(cache) == "1"  # ordinary start did not rerun bootstrap

    (world.repo / "marker.txt").write_text("v2\n")
    changed = _call(world, "deployment.apply",
                    {"path": str(world.repo), "name": "stack@worktree"})
    assert changed["ok"], changed
    assert _compose_counter(_compose_service_id(project, "cache")) == "2"
    route_port = changed["result"]["route_port"]
    pong = subprocess.run(
        ["docker", "exec", _compose_service_id(project, "cache"),
         "valkey-cli", "PING"], capture_output=True, text=True, check=True)
    assert pong.stdout.strip() == "PONG" and route_port

    removed = _call(world, "deployment.remove",
                    {"path": str(world.repo), "name": "stack@worktree",
                     "delete_data": True})
    assert removed["ok"], removed
