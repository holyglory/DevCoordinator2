"""Current Docker inventory: every container on the host, classified by
daemon-owned labels and recorded bindings — never by name, port, image, or
path heuristics. Unlabeled containers are honestly unmanaged/unknown."""

from __future__ import annotations

import json

from devcoordinator2.daemon import docker_cli
from devcoordinator2.daemon.db import Database

CLASSES = ("managed-test", "managed-preview", "managed-permanent",
           "orphaned-managed", "unmanaged")


def _all_containers() -> list[dict]:
    proc = docker_cli._run(["ps", "--all", "--no-trunc", "--format", "{{json .}}"],
                           timeout=30)
    if proc.returncode != 0:
        raise docker_cli.DockerError(proc.stderr.strip()[:512] or "docker ps failed")
    rows = []
    for line in proc.stdout.splitlines():
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows


def _parse_labels(raw: str) -> dict[str, str]:
    labels: dict[str, str] = {}
    for item in raw.split(","):
        if "=" in item:
            key, _, value = item.partition("=")
            labels[key] = value
    return labels


def containers(db: Database, instance: str) -> list[dict]:
    prefix = docker_cli.LABEL_PREFIX
    known_components = {
        (r["deployment_id"], r["name"]): r for r in db.query(
            "SELECT deployment_id, name, binding_identity FROM components")}
    compose_projects = {r["binding_identity"] for r in db.query(
        "SELECT binding_identity FROM components WHERE binding_kind='compose'")}
    deployments = {r["deployment_id"]: r for r in db.query(
        "SELECT deployment_id, repository_id, name, source, ttl_expires_at FROM deployments")}
    out = []
    for c in _all_containers():
        labels = _parse_labels(c.get("Labels", ""))
        entry = {
            "id": c.get("ID"), "name": c.get("Names"), "image": c.get("Image"),
            "state": c.get("State"), "status": c.get("Status"),
            "created": c.get("CreatedAt"), "repository_id": None, "deployment_id": None,
            "component": None, "run_id": None, "caller_uid": None, "client": None,
            "ttl_seconds": None, "data": None, "classification": "unmanaged",
        }
        purpose = labels.get(f"{prefix}.purpose")
        compose_project = labels.get("com.docker.compose.project")
        if labels.get(f"{prefix}.instance") == instance and purpose:
            entry.update(
                repository_id=labels.get(f"{prefix}.repository"),
                deployment_id=labels.get(f"{prefix}.deployment"),
                component=labels.get(f"{prefix}.component"),
                run_id=labels.get(f"{prefix}.run"),
                caller_uid=_int(labels.get(f"{prefix}.caller_uid")),
                client=labels.get(f"{prefix}.client"),
                ttl_seconds=_int(labels.get(f"{prefix}.ttl_seconds")),
                data=labels.get(f"{prefix}.data"),
            )
            if purpose == "test":
                entry["classification"] = "managed-test"
            else:
                key = (entry["deployment_id"], entry["component"])
                row = known_components.get(key)
                if row is not None and row["binding_identity"] == entry["id"]:
                    entry["classification"] = ("managed-preview"
                                               if purpose == "preview" else "managed-permanent")
                else:
                    entry["classification"] = "orphaned-managed"
        elif compose_project in compose_projects:
            dep = next((d for d in deployments.values()
                        if compose_project.startswith(f"dc2-{d['deployment_id']}-")), None)
            entry.update(deployment_id=dep["deployment_id"] if dep else None,
                         repository_id=dep["repository_id"] if dep else None,
                         component=compose_project.rsplit("-", 1)[-1],
                         classification="managed-permanent" if dep else "orphaned-managed")
        out.append(entry)
    return out


def _int(value: str | None) -> int | None:
    try:
        return int(value) if value is not None else None
    except ValueError:
        return None


def summary(rows: list[dict]) -> dict[str, int]:
    counts = dict.fromkeys(CLASSES, 0)
    for r in rows:
        counts[r["classification"]] = counts.get(r["classification"], 0) + 1
    return counts
