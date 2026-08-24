"""Current observed-only deployments imported from exact live identities.

These rows provide truthful attribution and routing. Per
DC2-2026-08-24-OBSERVED-LIFECYCLE the daemon may start/stop/restart the exact
recorded containers and read their logs; configuration authority (apply,
rollback, remove, recreation) still requires adopting the stack through
reviewed repository configuration. The import atomically replaces the complete
projection; it never retains disappeared containers as history.
"""

from __future__ import annotations

import json
import re
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2.daemon import deploy_runtime as rt
from devcoordinator2.daemon import docker_cli
from devcoordinator2.daemon.db import Database
from devcoordinator2.protocol import ProtocolError

SOURCE = "legacy-current-import"
DOMAIN_LABEL_RE = re.compile(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$")


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def list_deployments(db: Database, repository_id: str | None = None) -> list[dict]:
    params: tuple = ()
    where = ""
    if repository_id is not None:
        where = " WHERE d.repository_id=?"
        params = (repository_id,)
    rows = db.query(
        "SELECT d.*, r.domain, r.port AS route_port, r.public"
        " FROM observed_deployments d LEFT JOIN observed_routes r"
        " ON r.observed_deployment_id=d.observed_deployment_id" + where +
        " ORDER BY d.repository_id, d.name, d.observed_deployment_id", params)
    return [_list_row(r) for r in rows]


def _list_row(row: dict) -> dict:
    return {
        "deployment_id": row["observed_deployment_id"],
        "repository_id": row["repository_id"],
        "name": row["name"],
        "source": "observed",
        "state": row["state"],
        "health": row["health"],
        "domain": row["domain"],
        "route_port": row["route_port"],
        "current_generation": None,
        "updated_at": row["observed_at"],
        "ttl_expires_at": None,
        "observed_only": True,
        "public": bool(row["public"]) if row["public"] is not None else False,
    }


def exists(db: Database, deployment_id: str | None) -> bool:
    if not deployment_id:
        return False
    return bool(db.query(
        "SELECT 1 FROM observed_deployments WHERE observed_deployment_id=?",
        (deployment_id,)))


def status(db: Database, deployment_id: str) -> dict | None:
    rows = db.query(
        "SELECT d.*, r.domain, r.port AS route_port, r.component AS route_component,"
        " r.public FROM observed_deployments d LEFT JOIN observed_routes r"
        " ON r.observed_deployment_id=d.observed_deployment_id"
        " WHERE d.observed_deployment_id=?", (deployment_id,))
    if not rows:
        return None
    row = rows[0]
    components = []
    for c in db.query(
            "SELECT * FROM observed_containers WHERE observed_deployment_id=?"
            " ORDER BY compose_service, name, container_id", (deployment_id,)):
        components.append({
            "name": c["compose_service"],
            "display_name": c["name"],
            "type": "container",
            "state": c["state"],
            "health": c["health"],
            "generation": None,
            "binding": {"kind": "observed-container", "identity": c["container_id"]},
            "port": row["route_port"] if c["compose_service"] == row["route_component"]
            else None,
            "restarts": None,
            "owned": False,
            "independent_control": False,
            "last_error": c["status"] if c["health"] == "unhealthy" else None,
        })
    return {
        "deployment_id": deployment_id,
        "repository_id": row["repository_id"],
        "name": row["name"],
        "source": "observed",
        "state": row["state"],
        "health": row["health"],
        "current_generation": None,
        "previous_generation": None,
        "domain": row["domain"],
        "route_port": row["route_port"],
        "ttl_expires_at": None,
        "components": components,
        "log_dir": None,
        "observed_only": True,
        "native_project": row["native_project"],
        "observation_source": row["source"],
        "observed_at": row["observed_at"],
        "public": bool(row["public"]) if row["public"] is not None else False,
    }


def replace_current(db: Database, deployments: list[dict], containers: list[dict],
                    routes: list[dict], observed_at: str) -> dict:
    imported_at = observed_at
    with db.transaction() as conn:
        conn.execute("DELETE FROM observed_routes")
        conn.execute("DELETE FROM observed_containers")
        conn.execute("DELETE FROM observed_deployments")
        for d in deployments:
            conn.execute(
                "INSERT INTO observed_deployments(observed_deployment_id, repository_id,"
                " name, native_project, state, health, source, evidence_json, observed_at,"
                " imported_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
                (d["deployment_id"], d["repository_id"], d["name"], d["native_project"],
                 d["state"], d["health"], SOURCE,
                 json.dumps(d.get("evidence", {}), sort_keys=True), observed_at, imported_at))
        for c in containers:
            conn.execute(
                "INSERT INTO observed_containers(container_id, observed_deployment_id,"
                " repository_id, name, image, compose_service, state, status, health,"
                " observed_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
                (c["container_id"], c["deployment_id"], c["repository_id"], c["name"],
                 c["image"], c["compose_service"], "running", c["status"], c["health"],
                 observed_at))
        for route in routes:
            conflict = conn.execute(
                "SELECT deployment_id FROM domain_routes WHERE domain=?", (route["domain"],)
            ).fetchone()
            if conflict is not None:
                raise ValueError(
                    f"observed route {route['domain']} conflicts with a managed route")
            conn.execute(
                "INSERT INTO observed_routes(domain, observed_deployment_id, component, port,"
                " public, evidence_json, observed_at) VALUES(?,?,?,?,?,?,?)",
                (route["domain"], route["deployment_id"], route["component"], route["port"],
                 int(route["public"]), json.dumps(route.get("evidence", {}), sort_keys=True),
                 observed_at))
    return {"deployments": len(deployments), "containers": len(containers),
            "routes": len(routes)}


# -- lifecycle on exact recorded containers ----------------------------------

def _containers_for(db: Database, dep_id: str, component: str | None) -> list[dict]:
    rows = [dict(r) for r in db.query(
        "SELECT * FROM observed_containers WHERE observed_deployment_id=?"
        " ORDER BY compose_service, name, container_id", (dep_id,))]
    if component is not None:
        rows = [r for r in rows if r["compose_service"] == component]
        if not rows:
            raise ProtocolError("args_invalid", f"no observed component {component!r}")
    if not rows:
        raise ProtocolError("deployment_action_failed",
                            "no containers are recorded for this observed deployment")
    return rows


def _container_health(container_id: str, state: str) -> str:
    if state != "running":
        return "none"
    try:
        info = docker_cli.inspect(container_id)
    except docker_cli.DockerError:
        return "unknown"
    reported = ((info.get("State") or {}).get("Health") or {}).get("Status")
    return {"healthy": "healthy", "unhealthy": "unhealthy",
            "starting": "starting"}.get(reported, "unknown")


def refresh_states(db: Database, dep_id: str) -> None:
    """Re-inspect every recorded container and recompute the deployment row."""
    now = _now()
    states = []
    for c in db.query("SELECT container_id FROM observed_containers"
                      " WHERE observed_deployment_id=?", (dep_id,)):
        cid = c["container_id"]
        live = rt.container_state(cid)
        health = _container_health(cid, live["state"])
        states.append((live["state"], health))
        with db.transaction() as conn:
            conn.execute(
                "UPDATE observed_containers SET state=?, status=?, health=?, observed_at=?"
                " WHERE container_id=?",
                (live["state"], live.get("status", live["state"]), health, now, cid))
    if not states:
        return
    if all(s == "running" for s, _ in states):
        dep_state = "running"
    elif all(s in ("stopped", "missing") for s, _ in states):
        dep_state = "stopped"
    elif any(s == "failed" for s, _ in states) and not any(s == "running"
                                                           for s, _ in states):
        dep_state = "failed"
    else:
        dep_state = "degraded"
    if any(h == "unhealthy" for _, h in states):
        dep_health = "unhealthy"
    elif states and all(h == "healthy" for _, h in states):
        dep_health = "healthy"
    else:
        dep_health = "unknown"
    with db.transaction() as conn:
        conn.execute("UPDATE observed_deployments SET state=?, health=?, observed_at=?"
                     " WHERE observed_deployment_id=?", (dep_state, dep_health, now, dep_id))


def control(db: Database, action: str, dep_id: str, component: str | None) -> dict:
    """start/stop/restart the exact recorded containers. Never recreates,
    reconfigures, or removes anything; a missing container is reported, not
    replaced."""
    containers = _containers_for(db, dep_id, component)
    errors = []
    for c in containers:
        cid = c["container_id"]
        if rt.container_state(cid)["state"] == "missing":
            errors.append(f"{c['compose_service']}: container {cid[:12]} no longer exists;"
                          " re-import or adopt through repository configuration")
            continue
        try:
            if action == "start":
                rt.start_container(cid)
            elif action == "stop":
                rt.stop_container(cid)
            else:
                rt.restart_container(cid)
        except rt.RuntimeError_ as exc:
            errors.append(f"{c['compose_service']}: {exc}")
    refresh_states(db, dep_id)
    result = status(db, dep_id)
    if errors:
        raise ProtocolError("deployment_action_failed",
                            f"{action} failed for {len(errors)} container(s): "
                            + "; ".join(errors)[:900])
    return result


def logs(db: Database, dep_id: str, component: str, tail_lines: int) -> dict:
    containers = _containers_for(db, dep_id, component)
    tails = []
    for c in containers:
        try:
            tails.append(rt.container_logs(c["container_id"], tail_lines))
        except rt.RuntimeError_ as exc:
            tails.append(f"(logs unavailable: {exc})")
    return {"deployment_id": dep_id, "component": component,
            "tail": "\n".join(tails)[-65536:], "truncated_before_tail": True,
            "log_path": None, "observed_only": True}


# -- routed domain -----------------------------------------------------------

def set_domain(db: Database, dep_id: str, domain: str | None, port: int | None,
               component: str | None, public: bool | None) -> dict:
    """Set, change, or clear the routed domain of an observed deployment.
    Note docs/console.md: a later current-state re-import replaces this
    projection, including a domain set here."""
    route = db.query("SELECT * FROM observed_routes WHERE observed_deployment_id=?",
                     (dep_id,))
    now = _now()
    comp = component
    if domain is not None and not route:
        if port is None:
            raise ProtocolError(
                "args_invalid",
                "this observed deployment publishes no route yet; pass 'port'"
                " (the host port its service listens on) together with 'domain'")
        containers = _containers_for(db, dep_id, None)
        if comp is None and len(containers) == 1:
            comp = containers[0]["compose_service"]
        if comp is None:
            raise ProtocolError("args_invalid",
                                "'component' is required (several containers exist)")
    with db.transaction() as conn:
        if domain is None:
            conn.execute("DELETE FROM observed_routes WHERE observed_deployment_id=?",
                         (dep_id,))
        elif route:
            conn.execute(
                "UPDATE observed_routes SET domain=?, port=?, public=?, observed_at=?"
                " WHERE observed_deployment_id=?",
                (domain, port if port is not None else route[0]["port"],
                 int(public) if public is not None else route[0]["public"], now, dep_id))
        else:
            conn.execute(
                "INSERT INTO observed_routes(domain, observed_deployment_id, component,"
                " port, public, evidence_json, observed_at) VALUES(?,?,?,?,?,?,?)",
                (domain, dep_id, comp, port, int(bool(public)),
                 json.dumps({"set_by": "deployment.set_domain"}), now))
    updated = status(db, dep_id)
    return {"deployment_id": dep_id, "domain": updated["domain"],
            "route_port": updated["route_port"], "public": updated["public"],
            "observed_only": True}


def missing_install_fixtures(db: Database) -> list[str]:
    """List only missing Phase-8 installed-acceptance fixtures with no state."""
    candidates: list[str] = []
    rows = db.query(
        "SELECT repository_id, root_path FROM repositories"
        " WHERE root_path LIKE '/tmp/dc2-installed-%' ORDER BY root_path")
    for row in rows:
        root = row["root_path"]
        if Path(root).exists():
            continue
        rid = row["repository_id"]
        managed = db.query("SELECT 1 FROM deployments WHERE repository_id=?", (rid,))
        imported = db.query(
            "SELECT 1 FROM observed_deployments WHERE repository_id=?", (rid,))
        if not managed and not imported:
            candidates.append(root)
    return candidates


def prune_missing_install_fixtures(db: Database,
                                   candidates: list[str] | None = None) -> list[str]:
    targets = candidates if candidates is not None else missing_install_fixtures(db)
    pruned: list[str] = []
    with db.transaction() as conn:
        for root in targets:
            row = conn.execute(
                "SELECT repository_id FROM repositories WHERE root_path=?", (root,)
            ).fetchone()
            if row is None or Path(root).exists():
                continue
            rid = row["repository_id"]
            managed = conn.execute(
                "SELECT 1 FROM deployments WHERE repository_id=?", (rid,)).fetchone()
            imported = conn.execute(
                "SELECT 1 FROM observed_deployments WHERE repository_id=?", (rid,)).fetchone()
            if managed is not None or imported is not None:
                continue
            conn.execute("DELETE FROM worktrees WHERE repository_id=?", (rid,))
            conn.execute("DELETE FROM repositories WHERE repository_id=?", (rid,))
            pruned.append(root)
    return pruned
