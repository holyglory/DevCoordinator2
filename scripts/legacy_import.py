#!/usr/bin/env python3
"""Import reviewed legacy state into DevCoordinator2 (docs/legacy-deletion-map.md).

Imports only what the handover allows and what can be mapped honestly:
administrators (route-document owners), Telegram chat links and
repository/server subscriptions, and open bugs. Deployments, ports, and
domains are NOT imported blindly: the tool prints a declaration plan (one
`[deployment.<name>]` skeleton per legacy route/server) for the owner to
review and commit into each repository's .devcoordinator.toml, after which
`deployment apply` recreates them with exact new identities. Never touches
the legacy stores. `--dry-run` prints the plan without writing.
"""

from __future__ import annotations

import argparse
import json
import secrets
import sys
from datetime import UTC, datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from devcoordinator2 import bugs, ids
from devcoordinator2.daemon.db import Database


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def plan_deployments(export: dict) -> list[dict]:
    plan = []
    servers = export.get("authority", {}).get("server_definitions", [])
    ports = {(p["root"], p["server"]): p for p in export.get("authority", {}).get(
        "port_assignments", []) if p.get("status") == "active"}
    for s in servers:
        port = ports.get((s["root"], s["name"]))
        plan.append({
            "repository_root": s["root"], "legacy_server": s["name"], "role": s["role"],
            "suggested": {
                f"deployment.{s['name']}": {
                    "source": "worktree", "components": [s["name"]],
                    "domain": "<label from legacy route, if any>",
                },
                f"deployment.{s['name']}.component.{s['name']}": {
                    "type": "process", "command": s["command"], "cwd": s["cwd"] or ".",
                    "port": bool(port), "route": bool(port),
                    "health": ({"path": "/"} if s.get("health_url_template") else None),
                    "env_names_to_reference_outside_repo":
                        s.get("environment_looks_secret", []),
                },
            },
            "legacy_port": port["port"] if port else None,
        })
    for r in export.get("routes", {}).get("routes", []):
        plan.append({"legacy_route": r["slug"], "auth": r["auth"],
                     "upstream_port": r.get("upstream_port"),
                     "note": "map to the deployment serving this upstream; auth=public → "
                             "public = true; authenticated → grants"})
    return plan


def import_state(export: dict, db: Database, bugs_dir: Path, dry_run: bool) -> dict:
    report = {"administrators": [], "telegram_chats": [], "subscriptions": [], "bugs": [],
              "pending_access_requests": export.get("access_control", {}).get(
                  "pending_requests", [])}
    owners = [o.lower() for o in export.get("routes", {}).get("owners", [])]
    for email in owners:
        report["administrators"].append(email)
        if not dry_run and not db.query("SELECT 1 FROM users WHERE email=?", (email,)):
            with db.transaction() as conn:
                conn.execute("INSERT INTO users(user_id, email, administrator, created_at,"
                             " created_by) VALUES(?,?,1,?,'legacy-import')",
                             ("u" + secrets.token_hex(8), email, _now()))
    tg = export.get("telegram", {})
    legacy_roots = {r["legacy_repo_id"]: r["root"] for r in
                    export.get("authority", {}).get("repositories", [])}
    for bot in tg.get("bots", []):
        owner = (bot.get("owner") or (owners[0] if owners else "")).lower()
        scopes = ["server"] if owner in owners else []
        for legacy_repo in bot.get("projects", []):
            root = legacy_roots.get(legacy_repo)
            if root and Path(root).is_dir():
                scopes.append(f"repository:{ids.repository_id(Path(root))}")
        for auth in tg.get("authorizations", []):
            if auth.get("status") != "approved" or not auth.get("chat_id"):
                continue
            chat_id = int(auth["chat_id"])
            report["telegram_chats"].append({"chat_id": chat_id, "email": owner})
            report["subscriptions"].extend({"chat_id": chat_id, "scope": s} for s in scopes)
            if not dry_run:
                with db.transaction() as conn:
                    conn.execute("INSERT OR REPLACE INTO telegram_chats(chat_id, email, label,"
                                 " linked_at) VALUES(?,?,?,?)",
                                 (chat_id, owner, auth.get("username"), _now()))
                    for scope in scopes:
                        conn.execute("INSERT OR IGNORE INTO telegram_subscriptions(chat_id,"
                                     " scope, created_at) VALUES(?,?,?)",
                                     (chat_id, scope, _now()))
    for legacy in export.get("open_bugs", []):
        fields = {
            "component": str(legacy.get("component") or legacy.get("area") or "legacy")[:64],
            "summary": str(legacy.get("summary") or legacy.get("title") or "")[:200],
            "expected": str(legacy.get("expected") or legacy.get("expected_behavior")
                            or "-")[:2000],
            "actual": str(legacy.get("actual") or legacy.get("actual_behavior") or "-")[:2000],
            "steps": str(legacy.get("steps") or legacy.get("reproduction")
                         or legacy.get("reproduction_steps") or "-")[:4000],
        }
        if not fields["summary"]:
            continue
        report["bugs"].append(fields["summary"])
        if not dry_run:
            try:
                bugs.report(**fields, reporter="legacy-import", directory=bugs_dir)
            except bugs.BugError as exc:
                report["bugs"][-1] = f"SKIPPED ({exc}): {fields['summary']}"
    return report


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--export", type=Path, required=True)
    ap.add_argument("--state-dir", type=Path, required=True,
                    help="DevCoordinator2 state dir (authority.sqlite3 lives here)")
    ap.add_argument("--bugs-dir", type=Path, required=True)
    ap.add_argument("--dry-run", action="store_true")
    ns = ap.parse_args()
    export = json.loads(ns.export.read_text())
    db = Database(ns.state_dir / "authority.sqlite3")
    try:
        report = import_state(export, db, ns.bugs_dir, ns.dry_run)
    finally:
        db.close()
    report["deployment_declaration_plan"] = plan_deployments(export)
    report["dry_run"] = ns.dry_run
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
