"""Deployment durable state: DB rows, private secrets, environment files."""

from __future__ import annotations

import hashlib
import json
import os
import secrets as pysecrets
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.deploy_config import ComponentSpec, DeploymentSpec
from devcoordinator2.paths import InstanceConfig


def now_iso() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def deployment_id(worktree_id: str, name: str, source: str) -> str:
    raw = f"devcoordinator2.deployment\0{worktree_id}\0{name}\0{source}".encode()
    return "d" + hashlib.sha256(raw).hexdigest()[:16]


def fingerprint(obj) -> str:
    return hashlib.sha256(
        json.dumps(obj, sort_keys=True, separators=(",", ":")).encode()).hexdigest()[:24]


def component_fingerprint(spec: ComponentSpec) -> str:
    data = {k: (list(v) if isinstance(v, tuple) else
                (v.__dict__ if hasattr(v, "__dict__") else v))
            for k, v in sorted(spec.__dict__.items())}
    return fingerprint(data)


def is_generation_scoped(spec: ComponentSpec) -> bool:
    """Blue/green per generation; stable components live across generations."""
    return spec.type == "process" or (spec.type == "docker" and not spec.volumes)


def is_owned(spec: ComponentSpec) -> bool:
    return spec.type != "external" and not (spec.type == "postgres" and spec.shared_from)


# -- rows --------------------------------------------------------------------

def get_deployment(db: Database, dep_id: str) -> dict | None:
    rows = db.query("SELECT * FROM deployments WHERE deployment_id=?", (dep_id,))
    return dict(rows[0]) if rows else None


def find_deployment(db: Database, worktree_id: str, name: str,
                    source: str) -> dict | None:
    rows = db.query("SELECT * FROM deployments WHERE worktree_id=? AND name=? AND source=?",
                    (worktree_id, name, source))
    return dict(rows[0]) if rows else None


def list_deployments(db: Database, repository_id: str | None = None) -> list[dict]:
    if repository_id:
        rows = db.query("SELECT * FROM deployments WHERE repository_id=? ORDER BY name, source",
                        (repository_id,))
    else:
        rows = db.query("SELECT * FROM deployments ORDER BY repository_id, name, source")
    return [dict(r) for r in rows]


def components(db: Database, dep_id: str) -> list[dict]:
    return [dict(r) for r in db.query(
        "SELECT * FROM components WHERE deployment_id=? ORDER BY order_index", (dep_id,))]


def upsert_deployment(db: Database, *, dep_id: str, reg, name: str, source: str,
                      domain: str | None, spec: DeploymentSpec, spec_fp: str,
                      state: str, caller_uid: int, client: str,
                      ttl_expires_at: str | None) -> None:
    now = now_iso()
    with db.transaction() as conn:
        existing = conn.execute("SELECT deployment_id FROM deployments WHERE deployment_id=?",
                                (dep_id,)).fetchone()
        if existing is None:
            conn.execute(
                "INSERT INTO deployments(deployment_id, repository_id, worktree_id, name,"
                " source, domain, spec_fingerprint, spec_json, state, current_generation,"
                " previous_generation, created_at, created_by_uid, client, updated_at,"
                " ttl_expires_at) VALUES(?,?,?,?,?,?,?,?,?,NULL,NULL,?,?,?,?,?)",
                (dep_id, reg.repository_id, reg.worktree_id, name, source, domain,
                 spec_fp, json.dumps(spec.canonical(source)), state, now, caller_uid,
                 client, now, ttl_expires_at))
        else:
            conn.execute(
                "UPDATE deployments SET domain=?, spec_fingerprint=?, spec_json=?, state=?,"
                " updated_at=?, ttl_expires_at=? WHERE deployment_id=?",
                (domain, spec_fp, json.dumps(spec.canonical(source)), state, now,
                 ttl_expires_at, dep_id))
        conn.execute("UPDATE deployments SET public=? WHERE deployment_id=?",
                     (int(spec.public), dep_id))
        for cspec in spec.components:
            conn.execute(
                "INSERT INTO components(deployment_id, name, type, order_index,"
                " spec_fingerprint, desired_state, state, health, generation,"
                " binding_kind, binding_identity, restarts, last_error, updated_at)"
                " VALUES(?,?,?,?,?,'running','unknown','unknown',NULL,NULL,NULL,0,NULL,?)"
                " ON CONFLICT(deployment_id, name) DO UPDATE SET type=excluded.type,"
                " order_index=excluded.order_index, updated_at=excluded.updated_at",
                (dep_id, cspec.name, cspec.type, cspec.order,
                 component_fingerprint(cspec), now))
        names = [c.name for c in spec.components]
        conn.execute(
            f"DELETE FROM components WHERE deployment_id=? AND name NOT IN"
            f" ({','.join('?' * len(names))})", (dep_id, *names))


def set_deployment(db: Database, dep_id: str, **fields) -> None:
    fields["updated_at"] = now_iso()
    cols = ", ".join(f"{k}=?" for k in fields)
    with db.transaction() as conn:
        conn.execute(f"UPDATE deployments SET {cols} WHERE deployment_id=?",
                     (*fields.values(), dep_id))


def set_component(db: Database, dep_id: str, name: str, **fields) -> None:
    fields["updated_at"] = now_iso()
    cols = ", ".join(f"{k}=?" for k in fields)
    with db.transaction() as conn:
        conn.execute(f"UPDATE components SET {cols} WHERE deployment_id=? AND name=?",
                     (*fields.values(), dep_id, name))


def add_generation(db: Database, dep_id: str, number: int, commit: str | None,
                   dirty: bool, path: Path, fp: str) -> None:
    with db.transaction() as conn:
        conn.execute(
            "INSERT OR REPLACE INTO generations(deployment_id, number, commit_hash, dirty,"
            " path, fingerprint, created_at, state) VALUES(?,?,?,?,?,?,?,'candidate')",
            (dep_id, number, commit, int(dirty), str(path), fp, now_iso()))


def generation(db: Database, dep_id: str, number: int) -> dict | None:
    rows = db.query("SELECT * FROM generations WHERE deployment_id=? AND number=?",
                    (dep_id, number))
    return dict(rows[0]) if rows else None


def set_generation_state(db: Database, dep_id: str, number: int, state: str) -> None:
    with db.transaction() as conn:
        conn.execute("UPDATE generations SET state=? WHERE deployment_id=? AND number=?",
                     (state, dep_id, number))


def prune_generations(db: Database, dep_id: str, keep: set[int]) -> list[dict]:
    rows = [dict(r) for r in db.query("SELECT * FROM generations WHERE deployment_id=?",
                                      (dep_id,))]
    stale = [r for r in rows if r["number"] not in keep]
    with db.transaction() as conn:
        for r in stale:
            conn.execute("DELETE FROM generations WHERE deployment_id=? AND number=?",
                         (dep_id, r["number"]))
    return stale


def set_route(db: Database, domain: str | None, dep_id: str, component: str | None,
              port: int | None, generation_number: int | None) -> None:
    with db.transaction() as conn:
        conn.execute("DELETE FROM domain_routes WHERE deployment_id=?", (dep_id,))
        if domain:
            conn.execute(
                "INSERT OR REPLACE INTO domain_routes(domain, deployment_id, component, port,"
                " generation, published_at) VALUES(?,?,?,?,?,?)",
                (domain, dep_id, component, port, generation_number, now_iso()))


def effective_domain(row: dict | None, spec: DeploymentSpec, source: str) -> str | None:
    """An administrator's override (deployment.set_domain) wins over the
    repository-declared domain until cleared."""
    if row and row.get("domain_override"):
        return row["domain_override"]
    return spec.domain_for(source)


def override_domain(db: Database, dep_id: str, domain: str | None) -> dict:
    """Persist a domain override (or clear it, falling back to the declared
    domain) and move the live route with it. Uniqueness is the caller's check."""
    row = get_deployment(db, dep_id)
    if row is None:
        raise ValueError(f"no deployment {dep_id}")
    declared = json.loads(row["spec_json"]).get("domain")
    effective = domain if domain is not None else declared
    with db.transaction() as conn:
        conn.execute("UPDATE deployments SET domain_override=?, domain=?, updated_at=?"
                     " WHERE deployment_id=?", (domain, effective, now_iso(), dep_id))
        route = conn.execute("SELECT * FROM domain_routes WHERE deployment_id=?",
                             (dep_id,)).fetchone()
        if route is not None:
            if effective:
                conn.execute("UPDATE domain_routes SET domain=?, published_at=?"
                             " WHERE deployment_id=?", (effective, now_iso(), dep_id))
            else:
                conn.execute("DELETE FROM domain_routes WHERE deployment_id=?", (dep_id,))
    if route is None and effective:
        _create_route_from_spec(db, dep_id, row, effective)
    updated = get_deployment(db, dep_id)
    return {"deployment_id": dep_id, "domain": updated["domain"],
            "domain_source": "override" if domain is not None else "configuration",
            "declared_domain": declared}


def _create_route_from_spec(db: Database, dep_id: str, row: dict,
                            domain: str) -> None:
    spec = json.loads(row["spec_json"])
    route_comp = next((c["name"] for c in spec.get("components", []) if c.get("route")),
                      None)
    if route_comp is None:
        raise ValueError("this deployment declares no route component; add"
                         " route = true to the component that should receive traffic"
                         " and apply first")
    generation = row["current_generation"] or 0
    port_rows = db.query(
        "SELECT port, generation FROM port_assignments WHERE deployment_id=?"
        " AND component=? AND generation IN (?, 0) ORDER BY generation DESC",
        (dep_id, route_comp, generation))
    port = port_rows[0]["port"] if port_rows else None
    set_route(db, domain, dep_id, route_comp, port, generation or None)


def domain_owner(db: Database, domain: str) -> str | None:
    rows = db.query("SELECT deployment_id FROM domain_routes WHERE domain=?", (domain,))
    if rows:
        return rows[0]["deployment_id"]
    observed = db.query(
        "SELECT observed_deployment_id FROM observed_routes WHERE domain=?", (domain,))
    return observed[0]["observed_deployment_id"] if observed else None


def delete_deployment_rows(db: Database, dep_id: str) -> None:
    with db.transaction() as conn:
        for table in ("domain_routes", "port_assignments", "components", "generations"):
            conn.execute(f"DELETE FROM {table} WHERE deployment_id=?", (dep_id,))
        conn.execute("DELETE FROM deployments WHERE deployment_id=?", (dep_id,))


# -- private secrets and environment files ----------------------------------

def postgres_credentials(config: InstanceConfig, dep_id: str, component: str,
                         user: str, database: str) -> dict[str, str]:
    """Generated once per dedicated instance; root-only 0600 file."""
    directory = config.secrets_dir / dep_id
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    path = directory / f"{component}.json"
    if path.exists():
        data = json.loads(path.read_text())
        if data.get("user") == user and data.get("database") == database:
            return data
    data = {"user": user, "database": database,
            "password": pysecrets.token_urlsafe(24)}
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        os.write(fd, json.dumps(data).encode())
    finally:
        os.close(fd)
    return data


def read_postgres_credentials(config: InstanceConfig, dep_id: str,
                              component: str) -> dict[str, str] | None:
    path = config.secrets_dir / dep_id / f"{component}.json"
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def delete_secrets(config: InstanceConfig, dep_id: str) -> None:
    directory = config.secrets_dir / dep_id
    if directory.is_dir():
        for child in directory.iterdir():
            child.unlink()
        directory.rmdir()


def write_env_file(path: Path, env: dict[str, str], owner: tuple[int, int],
                   fmt: str = "systemd") -> None:
    """systemd EnvironmentFile (quoted, escaped) or docker --env-file
    (literal KEY=value; docker/compose do not strip quotes)."""
    lines = []
    for key, value in env.items():
        if "\n" in value or "\r" in value:
            raise ValueError(f"environment value for {key} must be a single line")
        if fmt == "systemd":
            escaped = value.replace("\\", "\\\\").replace('"', '\\"')
            lines.append(f'{key}="{escaped}"')
        else:
            lines.append(f"{key}={value}")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        os.write(fd, ("\n".join(lines) + "\n").encode())
        os.fchmod(fd, 0o600)
        os.fchown(fd, owner[0], owner[1])
    finally:
        os.close(fd)
