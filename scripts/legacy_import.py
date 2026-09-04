#!/usr/bin/env python3
"""Import reviewed legacy state into DevCoordinator2 (docs/legacy-deletion-map.md).

Imports only what the handover allows and what can be mapped honestly:
administrators (route-document owners), Telegram chat links and current
registered-repository subscriptions, open bugs, and an optional replaceable
observed-only projection of exact containers that are still running. Observed
resources carry no lifecycle authority. Temporary, validation, stopped,
removed, missing, and historical resources are excluded. Never touches the
legacy stores. `--dry-run` prints the plan without writing.
"""

from __future__ import annotations

import argparse
import json
import re
import secrets
import sys
from datetime import UTC, datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from devcoordinator2 import bugs, ids
from devcoordinator2.daemon import observed, routes
from devcoordinator2.daemon.db import Database


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def plan_deployments(export: dict) -> list[dict]:
    plan = []
    servers = export.get("authority", {}).get("server_definitions", [])
    ports = {(p["root"], p["server"]): p for p in export.get("authority", {}).get(
        "port_assignments", []) if p.get("status") == "active"}
    for s in servers:
        if s.get("role") in ("temporary", "validation-port-lease"):
            continue
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
    registered_roots = {r["root_path"] for r in db.query(
        "SELECT root_path FROM repositories")}
    for bot in tg.get("bots", []):
        owner = (bot.get("owner") or (owners[0] if owners else "")).lower()
        scopes = ["server"] if owner in owners else []
        for legacy_repo in bot.get("projects", []):
            root = legacy_roots.get(legacy_repo)
            if root in registered_roots:
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


def _container_health(status: str) -> str:
    lower = status.lower()
    if "unhealthy" in lower:
        return "unhealthy"
    if "health: starting" in lower:
        return "starting"
    if "healthy" in lower:
        return "healthy"
    return "unknown"


def plan_current_observations(export: dict, live_document: dict, route_map: dict,
                              db: Database) -> dict:
    if not isinstance(live_document, dict) or live_document.get("ok") is not True:
        raise ValueError(
            "live container document must be a successful health.containers result")
    live = live_document.get("result", {}).get("containers")
    if not isinstance(live, list):
        raise ValueError("live container document has no result.containers list")
    registered = {r["root_path"]: r["repository_id"] for r in db.query(
        "SELECT repository_id, root_path FROM repositories")}
    mappings: dict[str, set[tuple[str, str, str]]] = {}
    for row in export.get("authority", {}).get("docker_resources", []):
        values = (row.get("container_id"), row.get("root"),
                  row.get("compose_project"), row.get("compose_service"))
        if all(isinstance(value, str) and value for value in values):
            cid, root, project, service = values
            mappings.setdefault(cid, set()).add((root, project, service))
    containers: list[dict] = []
    skipped: list[dict] = []
    groups: dict[tuple[str, str, str], list[dict]] = {}
    for row in live:
        if row.get("state") != "running":
            skipped.append({"container_id": row.get("id"), "reason": "not_running"})
            continue
        cid = row.get("id")
        matches = mappings.get(cid, set())
        if len(matches) != 1:
            skipped.append({"container_id": cid,
                            "reason": "no_exact_mapping" if not matches
                            else "conflicting_mapping"})
            continue
        root, project, service = next(iter(matches))
        repository_id = registered.get(root)
        if repository_id is None:
            skipped.append({"container_id": cid, "reason": "repository_not_registered",
                            "repository_root": root})
            continue
        dep_id = ids.observed_deployment_id(repository_id, project)
        item = {
            "container_id": cid,
            "deployment_id": dep_id,
            "repository_id": repository_id,
            "name": str(row.get("name") or service),
            "image": str(row.get("image") or "unknown"),
            "compose_service": service,
            "status": str(row.get("status") or ""),
            "health": _container_health(str(row.get("status") or "")),
        }
        containers.append(item)
        groups.setdefault((repository_id, root, project), []).append(item)
    deployments = []
    by_project: dict[tuple[str, str], dict] = {}
    for (repository_id, root, project), items in sorted(groups.items()):
        healths = {item["health"] for item in items}
        health = ("unhealthy" if "unhealthy" in healths else
                  "healthy" if healths == {"healthy"} else "unknown")
        dep = {
            "deployment_id": ids.observed_deployment_id(repository_id, project),
            "repository_id": repository_id,
            "name": project,
            "native_project": project,
            "state": "degraded" if health == "unhealthy" else "running",
            "health": health,
            "evidence": {"repository_root": root,
                         "container_ids": sorted(item["container_id"] for item in items),
                         "legacy_exported_at": export.get("exported_at")},
        }
        deployments.append(dep)
        by_project[(root, project)] = dep
    planned_routes = []
    raw_routes = route_map.get("routes", []) if isinstance(route_map, dict) else []
    if not isinstance(raw_routes, list):
        raise ValueError("current route map routes must be a list")
    for route in raw_routes:
        if not isinstance(route, dict):
            raise ValueError("current route entries must be objects")
        expected = {"domain", "repository_root", "native_project", "component", "port",
                    "public", "evidence"}
        if set(route) != expected:
            raise ValueError(f"current route fields must be {sorted(expected)}")
        domain = route["domain"]
        if not isinstance(domain, str) or not re.fullmatch(
                r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", domain):
            raise ValueError("current route domain must be a DNS label")
        port = route["port"]
        if not isinstance(port, int) or isinstance(port, bool) or not 1 <= port <= 65535:
            raise ValueError("current route port must be 1..65535")
        if not isinstance(route["public"], bool):
            raise ValueError("current route public must be boolean")
        dep = by_project.get((route["repository_root"], route["native_project"]))
        if dep is None:
            raise ValueError(f"current route {domain} has no imported live project")
        services = {c["compose_service"] for c in containers
                    if c["deployment_id"] == dep["deployment_id"]}
        if route["component"] not in services:
            raise ValueError(f"current route {domain} component is not running")
        planned_routes.append({
            "domain": domain, "deployment_id": dep["deployment_id"],
            "component": route["component"], "port": port,
            "public": route["public"], "evidence": route["evidence"],
        })
    return {"deployments": deployments, "containers": containers,
            "routes": planned_routes, "skipped": skipped}


def prune_unregistered_telegram_scopes(db: Database) -> list[str]:
    valid = {f"repository:{r['repository_id']}" for r in db.query(
        "SELECT repository_id FROM repositories")}
    rows = [r["scope"] for r in db.query(
        "SELECT DISTINCT scope FROM telegram_subscriptions WHERE scope LIKE 'repository:%'")]
    stale = sorted(scope for scope in rows if scope not in valid)
    if stale:
        with db.transaction() as conn:
            conn.executemany("DELETE FROM telegram_subscriptions WHERE scope=?",
                             ((scope,) for scope in stale))
    return stale


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--export", type=Path, required=True)
    ap.add_argument("--state-dir", type=Path, required=True,
                    help="DevCoordinator2 state dir (authority.sqlite3 lives here)")
    ap.add_argument("--bugs-dir", type=Path, required=True)
    ap.add_argument("--live-containers", type=Path,
                    help="successful health.containers JSON for current-only"
                         " observation import")
    ap.add_argument("--current-route-map", type=Path,
                    help="reviewed current observed-route map")
    ap.add_argument("--routes-path", type=Path,
                    help="publish routes here after a current observation import")
    ap.add_argument("--base-domain", help="base domain for route publication")
    ap.add_argument("--prune-missing-install-fixtures", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    ns = ap.parse_args()
    export = json.loads(ns.export.read_text())
    db = Database(ns.state_dir / "authority.sqlite3")
    try:
        report = import_state(export, db, ns.bugs_dir, ns.dry_run)
        if ns.live_containers:
            live = json.loads(ns.live_containers.read_text())
            route_map = json.loads(ns.current_route_map.read_text()) \
                if ns.current_route_map else {"routes": []}
            current = plan_current_observations(export, live, route_map, db)
            report["current_observed"] = {
                "deployments": current["deployments"],
                "containers": current["containers"],
                "routes": current["routes"],
                "skipped": current["skipped"],
            }
            if not ns.dry_run:
                report["current_observed"]["imported"] = observed.replace_current(
                    db, current["deployments"], current["containers"], current["routes"],
                    _now())
                if ns.routes_path:
                    if ns.base_domain is None:
                        raise ValueError("--base-domain is required with --routes-path")
                    published = routes.publish(db, ns.routes_path, ns.base_domain)
                    report["current_observed"]["route_generation"] = published["generation"]
        fixture_candidates = (observed.missing_install_fixtures(db)
                              if ns.prune_missing_install_fixtures else [])
        report["would_prune_install_fixtures"] = (
            fixture_candidates if ns.dry_run else [])
        report["pruned_install_fixtures"] = (
            [] if ns.dry_run else observed.prune_missing_install_fixtures(
                db, fixture_candidates))
        report["pruned_telegram_scopes"] = (
            [] if ns.dry_run else prune_unregistered_telegram_scopes(db))
    finally:
        db.close()
    report["deployment_declaration_plan"] = plan_deployments(export)
    report["dry_run"] = ns.dry_run
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
