from __future__ import annotations

import json
import os
import socket
import threading
import time

import pytest

from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.test_capacity import CapacityBroker, _Pending


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
