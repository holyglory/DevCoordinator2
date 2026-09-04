"""Content-free PostgreSQL operational facts for dedicated instances.

Queries run inside the container over the local socket (the official image
trusts local connections) and never touch query text, row values, or
credentials. Results are numbers only."""

from __future__ import annotations

from devcoordinator2.daemon import docker_cli

_SQL = (
    "select (select count(*) from pg_stat_activity),"
    " (select coalesce(sum(size),0) from pg_ls_waldir()),"
    " (select coalesce(sum(temp_bytes),0) from pg_stat_database),"
    " (select coalesce(sum(pg_database_size(datname)),0) from pg_database"
    "  where not datistemplate)"
)


def facts(container_id: str, user: str, database: str) -> dict[str, int] | None:
    proc = docker_cli._run(["exec", container_id, "psql", "-U", user, "-d", database,
                            "-tA", "-F", "|", "-c", _SQL], timeout=20)
    if proc.returncode != 0:
        return None
    try:
        conns, wal, temp, dbsize = proc.stdout.strip().split("|")
        return {"pg_connections": int(conns), "pg_wal_bytes": int(wal),
                "pg_temp_bytes": int(temp), "pg_database_bytes": int(dbsize)}
    except ValueError:
        return None
