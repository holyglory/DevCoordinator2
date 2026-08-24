import json
import subprocess
import sys
from pathlib import Path

from devcoordinator2 import ids
from devcoordinator2.daemon.db import Database

ROOT = Path(__file__).resolve().parents[1]


def test_import_current_state_excludes_historical_resources(tmp_path: Path):
    repo = tmp_path / "repo"
    repo.mkdir()
    export = {
        "exported_at": "2026-08-23T00:00:00Z",
        "authority": {
            "repositories": [{"legacy_repo_id": "L1", "root": str(repo),
                              "display_name": "r", "state": "active"}],
            "port_assignments": [{"root": str(repo), "server": "web", "port": 3003,
                                  "status": "active"}],
            "server_definitions": [
                {"root": str(repo), "name": "web", "role": "web", "cwd": ".",
                 "command": ["npm", "start"], "health_url_template": "http://x/",
                 "environment_names": ["API_TOKEN"],
                 "environment_looks_secret": ["API_TOKEN"]},
                {"root": str(repo), "name": "old-preview", "role": "temporary", "cwd": ".",
                 "command": ["npm", "start"], "health_url_template": None,
                 "environment_names": [], "environment_looks_secret": []},
            ],
            "docker_resources": [
                {"root": str(repo), "container_id": "a" * 64, "name": "app-1",
                 "image": "app:current", "compose_project": "app-current",
                 "compose_service": "app"},
                {"root": str(repo), "container_id": "b" * 64, "name": "old-1",
                 "image": "app:old", "compose_project": "app-old",
                 "compose_service": "app"},
            ],
        },
        "routes": {"owners": ["Owner@Example.test"], "routes": [
            {"slug": "news", "auth": "google", "upstream_port": 3003}]},
        "access_control": {"pending_requests": [{"email": "p@example.test"}]},
        "telegram": {"bots": [{"owner": "owner@example.test", "projects": ["L1"]}],
                     "authorizations": [{"chat_id": 4242, "status": "approved",
                                         "username": "u"}]},
        "open_bugs": [{"component": "api", "summary": "legacy bug", "expected": "a",
                       "actual": "b", "steps": "c"}],
    }
    export_file = tmp_path / "export.json"
    export_file.write_text(json.dumps(export))
    state = tmp_path / "state"
    bugs_dir = tmp_path / "bugs"
    db = Database(state / "authority.sqlite3")
    repo_id = ids.repository_id(repo)
    with db.transaction() as conn:
        conn.execute("INSERT INTO repositories VALUES(?,?,?,?,?,?)",
                     (repo_id, str(repo), "repo", "t", 1000, "t"))
        conn.execute("INSERT INTO worktrees VALUES(?,?,?,?,?)",
                     (ids.worktree_id(repo), repo_id, str(repo), "t", "t"))
        conn.execute("INSERT INTO repositories VALUES(?,?,?,?,?,?)",
                     ("rfixture", "/tmp/dc2-installed-missing-import-fixture", "fixture",
                      "t", 1000, "t"))
        conn.execute("INSERT INTO worktrees VALUES(?,?,?,?,?)",
                     ("wfixture", "rfixture", "/tmp/dc2-installed-missing-import-fixture",
                      "t", "t"))
    db.close()
    live_file = tmp_path / "live.json"
    live_file.write_text(json.dumps({"ok": True, "result": {"containers": [
        {"id": "a" * 64, "name": "app-1", "image": "app:current", "state": "running",
         "status": "Up 1 hour (healthy)"},
        {"id": "b" * 64, "name": "old-1", "image": "app:old", "state": "exited",
         "status": "Exited (0)"},
    ]}}))
    route_file = tmp_path / "route-map.json"
    route_file.write_text(json.dumps({"routes": [{
        "domain": "app", "repository_root": str(repo),
        "native_project": "app-current", "component": "app", "port": 3003,
        "public": True, "evidence": {"probe": "ok"},
    }]}))
    routes_file = tmp_path / "routes.json"

    base = [sys.executable, str(ROOT / "scripts/legacy_import.py"), "--export",
            str(export_file), "--state-dir", str(state), "--bugs-dir", str(bugs_dir),
            "--live-containers", str(live_file), "--current-route-map", str(route_file)]
    proc = subprocess.run([*base, "--dry-run"], capture_output=True, text=True, check=True)
    dry = json.loads(proc.stdout)
    assert dry["dry_run"] and dry["administrators"] == ["owner@example.test"]
    assert not bugs_dir.exists() or not list(bugs_dir.glob("*.json"))
    assert len(dry["current_observed"]["containers"]) == 1

    proc = subprocess.run([*base, "--routes-path", str(routes_file), "--base-domain",
                           "example.test", "--prune-missing-install-fixtures"],
                          capture_output=True, text=True, check=True)
    report = json.loads(proc.stdout)
    db = Database(state / "authority.sqlite3")
    users = db.query("SELECT email, administrator FROM users")
    assert users[0]["email"] == "owner@example.test" and users[0]["administrator"] == 1
    assert db.query("SELECT email FROM telegram_chats WHERE chat_id=4242")[0]["email"] == \
        "owner@example.test"
    scopes = sorted(r["scope"] for r in db.query("SELECT scope FROM telegram_subscriptions"))
    assert "server" in scopes and any(s.startswith("repository:r") for s in scopes)
    assert len(db.query("SELECT * FROM observed_deployments")) == 1
    current = db.query("SELECT * FROM observed_containers")
    assert len(current) == 1 and current[0]["container_id"] == "a" * 64
    assert db.query("SELECT domain, port FROM observed_routes")[0]["domain"] == "app"
    assert not db.query("SELECT 1 FROM repositories WHERE repository_id='rfixture'")
    db.close()
    assert len(list(bugs_dir.glob("b*.json"))) == 1
    plan = report["deployment_declaration_plan"]
    assert len([p for p in plan if p.get("legacy_server")]) == 1
    assert plan[0]["legacy_port"] == 3003
    assert plan[0]["suggested"]["deployment.web.component.web"]["command"] == ["npm", "start"]
    assert "API_TOKEN" in plan[0]["suggested"]["deployment.web.component.web"][
        "env_names_to_reference_outside_repo"]
    assert report["pending_access_requests"][0]["email"] == "p@example.test"
    assert report["current_observed"]["imported"] == {
        "deployments": 1, "containers": 1, "routes": 1}
    assert report["pruned_install_fixtures"] == [
        "/tmp/dc2-installed-missing-import-fixture"]
    route_doc = json.loads(routes_file.read_text())
    assert route_doc["routes"][0]["domain"] == "app.example.test"
