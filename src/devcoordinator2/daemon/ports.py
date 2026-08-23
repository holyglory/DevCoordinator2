"""Port leases: unique host ports from the instance range, recorded in the
authority database and proven free on the host before assignment."""

from __future__ import annotations

import socket
from datetime import UTC, datetime

from devcoordinator2.daemon.db import Database


class PortExhausted(Exception):
    pass


def _bindable(port: int) -> bool:
    for family, addr in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET, "0.0.0.0")):
        with socket.socket(family, socket.SOCK_STREAM) as s:
            try:
                s.bind((addr, port))
            except OSError:
                return False
    return True


def lease(db: Database, port_range: tuple[int, int], deployment_id: str,
          component: str, generation: int) -> int:
    """Assign the lowest free port in range, transactionally unique."""
    now = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    with db.transaction() as conn:
        taken = {r["port"] for r in conn.execute(
            "SELECT port FROM port_assignments").fetchall()}
        for port in range(port_range[0], port_range[1] + 1):
            if port in taken or not _bindable(port):
                continue
            conn.execute(
                "INSERT INTO port_assignments(port, deployment_id, component,"
                " generation, assigned_at) VALUES(?,?,?,?,?)",
                (port, deployment_id, component, generation, now))
            return port
    raise PortExhausted(f"no free port in {port_range[0]}-{port_range[1]}")


def release(db: Database, deployment_id: str, generation: int | None = None,
            component: str | None = None) -> None:
    sql = "DELETE FROM port_assignments WHERE deployment_id=?"
    params: list = [deployment_id]
    if generation is not None:
        sql += " AND generation=?"
        params.append(generation)
    if component is not None:
        sql += " AND component=?"
        params.append(component)
    with db.transaction() as conn:
        conn.execute(sql, params)


def assigned(db: Database, deployment_id: str, generation: int) -> dict[str, int]:
    rows = db.query(
        "SELECT component, port FROM port_assignments WHERE deployment_id=?"
        " AND generation=?", (deployment_id, generation))
    return {r["component"]: r["port"] for r in rows}
