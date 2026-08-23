"""Atomic route-document publication (docs/route-document.md).

The daemon owns desired route state; every publication is one complete
snapshot written atomically. The edge (Phase 5) serves from the last valid
document across daemon restarts.
"""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2.daemon.db import Database

ROUTE_SCHEMA = 1


def _next_generation(db: Database) -> int:
    with db.transaction() as conn:
        row = conn.execute("SELECT value FROM meta WHERE key='route_generation'").fetchone()
        generation = (int(row["value"]) if row else 0) + 1
        conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES('route_generation', ?)",
                     (str(generation),))
    return generation


def publish(db: Database, path: Path, base_domain: str,
            access: dict | None = None) -> dict:
    """Snapshot current domain_routes with a port and write atomically."""
    rows = db.query(
        "SELECT r.domain, r.deployment_id, r.component, r.port, r.generation,"
        " d.public FROM domain_routes r JOIN deployments d"
        " ON d.deployment_id = r.deployment_id WHERE r.port IS NOT NULL ORDER BY r.domain")
    routes = []
    for r in rows:
        fqdn = f"{r['domain']}.{base_domain}" if base_domain else r["domain"]
        routes.append({
            "deployment_id": r["deployment_id"], "component": r["component"],
            "label": r["domain"], "domain": fqdn, "port": r["port"],
            "scheme": "http", "auth": "public" if r["public"] else "authenticated",
            "generation": r["generation"],
        })
    if access is None:
        owners = [u["email"] for u in db.query(
            "SELECT email FROM users WHERE administrator=1 ORDER BY email")]
        grants = [{"identity": g["email"], "deployment_id": g["deployment_id"],
                   "role": g["role"]} for g in db.query(
            "SELECT u.email, g.deployment_id, g.role FROM grants g JOIN users u"
            " ON u.user_id=g.user_id ORDER BY u.email, g.deployment_id")]
        access = {"owners": owners, "grants": grants}
    payload = {
        "generation": _next_generation(db),
        "published_at": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "domain": base_domain,
        "routes": routes,
        "access": access,
    }
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"),
                           ensure_ascii=False).encode()
    document = {"schema": ROUTE_SCHEMA,
                "payload_sha256": hashlib.sha256(canonical).hexdigest(),
                **payload}
    data = json.dumps(document, indent=2).encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    os.chmod(path.parent, 0o755)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".routes-")
    try:
        os.write(fd, data)
        os.fsync(fd)
        os.fchmod(fd, 0o644)
    finally:
        os.close(fd)
    os.replace(tmp, path)
    return document
