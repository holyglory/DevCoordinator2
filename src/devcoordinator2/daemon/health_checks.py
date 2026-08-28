"""Component health probes: bounded, content-free, never log bodies."""

from __future__ import annotations

import socket
import time
import urllib.error
import urllib.request
from collections.abc import Callable

from devcoordinator2.daemon import docker_cli


def http_ready(port: int, path: str, timeout_seconds: int,
               abort: Callable[[], str | None] | None = None) -> tuple[bool, str]:
    deadline = time.monotonic() + timeout_seconds
    last = "no response"
    url = f"http://127.0.0.1:{port}{path}"
    while time.monotonic() < deadline:
        reason = abort() if abort else None
        if reason:
            return False, reason
        try:
            with urllib.request.urlopen(url, timeout=5) as resp:
                if 200 <= resp.status < 400:
                    return True, f"http {resp.status}"
                last = f"http {resp.status}"
        except urllib.error.HTTPError as exc:
            last = f"http {exc.code}"
        except (urllib.error.URLError, OSError, ValueError) as exc:
            last = f"unreachable: {getattr(exc, 'reason', exc)}"
        time.sleep(0.5)
    return False, last


def tcp_ready(host: str, port: int, timeout_seconds: int,
              abort: Callable[[], str | None] | None = None) -> tuple[bool, str]:
    deadline = time.monotonic() + timeout_seconds
    last = "connection refused"
    while time.monotonic() < deadline:
        reason = abort() if abort else None
        if reason:
            return False, reason
        try:
            with socket.create_connection((host, port), timeout=3):
                return True, "tcp open"
        except OSError as exc:
            last = f"tcp closed: {exc.strerror or exc}"
        time.sleep(0.5)
    return False, last


def tcp_probe(host: str, port: int) -> tuple[bool, str]:
    return tcp_ready(host, port, 3)


def container_running(container_id: str) -> tuple[bool, str]:
    try:
        info = docker_cli.inspect(container_id)
    except docker_cli.DockerError as exc:
        return False, f"missing: {exc}"
    state = info.get("State") or {}
    status = state.get("Status", "unknown")
    health = (state.get("Health") or {}).get("Status")
    if status != "running":
        return False, f"container {status}"
    if health and health != "healthy":
        return health == "starting", f"container running, health {health}"
    return True, "container running"


def postgres_ready(container_id: str, user: str, database: str,
                   timeout_seconds: int) -> tuple[bool, str]:
    deadline = time.monotonic() + timeout_seconds
    consecutive = 0
    while time.monotonic() < deadline:
        if docker_cli.exec_ok(container_id, ["pg_isready", "-h", "127.0.0.1",
                                             "-U", user, "-d", database]):
            consecutive += 1
            if consecutive >= 2:
                return True, "accepting connections"
        else:
            consecutive = 0
        time.sleep(0.5)
    return False, "pg_isready never succeeded"
