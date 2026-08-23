"""Test-scoped ephemeral PostgreSQL (one throwaway instance per run).

The container carries the exact test identity in daemon-owned labels, keeps
its data on tmpfs (disposable by construction), publishes on loopback only,
and is removed on completion, timeout, cancellation, supersession, daemon
recovery, or the next start. Credentials are generated per run and reach
only the container and the test process environment — never summaries,
logs, metrics, or agent results.
"""

from __future__ import annotations

import secrets
import time
from dataclasses import dataclass

from devcoordinator2.daemon import docker_cli
from devcoordinator2.daemon.repoconfig import PostgresSpec

READY_TIMEOUT_SECONDS = 90.0
_PGDATA_TMPFS = "/var/lib/postgresql/data:rw,size=1g,mode=0700"


@dataclass(frozen=True)
class EphemeralPostgres:
    container_id: str
    host: str
    port: int
    user: str
    database: str
    password: str

    def env(self) -> dict[str, str]:
        url = (f"postgresql://{self.user}:{self.password}@{self.host}:{self.port}"
               f"/{self.database}")
        return {
            "PGHOST": self.host, "PGPORT": str(self.port), "PGUSER": self.user,
            "PGPASSWORD": self.password, "PGDATABASE": self.database,
            "DATABASE_URL": url,
        }


def provision(spec: PostgresSpec, *, run_id: str,
              labels: dict[str, str]) -> EphemeralPostgres:
    """Create and wait for readiness; on any failure remove what was made."""
    if not docker_cli.available():
        raise docker_cli.DockerError("docker is unavailable to the daemon")
    password = secrets.token_urlsafe(24)
    name = f"devcoordinator2-test-{run_id[1:]}-postgres"
    container_id = docker_cli.run_detached(
        name=name, image=spec.image, labels=labels,
        env_names=["POSTGRES_USER", "POSTGRES_PASSWORD", "POSTGRES_DB"],
        env_values={"POSTGRES_USER": spec.user, "POSTGRES_PASSWORD": password,
                    "POSTGRES_DB": spec.database},
        publish=["127.0.0.1::5432"],
        tmpfs=[_PGDATA_TMPFS],
    )
    try:
        port = docker_cli.published_host_port(container_id, "5432/tcp")
        _wait_ready(container_id, spec)
    except docker_cli.DockerError:
        docker_cli.remove_exact(container_id)
        raise
    return EphemeralPostgres(container_id=container_id, host="127.0.0.1",
                             port=port, user=spec.user, database=spec.database,
                             password=password)


def _wait_ready(container_id: str, spec: PostgresSpec) -> None:
    deadline = time.monotonic() + READY_TIMEOUT_SECONDS
    # During first-run init the official image serves a temporary server on
    # the Unix socket only, then restarts listening on TCP. Probing over TCP
    # (and requiring two consecutive successes) avoids racing that restart.
    consecutive = 0
    while time.monotonic() < deadline:
        if docker_cli.exec_ok(container_id,
                              ["pg_isready", "-h", "127.0.0.1", "-U", spec.user,
                               "-d", spec.database]):
            consecutive += 1
            if consecutive >= 2:
                return
        else:
            consecutive = 0
        time.sleep(0.5)
    raise docker_cli.DockerError("ephemeral postgres did not become ready")
