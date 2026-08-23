import json
import subprocess
import sys
from pathlib import Path

from devcoordinator2.daemon.db import Database

ROOT = Path(__file__).resolve().parents[1]


def test_import_admins_telegram_bugs_and_plan(tmp_path: Path):
    repo = tmp_path / "repo"
    repo.mkdir()
    export = {
        "authority": {
            "repositories": [{"legacy_repo_id": "L1", "root": str(repo), "display_name": "r",
                              "state": "active"}],
            "port_assignments": [{"root": str(repo), "server": "web", "port": 3003,
                                  "status": "active"}],
            "server_definitions": [{"root": str(repo), "name": "web", "role": "web",
                                    "cwd": ".", "command": ["npm", "start"],
                                    "health_url_template": "http://x/",
                                    "environment_names": ["API_TOKEN"],
                                    "environment_looks_secret": ["API_TOKEN"]}],
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
    proc = subprocess.run([sys.executable, str(ROOT / "scripts/legacy_import.py"), "--export",
                           str(export_file), "--state-dir", str(state), "--bugs-dir",
                           str(bugs_dir), "--dry-run"], capture_output=True, text=True,
                          check=True)
    dry = json.loads(proc.stdout)
    assert dry["dry_run"] and dry["administrators"] == ["owner@example.test"]
    assert not bugs_dir.exists() or not list(bugs_dir.glob("*.json"))
    proc = subprocess.run([sys.executable, str(ROOT / "scripts/legacy_import.py"), "--export",
                           str(export_file), "--state-dir", str(state), "--bugs-dir",
                           str(bugs_dir)], capture_output=True, text=True, check=True)
    report = json.loads(proc.stdout)
    db = Database(state / "authority.sqlite3")
    users = db.query("SELECT email, administrator FROM users")
    assert users[0]["email"] == "owner@example.test" and users[0]["administrator"] == 1
    assert db.query("SELECT email FROM telegram_chats WHERE chat_id=4242")[0]["email"] == \
        "owner@example.test"
    scopes = sorted(r["scope"] for r in db.query("SELECT scope FROM telegram_subscriptions"))
    assert "server" in scopes and any(s.startswith("repository:r") for s in scopes)
    db.close()
    assert len(list(bugs_dir.glob("b*.json"))) == 1
    plan = report["deployment_declaration_plan"]
    assert plan[0]["legacy_port"] == 3003
    assert plan[0]["suggested"]["deployment.web.component.web"]["command"] == ["npm", "start"]
    assert "API_TOKEN" in plan[0]["suggested"]["deployment.web.component.web"][
        "env_names_to_reference_outside_repo"]
    assert report["pending_access_requests"][0]["email"] == "p@example.test"
