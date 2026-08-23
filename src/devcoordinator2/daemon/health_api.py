"""Health views: host condition first, then repositories, then detail and
bounded history. Numbers come from the live sampler snapshot and the
bounded metric store — never fixtures."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from devcoordinator2.daemon import docker_cli, inventory, metrics_store
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.metrics_sampler import Sampler
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller, Handler
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

METRICS = ("cpu_percent", "memory_bytes", "pids", "storage_bytes", "memory_used", "load_1",
           "pg_connections", "pg_wal_bytes", "pg_temp_bytes", "pg_database_bytes",
           "io_read", "io_write")
SUBJECT_KINDS = ("host", "repository", "component", "container", "test", "daemon", "other",
                 "worktree", "deployment")


def build_health_handlers(config: InstanceConfig, db: Database, registry: Registry,
                          sampler: Sampler) -> dict[str, Handler]:
    def summary(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        if args:
            raise ProtocolError("args_invalid", f"unexpected args: {sorted(args)}")
        snap = sampler.snapshot()
        storage = snap["storage"].get(("host", "storage"), {})
        deployments = [dict(r) for r in db.query(
            "SELECT deployment_id, name, source, state FROM deployments")]
        unhealthy = [d for d in deployments if d["state"] in ("degraded", "failed")]
        active_tests = [k[1] for k in snap["current"] if k[0] == "test"]
        try:
            counts = inventory.summary(inventory.containers(db, config.unit_prefix))
        except docker_cli.DockerError:
            counts = {}
        return {
            "host": snap["host"], "storage": storage,
            "unhealthy_deployments": unhealthy, "active_tests": active_tests,
            "container_counts": counts, "alerts": sampler.alerts.current(),
            "sampling": {"cpu_memory_seconds": 15, "storage_seconds": 300,
                         "aggregate": "1 minute", "retention_days": 30,
                         "stored_minutes": metrics_store.table_size(db)},
        }

    def repositories(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        if args:
            raise ProtocolError("args_invalid", f"unexpected args: {sorted(args)}")
        snap = sampler.snapshot()
        rows = []
        for repo in registry.list_repositories():
            rid = repo["repository_id"]
            live = snap["current"].get(("repository", rid), {})
            storage = snap["storage"].get(("repository", rid), {})
            deployments = [dict(r) for r in db.query(
                "SELECT deployment_id, name, source, state FROM deployments"
                " WHERE repository_id=?", (rid,))]
            health = "healthy"
            if any(d["state"] in ("degraded", "failed") for d in deployments):
                health = "unhealthy"
            elif not deployments:
                health = "none"
            rows.append({
                "repository_id": rid, "display_name": repo["display_name"],
                "root_path": repo["root_path"],
                "cpu_percent": live.get("cpu_percent", 0.0),
                "memory_bytes": live.get("memory_bytes", 0),
                "storage_bytes": storage.get("total"), "storage": storage,
                "health": health, "deployments": deployments,
                "trend_cpu": metrics_store.trend(db, "repository", rid, "cpu_percent"),
                "trend_memory": metrics_store.trend(db, "repository", rid, "memory_bytes"),
            })
        daemon = snap["current"].get(("daemon", "daemon"), {})
        other = snap["current"].get(("other", "other"), {})
        return {"repositories": rows,
                "devcoordinator": {"cpu_percent": daemon.get("cpu_percent", 0.0),
                                   "memory_bytes": daemon.get("memory_bytes", 0),
                                   "storage_bytes": snap["storage"].get(
                                       ("host", "storage"), {}).get("devcoordinator_state")},
                "shared_unattributed": {"cpu_percent": other.get("cpu_percent", 0.0),
                                        "memory_bytes": other.get("memory_bytes", 0),
                                        "storage": {k: v for k, v in snap["storage"].get(
                                            ("host", "storage"), {}).items()
                                            if k.startswith("docker") or k == "other"}},
                "host": snap["host"]}

    def repository(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        unknown = set(args) - {"path"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        raw = args.get("path")
        if not isinstance(raw, str) or not Path(raw).is_absolute():
            raise ProtocolError("args_invalid", "'path' (absolute) is required")
        status = registry.repository_status(Path(raw), run_as=(caller.uid, caller.gid))
        if status is None:
            raise ProtocolError("repository_not_found", f"no registered repository at {raw}")
        rid = status["repository_id"]
        snap = sampler.snapshot()
        components = []
        for key, sample in snap["current"].items():
            meta = snap["meta"].get(key, {})
            if meta.get("repository_id") != rid:
                continue
            components.append({"kind": key[0], "id": key[1], **{k: v for k, v in meta.items()
                               if k not in ("kind", "id", "repository_id")},
                               **sample,
                               "storage": snap["storage"].get(key, {})})
        return {"repository_id": rid, "display_name": status["display_name"],
                "live": snap["current"].get(("repository", rid), {}),
                "storage": snap["storage"].get(("repository", rid), {}),
                "components": components}

    def history(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        unknown = set(args) - {"subject_kind", "subject_id", "metric", "minutes"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        kind, sid, metric = args.get("subject_kind"), args.get("subject_id"), args.get("metric")
        if kind not in SUBJECT_KINDS or not isinstance(sid, str) or metric not in METRICS:
            raise ProtocolError("args_invalid", "subject_kind/subject_id/metric invalid")
        minutes = args.get("minutes", 60)
        if not isinstance(minutes, int) or not (1 <= minutes <= 60 * 24 * 30):
            raise ProtocolError("args_invalid", "'minutes' must be 1..43200")
        points = metrics_store.series(db, kind, sid, metric, minutes)
        return {"subject_kind": kind, "subject_id": sid, "metric": metric,
                "minutes": minutes, "points": points[-1440:],
                "truncated": len(points) > 1440}

    def containers(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        if args:
            raise ProtocolError("args_invalid", f"unexpected args: {sorted(args)}")
        try:
            rows = inventory.containers(db, config.unit_prefix)
        except docker_cli.DockerError as exc:
            raise ProtocolError("internal_error", f"docker unavailable: {exc}") from exc
        snap = sampler.snapshot()
        for row in rows:
            live = snap["current"].get(("container", row["id"]), {})
            storage = snap["storage"].get(("component", f"{row['deployment_id']}/"
                                                        f"{row['component']}"), {})
            row["cpu_percent"] = live.get("cpu_percent")
            row["memory_bytes"] = live.get("memory_bytes")
            row["pids"] = live.get("pids")
            row["container_layer_bytes"] = storage.get("container_layer")
        return {"containers": rows, "counts": inventory.summary(rows)}

    def container_remove(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        """Explicit removal of one exact DevCoordinator-owned ephemeral
        container (orphaned managed or managed test). Unmanaged containers
        and permanent deployment containers are never removable here."""
        unknown = set(args) - {"container_id"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        cid = args.get("container_id")
        if not isinstance(cid, str) or len(cid) != 64:
            raise ProtocolError("args_invalid", "'container_id' must be a full 64-hex id")
        try:
            rows = inventory.containers(db, config.unit_prefix)
        except docker_cli.DockerError as exc:
            raise ProtocolError("internal_error", f"docker unavailable: {exc}") from exc
        row = next((r for r in rows if r["id"] == cid), None)
        if row is None:
            raise ProtocolError("args_invalid", "no such container")
        if row["classification"] not in ("orphaned-managed", "managed-test"):
            raise ProtocolError("permission_denied",
                                f"{row['classification']} containers are not removed by"
                                " DevCoordinator; decide manually")
        try:
            docker_cli.remove_exact(cid)
        except docker_cli.DockerError as exc:
            raise ProtocolError("internal_error", str(exc)) from exc
        return {"container_id": cid, "removed": True,
                "classification": row["classification"]}

    return {"health.summary": summary, "health.repositories": repositories,
            "health.repository": repository, "health.history": history,
            "health.containers": containers, "health.container_remove": container_remove}
