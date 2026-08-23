"""Root-required end-to-end lifecycle tests against real systemd units.

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
import socket
import subprocess
import sys
import time
import uuid
from pathlib import Path

import pytest

pytestmark = pytest.mark.skipif(
    os.geteuid() != 0 or not os.environ.get("DEVCOORDINATOR2_ROOT_TESTS"),
    reason="requires root and DEVCOORDINATOR2_ROOT_TESTS=1",
)

REPO_ROOT = Path(__file__).resolve().parents[2]
UNIT_PREFIX = "devcoordinator2-inttest"


def _caller() -> pwd.struct_passwd:
    sudo_uid = os.environ.get("SUDO_UID")
    if sudo_uid and int(sudo_uid) != 0:
        return pwd.getpwuid(int(sudo_uid))
    return pwd.getpwnam("nobody")


def call_as(uid: int, gid: int, sock_path: Path, request: dict) -> dict:
    """Send one request from a forked child running as uid/gid."""
    read_fd, write_fd = os.pipe()
    pid = os.fork()
    if pid == 0:
        try:
            os.close(read_fd)
            os.setgroups([gid])
            os.setgid(gid)
            os.setuid(uid)
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
                s.settimeout(30)
                s.connect(str(sock_path))
                s.sendall((json.dumps(request) + "\n").encode())
                s.shutdown(socket.SHUT_WR)
                data = b""
                while True:
                    part = s.recv(65536)
                    if not part:
                        break
                    data += part
            os.write(write_fd, data)
            os._exit(0)
        except Exception:
            os._exit(1)
    os.close(write_fd)
    chunks = b""
    while True:
        part = os.read(read_fd, 65536)
        if not part:
            break
        chunks += part
    os.close(read_fd)
    _, status = os.waitpid(pid, 0)
    assert status == 0, f"child failed (no response); raw={chunks!r}"
    return json.loads(chunks)


def _request(command: str, args: dict | None = None) -> dict:
    return {"protocol": 1, "id": uuid.uuid4().hex[:8], "command": command,
            "args": args or {}, "client": {"kind": "other"}}


class Daemon:
    def __init__(self, base: Path):
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
        self.proc: subprocess.Popen | None = None

    def start(self):
        self.proc = subprocess.Popen(
            [sys.executable, "-m", "devcoordinator2.daemon"],
            env=self.env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.socket_path.exists():
                os.chmod(self.socket_path, 0o666)
                return
            if self.proc.poll() is not None:
                out = self.proc.stdout.read().decode(errors="replace")
                raise RuntimeError(f"daemon exited early:\n{out}")
            time.sleep(0.1)
        raise RuntimeError("daemon socket never appeared")

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


@pytest.fixture
def world(tmp_path: Path):
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
    daemon = Daemon(base)
    daemon.start()
    yield type("World", (), {
        "caller": caller, "repo": repo, "daemon": daemon, "base": base,
    })
    daemon.stop()
    subprocess.run(["bash", "-c",
                    f"systemctl stop '{UNIT_PREFIX}-*' 2>/dev/null;"
                    f" systemctl reset-failed '{UNIT_PREFIX}-*' 2>/dev/null;"
                    " true"], check=False)


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
    while time.monotonic() < deadline:
        resp = _call(world, "test.status", {"path": str(path)})
        assert resp["ok"], resp
        last = resp["result"]
        if last["status"] in terminal:
            return last
        time.sleep(0.3)
    raise AssertionError(f"status never reached {terminal}; last={last}")


def _units() -> list[str]:
    out = subprocess.run(
        ["systemctl", "list-units", "--all", "--plain", "--no-legend",
         f"{UNIT_PREFIX}-*.service"],
        capture_output=True, text=True).stdout
    return [line.split()[0] for line in out.splitlines() if line.split()]


def test_pass_uid_and_bounded_output(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["id"]\n'
                  'timeout_seconds = 60\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    assert resp["result"]["status"] == "running"
    final = _wait_status(world, world.repo, {"passed", "failed"})
    assert final["status"] == "passed"
    assert final["exit_code"] == 0
    assert final["caller_uid"] == world.caller.pw_uid
    # Successful status carries no log text.
    assert "tail" not in final and "stdout" not in final
    out = _call(world, "test.output",
                {"path": str(world.repo), "stream": "stdout"})
    assert f"uid={world.caller.pw_uid}" in out["result"]["tail"]
    # Summary file is owned by the caller and valid JSON.
    sp = Path(final["summary_path"])
    assert sp.stat().st_uid == world.caller.pw_uid
    assert json.loads(sp.read_text())["status"] == "passed"


def test_broken_command_terminal_failure(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["/nonexistent/prog"]\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"] is False
    assert resp["error"]["code"] == "test_start_failed"
    assert "queued" not in json.dumps(resp).lower()


def test_timeout_kills_whole_cgroup(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\n'
                  'command = ["sleep", "120"]\ntimeout_seconds = 2\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    final = _wait_status(world, world.repo, {"timed-out"}, timeout=40)
    assert final["exit_code"] is None
    assert _units() == []


def test_cancel(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    stop = _call(world, "test.stop", {"path": str(world.repo)})
    assert stop["ok"], stop
    assert stop["result"]["status"] == "cancelled"
    assert _units() == []


def test_supersession_latest_start_wins(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n')
    first = _call(world, "test.start", {"path": str(world.repo)})
    assert first["ok"], first
    second = _call(world, "test.start", {"path": str(world.repo)})
    assert second["ok"], second
    assert second["result"]["run_id"] != first["result"]["run_id"]
    active = _units()
    assert len(active) == 1
    assert second["result"]["unit"] in active[0]
    status = _call(world, "test.status", {"path": str(world.repo)})
    assert status["result"]["run_id"] == second["result"]["run_id"]
    _call(world, "test.stop", {"path": str(world.repo)})


def test_flooder_capped_but_counted(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\n'
                  'command = ["dd", "if=/dev/zero", "bs=64k", "count=128",'
                  ' "status=none"]\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    final = _wait_status(world, world.repo, {"passed", "failed"}, timeout=60)
    assert final["status"] == "passed"
    assert final["stdout_bytes_observed"] == 64 * 1024 * 128
    assert final["stdout_bytes_retained"] == 4 * 1024 * 1024
    assert final["stdout_truncated"] is True


def test_daemon_restart_marks_interrupted(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    summary_path = Path(resp["result"]["summary_path"])
    world.daemon.kill_hard()
    assert json.loads(summary_path.read_text())["status"] == "running"
    world.daemon.start()
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if json.loads(summary_path.read_text())["status"] == "interrupted":
            break
        time.sleep(0.3)
    doc = json.loads(summary_path.read_text())
    assert doc["status"] == "interrupted"
    assert _units() == []
    # Never resurrected: still interrupted after a grace period.
    time.sleep(2)
    assert json.loads(summary_path.read_text())["status"] == "interrupted"


def test_root_caller_rejected(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["id"]\n')
    resp = call_as(0, 0, world.daemon.socket_path,
                   _request("test.start", {"path": str(world.repo)}))
    assert resp["ok"] is False
    assert resp["error"]["code"] == "test_start_failed"
    assert "root" in resp["error"]["message"]


# -- Phase 2: test-scoped ephemeral PostgreSQL ---------------------------------

def _containers_with_label(key: str, value: str) -> list[str]:
    out = subprocess.run(
        ["docker", "ps", "--all", "--no-trunc", "--quiet",
         "--filter", f"label=devcoordinator2.{key}={value}"],
        capture_output=True, text=True).stdout
    return [ln.strip() for ln in out.splitlines() if ln.strip()]


PG_TOML = ('schema = 1\n[test.unit]\n'
           'command = ["psql", "-v", "ON_ERROR_STOP=1", "-c",'
           ' "create table t(x int); insert into t values (42); select x from t"]\n'
           'timeout_seconds = 120\n[test.unit.postgres]\n'
           'image = "postgres:16-alpine"\ndatabase = "app_test"\nuser = "app"\n')


def test_postgres_real_query_labels_secrecy_and_cleanup(world):
    _write_config(world.repo, world.caller, PG_TOML)
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    run_id = resp["result"]["run_id"]
    unit = resp["result"]["unit"]
    # The container exists, carries the exact run identity and attribution.
    owned = _containers_with_label("run", run_id)
    assert len(owned) == 1, owned
    labels = json.loads(subprocess.run(
        ["docker", "inspect", "--format", "{{json .Config.Labels}}", owned[0]],
        capture_output=True, text=True).stdout)
    assert labels["devcoordinator2.purpose"] == "test"
    assert labels["devcoordinator2.caller_uid"] == str(world.caller.pw_uid)
    assert labels["devcoordinator2.data"] == "disposable"
    assert labels["devcoordinator2.instance"] == UNIT_PREFIX
    # The password never enters the unit's public Environment property.
    env_prop = subprocess.run(["systemctl", "show", unit, "-p", "Environment"],
                              capture_output=True, text=True).stdout
    assert "PGPASSWORD" not in env_prop
    env_file = world.repo / ".devcoordinator" / "test" / "current" / "env"
    st = env_file.stat()
    assert st.st_mode & 0o777 == 0o600 and st.st_uid == world.caller.pw_uid
    final = _wait_status(world, world.repo, {"passed", "failed"}, timeout=120)
    out = _call(world, "test.output", {"path": str(world.repo),
                                       "stream": "stdout"})["result"]["tail"]
    err = _call(world, "test.output", {"path": str(world.repo),
                                       "stream": "stderr"})["result"]["tail"]
    assert final["status"] == "passed", (final, out, err)
    assert "42" in out
    # Summary and status carry no credentials; container is gone.
    assert "PGPASSWORD" not in json.dumps(final)
    assert _containers_with_label("run", run_id) == []


def test_postgres_removed_on_supersession_and_recovery(world):
    slow = ('schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n'
            '[test.unit.postgres]\nimage = "postgres:16-alpine"\n')
    _write_config(world.repo, world.caller, slow)
    first = _call(world, "test.start", {"path": str(world.repo)})
    assert first["ok"], first
    first_run = first["result"]["run_id"]
    assert len(_containers_with_label("run", first_run)) == 1
    second = _call(world, "test.start", {"path": str(world.repo)})
    assert second["ok"], second
    second_run = second["result"]["run_id"]
    assert _containers_with_label("run", first_run) == []
    assert len(_containers_with_label("run", second_run)) == 1
    # Daemon dies; restart recovery removes the orphaned test container.
    world.daemon.kill_hard()
    assert len(_containers_with_label("run", second_run)) == 1
    world.daemon.start()
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline and _containers_with_label("run", second_run):
        time.sleep(0.5)
    assert _containers_with_label("run", second_run) == []
    assert _containers_with_label("instance", UNIT_PREFIX) == []
