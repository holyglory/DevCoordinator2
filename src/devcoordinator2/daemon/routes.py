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


def publish(db: Database, path: Path, base_domain: str) -> dict:
    """Snapshot current domain_routes with a port and write atomically."""
    rows = db.query(
        "SELECT domain, deployment_id, component, port, generation FROM domain_routes"
        " WHERE port IS NOT NULL ORDER BY domain")
    routes = []
    for r in rows:
        fqdn = f"{r['domain']}.{base_domain}" if base_domain else r["domain"]
        routes.append({
            "deployment_id": r["deployment_id"], "component": r["component"],
            "label": r["domain"], "domain": fqdn, "port": r["port"],
            "scheme": "http", "auth": "authenticated",
            "generation": r["generation"],
        })
    payload = {
        "generation": _next_generation(db),
        "published_at": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "domain": base_domain,
        "routes": routes,
        "access": {"owners": [], "grants": []},
    }
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    document = {"schema": ROUTE_SCHEMA,
                "payload_sha256": hashlib.sha256(canonical).hexdigest(),
                **payload}
    data = json.dumps(document, indent=2).encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".routes-")
    try:
        os.write(fd, data)
        os.fsync(fd)
        os.fchmod(fd, 0o644)
    finally:
        os.close(fd)
    os.replace(tmp, path)
    return document
