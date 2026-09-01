#!/usr/bin/env python3
"""Read-only export of the legacy installation's import-eligible state into
one reviewable JSON document (docs/legacy-deletion-map.md).

Never writes to the legacy stores. Never exports secrets: bot tokens,
upstream authorizations, environment values, and session material are
omitted by construction (environment variable *names* only). All paths are
arguments — nothing installation-specific lives in this file.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from datetime import UTC, datetime
from pathlib import Path

SECRET_HINT = ("token", "secret", "password", "passwd", "credential", "key", "authorization")


def export_authority(path: Path) -> dict:
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    q = lambda sql, *p: [dict(r) for r in conn.execute(sql, p).fetchall()]  # noqa: E731
    repos = {r["repo_id"]: r for r in q(
        "SELECT repo_id, canonical_root, display_name, state FROM repositories")}
    active = {rid for rid, r in repos.items() if r["state"] == "active"}
    installations = {r["repo_id"]: r for r in q(
        "SELECT repo_id, status, startup_fenced FROM repository_installations")}
    out = {
        "repositories": [
            {"legacy_repo_id": rid, "root": r["canonical_root"],
             "display_name": r["display_name"], "state": r["state"],
             "installation": installations.get(rid, {}).get("status")}
            for rid, r in repos.items()],
        "port_assignments": [
            {"root": repos.get(r["repo_id"], {}).get("canonical_root"),
             "server": r["server_name"], "port": r["port"], "status": r["status"]}
            for r in q("SELECT repo_id, server_name, port, status FROM port_assignments")],
        "server_definitions": [],
        "docker_resources": [],
        "database_bindings": [],
    }
    for s in q("SELECT server_definition_id, repo_id, name, role, cwd, health_url_template,"
               " log_path FROM server_definitions"):
        if s["repo_id"] not in active:
            continue
        args = [r["argument"] for r in q(
            "SELECT argument FROM server_command_arguments WHERE server_definition_id=?"
            " ORDER BY ordinal", s["server_definition_id"])]
        env_names = [r["name"] for r in q(
            "SELECT name FROM server_environment WHERE server_definition_id=?",
            s["server_definition_id"])]
        out["server_definitions"].append({
            "root": repos[s["repo_id"]]["canonical_root"], "name": s["name"],
            "role": s["role"], "cwd": s["cwd"], "command": args,
            "health_url_template": s["health_url_template"], "log_path": s["log_path"],
            "environment_names": env_names,
            "environment_looks_secret": [n for n in env_names
                                         if any(h in n.lower() for h in SECRET_HINT)],
        })
    for d in q("SELECT docker_resource_id, full_container_id, current_name, image, repo_id"
               " FROM docker_resources WHERE repo_id IS NOT NULL"):
        labels = {r["name"]: r["value"] for r in q(
            "SELECT name, value FROM docker_labels WHERE docker_resource_id=?",
            d["docker_resource_id"])}
        out["docker_resources"].append({
            "root": repos.get(d["repo_id"], {}).get("canonical_root"),
            "container_id": d["full_container_id"], "name": d["current_name"],
            "image": d["image"],
            "compose_project": labels.get("com.docker.compose.project"),
            "compose_service": labels.get("com.docker.compose.service"),
        })
    for b in q("SELECT database_binding_id, repo_id, database_name, engine_kind"
               " FROM database_bindings"):
        out["database_bindings"].append({
            "root": repos.get(b["repo_id"], {}).get("canonical_root"),
            "database": b["database_name"], "engine": b["engine_kind"]})
    conn.close()
    return out


def export_routes(path: Path) -> dict:
    doc = json.loads(path.read_text())
    pub = doc.get("publication", doc)
    routes = []
    for slug, r in (pub.get("routes") or {}).items():
        up = r.get("upstream") or {}
        routes.append({"slug": slug, "auth": r.get("auth"), "kind": r.get("kind"),
                       "upstream_host": up.get("host"), "upstream_port": up.get("port"),
                       "upstream_status": up.get("status", "configured"),
                       "has_upstream_authorization": bool(r.get("upstream_authorization")
                                                          or up.get("authorization"))})
    access = pub.get("access") or {}
    return {"generation": pub.get("generation"), "domain": pub.get("domain"),
            "console_host": pub.get("console_host"), "routes": routes,
            "owners": access.get("owners", []), "grants": access.get("grants", {})}


def export_access_control(path: Path) -> dict:
    data = json.loads(path.read_text())
    users = data.get("users") or {}
    requests = data.get("requests") or data.get("access_requests") or data.get("pending") or []
    if isinstance(requests, dict):
        requests = list(requests.values())
    return {"users": list(users.keys()) if isinstance(users, dict) else users,
            "pending_requests": [
                {k: v for k, v in (r.items() if isinstance(r, dict) else [])
                 if k in ("email", "requested_at", "resource", "status")}
                for r in requests]}


def export_telegram(path: Path) -> dict:
    data = json.loads(path.read_text())
    bots = []
    for bot in (data.get("bots") or {}).values() if isinstance(data.get("bots"), dict) \
            else (data.get("bots") or []):
        bots.append({"label": bot.get("label"), "username": bot.get("username"),
                     "owner": bot.get("ownerEmail"), "enabled": bot.get("enabled"),
                     "projects": bot.get("projects", []),
                     "token": "<redacted>"})
    auths = data.get("authorizationRequests") or {}
    auth_list = list(auths.values()) if isinstance(auths, dict) else auths
    return {"bots": bots, "authorizations": [
        {"chat_id": a.get("chatId") or a.get("chat_id"), "username": a.get("username"),
         "status": a.get("status"), "approved_by": a.get("approvedBy")}
        for a in auth_list if isinstance(a, dict)],
        "outbox_pending": sum(1 for m in (data.get("outbox") or [])
                              if isinstance(m, dict) and m.get("status") != "delivered")}


def export_bugs(path: Path) -> list:
    out = []
    for file in sorted(path.glob("*.json")) if path.is_dir() else []:
        try:
            out.append(json.loads(file.read_text()))
        except (OSError, json.JSONDecodeError):
            continue
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--authority-db", type=Path, required=True)
    ap.add_argument("--routes-publication", type=Path)
    ap.add_argument("--access-control", type=Path)
    ap.add_argument("--telegram-state", type=Path)
    ap.add_argument("--bugs-dir", type=Path)
    ap.add_argument("--out", type=Path, required=True)
    ns = ap.parse_args()
    doc = {"exported_at": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
           "authority": export_authority(ns.authority_db)}
    if ns.routes_publication and ns.routes_publication.exists():
        doc["routes"] = export_routes(ns.routes_publication)
    if ns.access_control and ns.access_control.exists():
        doc["access_control"] = export_access_control(ns.access_control)
    if ns.telegram_state and ns.telegram_state.exists():
        doc["telegram"] = export_telegram(ns.telegram_state)
    if ns.bugs_dir:
        doc["open_bugs"] = export_bugs(ns.bugs_dir)
    ns.out.write_text(json.dumps(doc, indent=2))
    summary = {k: (len(v) if isinstance(v, list) else
                   {kk: len(vv) for kk, vv in v.items() if isinstance(vv, list)})
               for k, v in doc.items() if k != "exported_at"}
    print(json.dumps({"out": str(ns.out), "summary": summary}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
