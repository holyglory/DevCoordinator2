"""Deployment status, listing, and bounded logs (read-only views)."""

from __future__ import annotations

from pathlib import Path

from devcoordinator2.daemon import deploy_engine as eng
from devcoordinator2.daemon import deploy_runtime as rt
from devcoordinator2.daemon import deploy_state as st
from devcoordinator2.daemon import health_checks, observed, ports
from devcoordinator2.daemon.capture import tail_file
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.deploy_config import (
    ComponentSpec,
    list_deployment_names,
    load_deployment_spec,
)
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.repoconfig import ConfigError
from devcoordinator2.daemon.server import Caller
from devcoordinator2.protocol import ProtocolError


def component_states(ctx: eng.Ctx) -> list[dict]:
    out = []
    port_map = {**ports.assigned(ctx.db, ctx.dep_id, 0)}
    current = (st.get_deployment(ctx.db, ctx.dep_id) or {}).get("current_generation")
    if current:
        port_map.update(ports.assigned(ctx.db, ctx.dep_id, current))
    for c in st.components(ctx.db, ctx.dep_id):
        spec = ctx.spec.component(c["name"])
        receipts = st.compose_completions(
            ctx.db, ctx.dep_id, c["name"], current or 0) \
            if spec is not None and spec.type == "compose" else {}
        desires = st.compose_service_desires(ctx.db, ctx.dep_id, c["name"]) \
            if spec is not None and spec.type == "compose" else {}
        if spec is not None and spec.type == "compose" \
                and c["desired_state"] == "stopped":
            desires.update({service: "stopped" for service in spec.services
                            if service not in spec.finite_services})
        live = _live(c, spec, receipts, desires)
        if live.get("services") and spec is not None:
            live["services"] = [
                {**service,
                 "independent": service["name"] in spec.independent_services}
                for service in live["services"]
            ]
        item = {
            "name": c["name"], "type": c["type"], "state": live["state"],
            "health": live["health"], "generation": c["generation"],
            "binding": {"kind": c["binding_kind"], "identity": c["binding_identity"]},
            "port": port_map.get(c["name"]), "restarts": live.get("restarts", 0),
            "owned": bool(spec and st.is_owned(spec)),
            "independent_control": bool(spec and spec.independent_control),
            "last_error": c["last_error"],
        }
        if live.get("services"):
            item["services"] = live["services"]
        if receipts:
            item["completed_services"] = list(receipts.values())
        out.append(item)
    return out

def _live(c: dict, spec: ComponentSpec | None,
          receipts: dict[str, dict] | None = None,
          desires: dict[str, str] | None = None) -> dict:
    kind, identity = c["binding_kind"], c["binding_identity"]
    if spec is not None and spec.type == "external":
        host, port = spec.tcp.rsplit(":", 1)
        ok, _ = health_checks.tcp_probe(host, int(port))
        return {"state": "running" if ok else "failed",
                "health": "healthy" if ok else "unhealthy"}
    if not identity:
        return {"state": c["state"], "health": c["health"]}
    if kind == "unit":
        s = rt.process_state(identity)
    elif kind == "container":
        s = rt.container_state(identity)
    elif kind == "compose":
        s = rt.compose_state(identity, spec.services if spec else (),
                             spec.finite_services if spec else (), receipts, desires)
    else:
        s = {"state": c["state"]}
    health = c["health"] if s["state"] == "running" else "none"
    if s["state"] != "running" and c["state"] == "running":
        health = "unhealthy"
    return {"state": s["state"], "health": health, "restarts": s.get("restarts", 0),
            "services": s.get("services")}

def status(ctx: eng.Ctx, row: dict) -> dict:
    comps = component_states(ctx)
    owned = [c for c in comps if c["owned"]]
    if row["state"] in ("applying",):
        state = row["state"]
    elif owned and all(c["state"] == "running" for c in owned):
        state = "running"
    elif all(c["state"] == "stopped" for c in owned):
        state = "stopped"
    else:
        state = "degraded"
    routes = ctx.db.query("SELECT domain, port FROM domain_routes WHERE deployment_id=?",
                            (ctx.dep_id,))
    repo = ctx.db.query("SELECT display_name FROM repositories WHERE repository_id=?",
                        (row["repository_id"],))
    route_comp = ctx.spec.route_component
    return {
        "deployment_id": ctx.dep_id, "name": ctx.spec.name, "source": ctx.source,
        "repository_id": row["repository_id"],
        "repository_name": repo[0]["display_name"] if repo else None,
        "state": state,
        "current_generation": row["current_generation"],
        "previous_generation": row["previous_generation"],
        "domain": routes[0]["domain"] if routes else None,
        "route_port": routes[0]["port"] if routes else None,
        "route_component": route_comp.name if route_comp else None,
        "public": bool(row["public"]),
        "ttl_expires_at": row["ttl_expires_at"],
        "components": comps, "log_dir": str(ctx.dir / "logs"),
    }

def list_all(db: Database, registry: Registry, path: Path | None, caller: Caller) -> dict:
    active_repositories = {
        repository["repository_id"] for repository in registry.list_repositories()
    }
    rows = [
        row for row in st.list_deployments(db)
        if row["repository_id"] in active_repositories
    ]
    declared = []
    if path is not None:
        reg = eng.resolve_registration(registry, path, caller)
        try:
            for dname in list_deployment_names(Path(reg.worktree_path)):
                spec = load_deployment_spec(Path(reg.worktree_path), dname)
                for source in spec.sources:
                    declared.append({"name": dname, "source": source,
                                     "deployment_id": st.deployment_id(
                                         reg.worktree_id, dname, source)})
        except ConfigError as exc:
            raise ProtocolError("repository_config_invalid", str(exc)) from exc
    route_ports = {r["deployment_id"]: r["port"] for r in db.query(
        "SELECT deployment_id, port FROM domain_routes")}
    repo_names = {
        repository["repository_id"]: repository["display_name"]
        for repository in registry.list_repositories()
    }
    managed = [{
        "deployment_id": r["deployment_id"], "repository_id": r["repository_id"],
        "repository_name": repo_names.get(r["repository_id"]),
        "name": r["name"], "source": r["source"], "state": r["state"],
        "domain": r["domain"], "public": bool(r["public"]),
        "current_generation": r["current_generation"],
        "route_port": route_ports.get(r["deployment_id"]), "updated_at": r["updated_at"],
        "ttl_expires_at": r["ttl_expires_at"], "observed_only": False,
    } for r in rows]
    imported = [
        row for row in observed.list_deployments(db)
        if row["repository_id"] in active_repositories
    ]
    return {"deployments": [*managed, *imported], "declared": declared}

def logs(ctx: eng.Ctx, row: dict, worktree: Path, component: str,
     tail_lines: int) -> dict:
    spec = ctx.spec.component(component)
    rows = {c["name"]: c for c in st.components(ctx.db, ctx.dep_id)}
    if spec is None or component not in rows:
        raise ProtocolError("args_invalid", f"no component {component!r}")
    c = rows[component]
    if spec.type == "process" or component == "build":
        data, truncated = tail_file(ctx.log_path(component), 65536)
        text = "\n".join(data.decode("utf-8", "replace").splitlines()[-tail_lines:])
        return {"component": component, "tail": text, "truncated_before_tail": truncated,
                "log_path": str(ctx.log_path(component))}
    if c["binding_kind"] == "container" and c["binding_identity"]:
        return {"component": component,
                "tail": rt.container_logs(c["binding_identity"], tail_lines),
                "container_id": c["binding_identity"]}
    if c["binding_kind"] == "compose" and c["binding_identity"]:
        runtime_generation = eng.runtime_generation(ctx, row, c)
        gen = st.generation(ctx.db, ctx.dep_id, runtime_generation)
        gp = Path(gen["path"]) if gen else worktree
        return {"component": component, "tail": rt.compose_logs(
            c["binding_identity"], ctx.compose_files(spec, gp), gp,
            ctx.compose_env_files(spec, gp, runtime_generation),
            tail_lines)}
    return {"component": component, "tail": "", "note": "component has no logs"}
