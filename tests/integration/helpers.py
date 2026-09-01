"""Shared helpers for root-required integration tests (real daemon subprocess,

real systemd units, real Docker).

Run with:
    sudo DEVCOORDINATOR2_ROOT_TESTS=1 .venv/bin/pytest tests/integration -v

A real daemon subprocess serves a private socket; requests are made from a
non-root uid (SUDO_UID or nobody) via fork+setuid so SO_PEERCRED is honest.
Units use a dedicated prefix so nothing touches real instances.
"""

from __future__ import annotations

import json
import os
import pwd
import signal
import subprocess
import sys
import time
import uuid
from pathlib import Path

import pytest

from devcoordinator2.daemon.test_admission import DirectoryEvents
from devcoordinator2.ids import repository_id

ROOT_ONLY = pytest.mark.skipif(
    os.geteuid() != 0 or not os.environ.get("DEVCOORDINATOR2_ROOT_TESTS"),
    reason="requires root and DEVCOORDINATOR2_ROOT_TESTS=1",
)

REPO_ROOT = Path(__file__).resolve().parents[2]
# A fresh pytest process owns a disjoint host namespace, so independent root
# modules or separate governed invocations can overlap without unit/container
# collisions. Tests inside one process still share their fixture deliberately.
_PROCESS_NAMESPACE = str(os.getpid())
UNIT_PREFIX = f"devcoordinator2-inttest-{_PROCESS_NAMESPACE}"
DEPLOY_PREFIX = f"devcoordinator2-inttest-deploy-{_PROCESS_NAMESPACE}"
_REQUEST_AS = """
import socket
import sys
request = sys.stdin.buffer.readline()
if not request.endswith(b"\\n"):
    raise SystemExit(2)
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
    connection.settimeout(900)
    connection.connect(sys.argv[1])
    connection.sendall(request)
    connection.shutdown(socket.SHUT_WR)
    while True:
        block = connection.recv(65536)
        if not block:
            break
        sys.stdout.buffer.write(block)
"""


def _caller() -> pwd.struct_passwd:
    sudo_uid = os.environ.get("SUDO_UID")
    if sudo_uid and int(sudo_uid) != 0:
        return pwd.getpwuid(int(sudo_uid))
    return pwd.getpwnam("nobody")


def call_as(uid: int, gid: int, sock_path: Path, request: dict) -> dict:
    """Send one request through a separately exec'd credential boundary.

    setpriv performs the uid/gid transition before a fresh interpreter starts,
    so this remains safe when the pytest process already owns threads.
    """
    proc = subprocess.run(
        ["setpriv", f"--reuid={uid}", f"--regid={gid}", "--clear-groups", "--",
         "/usr/bin/python3", "-c", _REQUEST_AS, str(sock_path)],
        input=json.dumps(request) + "\n", capture_output=True, text=True,
        timeout=900, check=False,
    )
    assert proc.returncode == 0, (
        f"credential helper failed; stderr={proc.stderr[:1024]!r}; "
        f"stdout={proc.stdout[:1024]!r}")
    return json.loads(proc.stdout)


def _request(command: str, args: dict | None = None) -> dict:
    return {"protocol": 1, "id": uuid.uuid4().hex[:8], "command": command,
            "args": args or {}, "client": {"kind": "other"}}


class Daemon:
    def __init__(self, base: Path, compose_allowlist: Path | None = None):
        self.base = base
        self.socket_path = base / "daemon.sock"
        # Strip sudo markers: git special-cases SUDO_UID, which masked the
        # safe.directory "dubious ownership" failure an installed daemon
        # (no sudo env) would hit. The daemon must work without them.
        clean_env = {k: v for k, v in os.environ.items()
                     if not k.startswith("SUDO_")}
        self.env = {
            **clean_env,
            "PYTHONPATH": str(REPO_ROOT / "src"),
            "DEVCOORDINATOR2_SOCKET": str(self.socket_path),
            "DEVCOORDINATOR2_STATE_DIR": str(base / "state"),
            "DEVCOORDINATOR2_UNIT_PREFIX": UNIT_PREFIX,
            "DEVCOORDINATOR2_SLICE": "devcoordinator2-tests.slice",
            "DEVCOORDINATOR2_CLIENT_GROUP": "",
            "DEVCOORDINATOR2_INSTANCE_ENV": "/nonexistent",
        }
        if compose_allowlist is not None:
            self.env["DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE"] = str(
                compose_allowlist)
        self.proc: subprocess.Popen | None = None

    def start(self):
        # A stale socket from a killed daemon would make readiness detection
        # lie; the new daemon re-creates it on bind.
        self.socket_path.unlink(missing_ok=True)
        with DirectoryEvents(self.base) as events:
            self.proc = subprocess.Popen(
                [sys.executable, "-m", "devcoordinator2.daemon"],
                env=self.env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            )
            deadline = time.monotonic() + 15
            while True:
                if self.socket_path.exists():
                    os.chmod(self.socket_path, 0o666)
                    return
                if self.proc.poll() is not None:
                    out = self.proc.stdout.read().decode(errors="replace")
                    raise RuntimeError(f"daemon exited early:\n{out}")
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError("daemon socket event never arrived")
                try:
                    events.wait(remaining)
                except TimeoutError as exc:
                    raise RuntimeError("daemon socket event never arrived") from exc

    def kill_hard(self):
        self.proc.send_signal(signal.SIGKILL)
        self.proc.wait(10)

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(10)


def make_world(tmp_path: Path):
    """World-traversable base dir, a caller-owned git repo, and a daemon."""
    caller = _caller()
    base = tmp_path
    # pytest tmp dirs are 0700; open the chain so the non-root caller can
    # reach the socket and repo.
    p = base
    while p != Path("/"):
        try:
            os.chmod(p, os.stat(p).st_mode | 0o755)
        except OSError:
            break
        p = p.parent
    repo = base / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True,
                   env={"PATH": "/usr/bin:/bin", "HOME": str(base)})
    allowlist = base / "compose-env-allowlist.json"
    allowlist.write_text(json.dumps({
        "schema": 1,
        "authorizations": [{
            "repository_id": repository_id(repo),
            "path": "compose.env",
        }],
    }))
    os.chmod(allowlist, 0o600)
    daemon = Daemon(base, allowlist)
    daemon.start()
    yield type("World", (), {
        "caller": caller, "repo": repo, "daemon": daemon, "base": base,
    })
    daemon.stop()
    subprocess.run(["bash", "-c",
                    f"systemctl stop '{UNIT_PREFIX}-*' 2>/dev/null;"
                    f" systemctl reset-failed '{UNIT_PREFIX}-*' 2>/dev/null;"
                    f" systemctl stop '{DEPLOY_PREFIX}-*' 2>/dev/null;"
                    f" systemctl reset-failed '{DEPLOY_PREFIX}-*' 2>/dev/null;"
                    " true"], check=False)
    ids = subprocess.run(["docker", "ps", "-aq", "--no-trunc", "--filter",
                          f"label=devcoordinator2.instance={UNIT_PREFIX}"],
                         capture_output=True, text=True).stdout.split()
    if ids:
        subprocess.run(["docker", "rm", "-f", "-v", *ids], capture_output=True)


def _write_config(repo: Path, caller: pwd.struct_passwd, body: str):
    cfg = repo / ".devcoordinator.toml"
    cfg.write_text(body)
    for path in [repo, cfg, repo / ".git"]:
        subprocess.run(["chown", "-R", f"{caller.pw_uid}:{caller.pw_gid}",
                        str(path)], check=True)


def _call(world, command, args=None):
    return call_as(world.caller.pw_uid, world.caller.pw_gid,
                   world.daemon.socket_path, _request(command, args))


def _wait_status(world, path: Path, terminal: set[str], timeout=30) -> dict:
    deadline = time.monotonic() + timeout
    last = None
    current = path / ".devcoordinator" / "test" / "current"
    while True:
        with DirectoryEvents(current) as events:
            resp = _call(world, "test.status", {"path": str(path)})
            assert resp["ok"], resp
            last = resp["result"]
            if last["status"] in terminal:
                return last
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            try:
                events.wait(remaining)
            except TimeoutError:
                break
    raise AssertionError(f"status never reached {terminal}; last={last}")


def _units() -> list[str]:
    out = subprocess.run(
        ["systemctl", "list-units", "--all", "--plain", "--no-legend",
         f"{UNIT_PREFIX}-*.service"],
        capture_output=True, text=True).stdout
    return [line.split()[0] for line in out.splitlines() if line.split()]
