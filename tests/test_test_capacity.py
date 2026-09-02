from __future__ import annotations

import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.test_capacity import CapacityBroker, _Pending

ROOT = Path(__file__).resolve().parents[1]
_WAIT_FOR_SIGNAL = """
import json
import os
import signal
import sys

events, label = sys.argv[1:]
signal.signal(signal.SIGUSR1, lambda _signum, _frame: sys.exit(0))
payload = json.dumps({
    "run": label,
    "check": os.environ["DEVCOORDINATOR_CHECK_NAME"],
    "pid": os.getpid(),
}, separators=(",", ":")).encode() + b"\\n"
descriptor = os.open(events, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
try:
    os.write(descriptor, payload)
finally:
    os.close(descriptor)
signal.pause()
"""


class Clock:
    def __init__(self):
        self.value = 0.0

    def __call__(self):
        return self.value


@pytest.fixture
def running(tmp_path):
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(
        db, tmp_path / "capacity.sock", logical_cpus=2,
        sample_interval=3600)
    broker.start()
    yield broker, db
    broker.shutdown()
    db.close()


def _request(path, run_id, leaf_id):
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.connect(str(path))
    client.sendall((json.dumps({
        "schema": 1, "action": "acquire", "run_id": run_id,
        "leaf_id": leaf_id,
    }, separators=(",", ":")) + "\n").encode())
    return client


def _response(client):
    data = bytearray()
    while not data.endswith(b"\n"):
        data.extend(client.recv(4096))
    return json.loads(data)


def _eventually(predicate, timeout=2.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        threading.Event().wait(0.01)
    raise AssertionError("condition did not become true")


@pytest.fixture(scope="module")
def rust_executor():
    cargo = shutil.which("cargo")
    assert cargo is not None, "Cargo is required for the Rust capacity integration"
    build = subprocess.run(
        [cargo, "build", "--locked", "--release", "--package",
         "devcoordinator2-executor"],
        cwd=ROOT, capture_output=True, text=True, timeout=300, check=False,
    )
    assert build.returncode == 0, (
        f"Rust executor build failed: {build.stderr[-4096:]}")
    binary = ROOT / "target" / "release" / "devcoordinator2-executor"
    assert binary.is_file() and os.access(binary, os.X_OK)
    return binary


def _source_digest(executor: Path, repository: Path) -> str:
    result = subprocess.run(
        [str(executor), "source-digest", "--worktree", str(repository)],
        capture_output=True, text=True, timeout=10, check=False,
    )
    assert result.returncode == 0, result.stderr[-2048:]
    return json.loads(result.stdout)["sha256"]


def _write_plan(repository: Path, run_id: str, commands: list[list[str]],
                source_digest: str) -> Path:
    current = repository / ".devcoordinator" / run_id
    plan_path = repository / ".devcoordinator" / "plans" / f"{run_id}.json"
    plan_path.parent.mkdir(parents=True, exist_ok=True)
    checks = []
    for index, command in enumerate(commands):
        checks.append({
            "name": f"check-{index}",
            "tier": "development",
            "role": "work",
            "after": [],
            "requires": [],
            "invalidates": [],
            "cwd": ".",
            "env": {},
            "timeout_seconds": 15,
            "completion": "process",
            "on_failure": "continue",
            "produces": [],
            "command": command,
            "discover": None,
            "case_command": None,
            "cases": None,
        })
    plan_path.write_text(json.dumps({
        "schema": 2,
        "run_id": run_id,
        "test": "capacity",
        "worktree_root": str(repository),
        "current_dir": str(current),
        "requested_tier": "development",
        "readiness_eligible": False,
        "proof": "complete",
        "selection": [],
        "origin_run_id": None,
        "source_digest": source_digest,
        "config_digest": "c" * 64,
        "reused": {},
        "checks": checks,
    }), encoding="utf-8")
    return plan_path


def _start_executor(executor: Path, plan: Path, socket_path: Path) \
        -> subprocess.Popen:
    return subprocess.Popen(
        [str(executor), "run", str(plan)],
        env={**os.environ, "DEVCOORDINATOR_CAPACITY_SOCKET": str(socket_path)},
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )


def _event_rows(path: Path) -> list[dict]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        return []
    rows = []
    for line in lines:
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            break
    return rows


def _wait_for_rows(path: Path, count: int) -> list[dict]:
    _eventually(lambda: len(_event_rows(path)) >= count, timeout=10)
    return _event_rows(path)


def _finish_executor(process: subprocess.Popen, expected: int = 0) -> None:
    stdout, stderr = process.communicate(timeout=15)
    assert process.returncode == expected, (
        f"executor exited {process.returncode}; stdout={stdout[-2048:]!r}; "
        f"stderr={stderr[-2048:]!r}")


def _pid_is_gone(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    return False


def test_initial_capacity_cap_and_persistence(tmp_path):
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(db, tmp_path / "capacity.sock", logical_cpus=4)
    assert broker.snapshot() == {
        "learned_capacity": 8,
        "effective_capacity": 8,
        "cap": None,
        "active": 0,
        "waiting": 0,
        "paused": False,
        "last_adjustment": None,
    }
    changed = broker.set_cap(3, "uid:1000")
    assert changed["learned_capacity"] == 8
    assert changed["effective_capacity"] == 3
    assert changed["last_adjustment"]["reason"] == "administrator_cap_changed"
    assert changed["last_adjustment"]["previous_capacity"] == 8
    assert changed["last_adjustment"]["new_capacity"] == 3
    restored = CapacityBroker(db, tmp_path / "other.sock", logical_cpus=128)
    assert restored.snapshot()["cap"] == 3
    assert restored.snapshot()["learned_capacity"] == 8
    db.close()


def test_repeated_cap_and_already_clear_are_true_noops(tmp_path):
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(db, tmp_path / "capacity.sock", logical_cpus=4)
    initial_updated = db.query(
        "SELECT updated_at FROM test_capacity_state WHERE singleton=1")[0]["updated_at"]
    assert broker.set_cap(None, "first")["last_adjustment"] is None
    assert db.query("SELECT COUNT(*) AS count FROM test_capacity_events")[0]["count"] == 0
    assert db.query(
        "SELECT updated_at FROM test_capacity_state WHERE singleton=1")[0]["updated_at"] \
        == initial_updated

    first = broker.set_cap(3, "first")
    first_event = first["last_adjustment"]
    updated = db.query(
        "SELECT updated_at FROM test_capacity_state WHERE singleton=1")[0]["updated_at"]
    repeated = broker.set_cap(3, "second")
    assert repeated["last_adjustment"] == first_event
    assert db.query("SELECT COUNT(*) AS count FROM test_capacity_events")[0]["count"] == 1
    assert db.query(
        "SELECT updated_at FROM test_capacity_state WHERE singleton=1")[0]["updated_at"] \
        == updated

    cleared = broker.set_cap(None, "first")
    clear_event = cleared["last_adjustment"]
    repeated_clear = broker.set_cap(None, "second")
    assert repeated_clear["last_adjustment"] == clear_event
    assert db.query("SELECT COUNT(*) AS count FROM test_capacity_events")[0]["count"] == 2
    db.close()


def test_socket_permit_waits_and_disconnect_releases(running):
    broker, _db = running
    broker.set_cap(1, "uid:1000")
    broker.register_run("trun", os.getuid())
    first = _request(broker.socket_path, "trun", "one")
    first_response = _response(first)
    assert first_response["status"] == "granted"
    assert first_response["waited"] is False
    assert first_response["effective_capacity"] == 1

    second = _request(broker.socket_path, "trun", "two")
    _eventually(lambda: broker.snapshot()["waiting"] == 1)
    first.close()
    second_response = _response(second)
    assert second_response["status"] == "granted"
    assert second_response["waited"] is True
    second.close()
    _eventually(lambda: broker.snapshot()["active"] == 0)


def test_socket_rejects_wrong_run_identity(running):
    broker, _db = running
    broker.register_run("trun", os.getuid() + 1)
    client = _request(broker.socket_path, "trun", "leaf")
    assert _response(client) == {
        "schema": 1, "status": "denied", "error": "run identity is unavailable"}
    client.close()


def test_lowering_cap_never_kills_active_permits(running):
    broker, _db = running
    broker.register_run("trun", os.getuid())
    first = _request(broker.socket_path, "trun", "one")
    second = _request(broker.socket_path, "trun", "two")
    assert _response(first)["status"] == "granted"
    assert _response(second)["status"] == "granted"
    changed = broker.set_cap(1, "uid:1000")
    assert changed["effective_capacity"] == 1 and changed["active"] == 2
    third = _request(broker.socket_path, "trun", "three")
    _eventually(lambda: broker.snapshot()["waiting"] == 1)
    first.close()
    _eventually(lambda: broker.snapshot()["active"] == 1)
    assert broker.snapshot()["waiting"] == 1
    second.close()
    assert _response(third)["status"] == "granted"
    third.close()


def _pending(run_id: str, leaf_id: str):
    client, server = socket.socketpair()
    return _Pending(server, run_id, leaf_id), client


def test_fifo_within_run_and_round_robin_across_runs(tmp_path):
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(db, tmp_path / "capacity.sock", logical_cpus=1)
    broker.set_cap(1, "fixture")
    broker.register_run("ta", os.getuid())
    broker.register_run("tb", os.getuid())
    first, first_client = _pending("ta", "a0")
    queued = [_pending(run, leaf) for run, leaf in (
        ("ta", "a1"), ("ta", "a2"), ("tb", "b1"), ("tb", "b2"))]
    with broker._condition:
        broker._enqueue_locked(first)
        broker._grant_ready_locked()
        for pending, _client in queued:
            broker._enqueue_locked(pending)
        broker._grant_ready_locked()
    order = []
    current = first
    for _ in range(4):
        broker._release(current.permit_id)
        current = next(pending for pending, _client in queued
                       if pending.permit_id is not None and pending.leaf_id not in order)
        order.append(current.leaf_id)
    assert order == ["a1", "b1", "a2", "b2"]
    broker._release(current.permit_id)
    first_client.close()
    first.connection.close()
    for pending, client in queued:
        client.close()
        pending.connection.close()
    broker.unregister_run("ta")
    broker.unregister_run("tb")
    db.close()


def test_run_end_learning_increases_then_decreases(tmp_path):
    clock = Clock()
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(
        db, tmp_path / "capacity.sock", logical_cpus=1, clock=clock,
        sample_interval=3600, min_epoch_seconds=600)
    broker.register_run("tlow", os.getuid())
    leaves = [_pending("tlow", str(index)) for index in range(2)]
    with broker._condition:
        for pending, _client in leaves:
            broker._enqueue_locked(pending)
        broker._grant_ready_locked()
    for _ in range(40):
        broker.record_sample(25.0, 30.0)
    clock.value = 601.0
    for pending, client in leaves:
        broker._release(pending.permit_id)
        client.close()
        pending.connection.close()
    # Dependency waves may temporarily leave a live run with no permit. That
    # gap is still part of one workload epoch and cannot trigger learning.
    assert broker.snapshot()["learned_capacity"] == 2
    next_wave, next_client = _pending("tlow", "next-wave")
    with broker._condition:
        broker._enqueue_locked(next_wave)
        broker._grant_ready_locked()
    clock.value = 700.0
    broker._release(next_wave.permit_id)
    next_client.close()
    next_wave.connection.close()
    assert broker.snapshot()["learned_capacity"] == 2
    broker.unregister_run("tlow")
    assert broker.snapshot()["learned_capacity"] == 3
    assert broker.snapshot()["last_adjustment"]["reason"] == \
        "underused_saturated_epoch"

    broker.register_run("thigh", os.getuid())
    leaves = [_pending("thigh", str(index)) for index in range(3)]
    with broker._condition:
        for pending, _client in leaves:
            broker._enqueue_locked(pending)
        broker._grant_ready_locked()
    for _ in range(4):
        broker.record_sample(99.0, 20.0)
    assert broker.snapshot()["paused"] is True
    broker.record_sample(20.0, 20.0)
    broker.record_sample(20.0, 20.0)
    assert broker.snapshot()["paused"] is False
    clock.value = 1301.0
    for pending, client in leaves:
        broker._release(pending.permit_id)
        client.close()
        pending.connection.close()
    broker.unregister_run("thigh")
    assert broker.snapshot()["learned_capacity"] == 2
    assert broker.snapshot()["last_adjustment"]["reason"] == "sustained_pressure"
    db.close()


def test_missing_measurements_never_adjust(tmp_path):
    clock = Clock()
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(
        db, tmp_path / "capacity.sock", logical_cpus=1, clock=clock,
        sample_interval=3600, min_epoch_seconds=600)
    broker.register_run("tmissing", os.getuid())
    leaves = [_pending("tmissing", str(index)) for index in range(2)]
    with broker._condition:
        for pending, _client in leaves:
            broker._enqueue_locked(pending)
        broker._grant_ready_locked()
    for _ in range(40):
        broker.record_sample(None, 10.0)
    clock.value = 601.0
    for pending, client in leaves:
        broker._release(pending.permit_id)
        client.close()
        pending.connection.close()
    broker.unregister_run("tmissing")
    assert broker.snapshot()["learned_capacity"] == 2
    assert broker.snapshot()["last_adjustment"] is None
    db.close()


def test_one_measured_resource_can_prove_sustained_pressure(tmp_path):
    clock = Clock()
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(
        db, tmp_path / "capacity.sock", logical_cpus=2, clock=clock,
        sample_interval=3600, min_epoch_seconds=600)
    broker.register_run("tmemory", os.getuid())
    leaves = [_pending("tmemory", str(index)) for index in range(4)]
    with broker._condition:
        for pending, _client in leaves:
            broker._enqueue_locked(pending)
        broker._grant_ready_locked()
    for _ in range(4):
        broker.record_sample(None, 99.0)
    clock.value = 601.0
    for pending, client in leaves:
        broker._release(pending.permit_id)
        client.close()
        pending.connection.close()
    broker.unregister_run("tmemory")
    state = broker.snapshot()
    assert state["learned_capacity"] == 3
    assert state["last_adjustment"]["p95_cpu_percent"] is None
    assert state["last_adjustment"]["p95_memory_percent"] == 99.0
    db.close()


def test_alternating_cpu_and_memory_pressure_never_counts_as_sustained(tmp_path):
    clock = Clock()
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(
        db, tmp_path / "capacity.sock", logical_cpus=2, clock=clock,
        sample_interval=3600, min_epoch_seconds=600)
    broker.register_run("talternating", os.getuid())
    leaves = [_pending("talternating", str(index)) for index in range(4)]
    with broker._condition:
        for pending, _client in leaves:
            broker._enqueue_locked(pending)
        broker._grant_ready_locked()
    for index in range(8):
        broker.record_sample(99.0, 20.0) if index % 2 == 0 \
            else broker.record_sample(20.0, 99.0)
    assert broker.snapshot()["paused"] is False
    clock.value = 601.0
    for pending, client in leaves:
        broker._release(pending.permit_id)
        client.close()
        pending.connection.close()
    broker.unregister_run("talternating")
    state = broker.snapshot()
    assert state["learned_capacity"] == 4
    assert state["last_adjustment"] is None
    db.close()


def test_real_rust_executor_obeys_fair_capacity_and_disconnects_without_leaks(
        tmp_path, rust_executor):
    repository = tmp_path / "repository"
    repository.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repository, check=True)
    (repository / "README.md").write_text("capacity integration\n", encoding="utf-8")
    subprocess.run(["git", "add", "README.md"], cwd=repository, check=True)
    digest = _source_digest(rust_executor, repository)
    events = repository / ".devcoordinator" / "capacity-events.jsonl"

    def waiting_command(label: str) -> list[str]:
        return [sys.executable, "-c", _WAIT_FOR_SIGNAL, str(events), label]

    plans = {
        "run-a": _write_plan(
            repository, "run-a", [waiting_command("A") for _ in range(3)], digest),
        "run-b": _write_plan(
            repository, "run-b", [waiting_command("B") for _ in range(2)], digest),
        "run-active-crash": _write_plan(
            repository, "run-active-crash", [waiting_command("C")], digest),
        "run-waiting-crash": _write_plan(
            repository, "run-waiting-crash", [waiting_command("E")], digest),
        "run-probe": _write_plan(
            repository, "run-probe", [["/bin/true"]], digest),
    }
    db = Database(tmp_path / "authority.sqlite3")
    broker = CapacityBroker(
        db, tmp_path / "capacity.sock", logical_cpus=1, sample_interval=3600)
    broker.set_cap(1, "integration")
    processes: list[subprocess.Popen] = []
    leaf_pids: set[int] = set()
    registered = list(plans)
    for run_id in registered:
        broker.register_run(run_id, os.getuid())

    try:
        broker.start()
        run_a = _start_executor(rust_executor, plans["run-a"], broker.socket_path)
        processes.append(run_a)
        _eventually(
            lambda: broker.snapshot()["active"] == 1
            and broker.snapshot()["waiting"] == 2,
            timeout=10,
        )
        assert [row["run"] for row in _wait_for_rows(events, 1)] == ["A"]

        run_b = _start_executor(rust_executor, plans["run-b"], broker.socket_path)
        processes.append(run_b)
        _eventually(
            lambda: broker.snapshot()["active"] == 1
            and broker.snapshot()["waiting"] == 4,
            timeout=10,
        )

        expected_order = ["A", "A", "B", "A", "B"]
        for completed, expected in enumerate(expected_order, start=1):
            rows = _wait_for_rows(events, completed)
            assert [row["run"] for row in rows] == expected_order[:completed]
            assert rows[-1]["run"] == expected
            assert broker.snapshot()["active"] == 1
            leaf_pid = rows[-1]["pid"]
            leaf_pids.add(leaf_pid)
            os.kill(leaf_pid, signal.SIGUSR1)
            _eventually(lambda pid=leaf_pid: _pid_is_gone(pid), timeout=10)
            leaf_pids.discard(leaf_pid)

        _finish_executor(run_a)
        _finish_executor(run_b)
        processes.remove(run_a)
        processes.remove(run_b)
        _eventually(
            lambda: broker.snapshot()["active"] == 0
            and broker.snapshot()["waiting"] == 0,
            timeout=10,
        )
        reports = [
            json.loads((repository / ".devcoordinator" / run_id
                        / "check-report.json").read_text(encoding="utf-8"))
            for run_id in ("run-a", "run-b")
        ]
        assert sum(report["capacity"]["capacity_wait_count"]
                   for report in reports) == 4
        assert all(report["capacity"]["effective_capacity"] == 1
                   for report in reports)

        active_crash = _start_executor(
            rust_executor, plans["run-active-crash"], broker.socket_path)
        processes.append(active_crash)
        active_row = _wait_for_rows(events, 6)[-1]
        leaf_pids.add(active_row["pid"])
        _eventually(lambda: broker.snapshot()["active"] == 1, timeout=10)

        waiting_crash = _start_executor(
            rust_executor, plans["run-waiting-crash"], broker.socket_path)
        processes.append(waiting_crash)
        _eventually(lambda: broker.snapshot()["waiting"] == 1, timeout=10)
        waiting_crash.kill()
        _finish_executor(waiting_crash, expected=-signal.SIGKILL)
        processes.remove(waiting_crash)
        _eventually(
            lambda: broker.snapshot()["active"] == 1
            and broker.snapshot()["waiting"] == 0,
            timeout=10,
        )

        active_crash.kill()
        _finish_executor(active_crash, expected=-signal.SIGKILL)
        processes.remove(active_crash)
        _eventually(
            lambda: broker.snapshot()["active"] == 0
            and broker.snapshot()["waiting"] == 0,
            timeout=10,
        )
        try:
            os.killpg(active_row["pid"], signal.SIGKILL)
        except ProcessLookupError:
            pass
        leaf_pids.discard(active_row["pid"])

        probe = _start_executor(rust_executor, plans["run-probe"], broker.socket_path)
        processes.append(probe)
        _finish_executor(probe)
        processes.remove(probe)
        _eventually(
            lambda: broker.snapshot()["active"] == 0
            and broker.snapshot()["waiting"] == 0,
            timeout=10,
        )
    finally:
        for process in processes:
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=5)
        for pid in leaf_pids:
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for run_id in registered:
            broker.unregister_run(run_id)
        broker.shutdown()
        db.close()
