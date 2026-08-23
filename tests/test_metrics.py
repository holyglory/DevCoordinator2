from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest

from devcoordinator2.daemon import events, metrics_store
from devcoordinator2.daemon.alerts import AlertEngine, Condition
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.metrics_sources import _parse_size, cgroup_stats, host_memory


@pytest.fixture
def db(tmp_path: Path):
    database = Database(tmp_path / "db.sqlite3")
    yield database
    database.close()


def test_parse_sizes():
    assert _parse_size("553MB") == 553_000_000
    assert _parse_size("2.5GiB") == int(2.5 * 2**30)
    assert _parse_size("0B (virtual 553MB)") == 0
    assert _parse_size("garbage") == 0


def test_cgroup_stats_reads_real_root():
    stats = cgroup_stats(Path("/sys/fs/cgroup"))
    assert stats is None or stats["cpu_usec"] >= 0
    assert cgroup_stats(Path("/nonexistent")) is None
    mem = host_memory()
    assert mem["total"] > 0 and mem["used"] >= 0


def test_flush_series_trend_and_expire(db):
    now = datetime.now(UTC)
    minute = metrics_store.minute_key(now)
    metrics_store.flush(db, minute, {("repository", "r1", "cpu_percent"): (1.0, 6.0, 5.0, 3)})
    old = metrics_store.minute_key(now - timedelta(days=31))
    metrics_store.flush(db, old, {("repository", "r1", "cpu_percent"): (0.0, 0.0, 0.0, 1)})
    points = metrics_store.series(db, "repository", "r1", "cpu_percent", 60)
    assert len(points) == 1 and points[0]["avg"] == 2.0 and points[0]["max"] == 5.0
    assert metrics_store.trend(db, "repository", "r1", "cpu_percent") == [2.0]
    assert metrics_store.table_size(db) == 2
    assert metrics_store.expire(db) == 1
    assert metrics_store.table_size(db) == 1
    assert metrics_store.series(db, "repository", "none", "cpu_percent", 60) == []


def test_alert_sustain_dedupe_and_recovery(db, monkeypatch):
    received = []
    events.subscribe(received.append)
    engine = AlertEngine(db)
    clock = [1000.0]
    monkeypatch.setattr("devcoordinator2.daemon.alerts.time.monotonic", lambda: clock[0])
    cond = Condition("host/cpu", "host_cpu", "host", "host", "warning", "cpu high", True, 300)
    engine.evaluate([cond])
    assert engine.current() == []  # not sustained yet
    clock[0] += 299
    engine.evaluate([cond])
    assert engine.current() == []
    clock[0] += 2
    engine.evaluate([cond])
    assert [a["alert_key"] for a in engine.current()] == ["host/cpu"]
    engine.evaluate([cond])  # still active: deduplicated, no second event
    assert sum(1 for e in received if e["kind"] == "alert.opened") == 1
    engine.evaluate([Condition(**{**cond.__dict__, "active": False})])
    assert engine.current() == []
    recovered = [e for e in received if e["kind"] == "alert.recovered"]
    assert len(recovered) == 1 and recovered[0]["alert_key"] == "host/cpu"
    # Persisted open alerts survive an engine restart.
    engine.evaluate([cond])
    clock[0] += 301
    engine.evaluate([cond])
    assert AlertEngine(db).current()[0]["kind"] == "host_cpu"
    # Vanished subject recovers.
    AlertEngine(db).evaluate([])
    assert AlertEngine(db).current() == []
