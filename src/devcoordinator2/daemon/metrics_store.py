"""Bounded time series: one-minute aggregates, 30-day retention, direct
deletion. Disposable by design."""

from __future__ import annotations

from datetime import UTC, datetime, timedelta

from devcoordinator2.daemon.db import Database

RETENTION_DAYS = 30


def minute_key(ts: datetime) -> str:
    return ts.strftime("%Y-%m-%dT%H:%MZ")


def flush(db: Database, minute: str,
          aggregates: dict[tuple[str, str, str], tuple[float, float, float, int]]) -> None:
    """aggregates: (kind, id, metric) -> (min, sum, max, count)."""
    if not aggregates:
        return
    rows = [(kind, sid, metric, minute, mn, total / count, mx, count)
            for (kind, sid, metric), (mn, total, mx, count) in aggregates.items()
            if count]
    with db.transaction() as conn:
        conn.executemany(
            "INSERT OR REPLACE INTO metric_minutes(subject_kind, subject_id, metric,"
            " minute_utc, min_value, avg_value, max_value, samples) VALUES(?,?,?,?,?,?,?,?)",
            rows)


def expire(db: Database, now: datetime | None = None) -> int:
    cutoff = minute_key((now or datetime.now(UTC)) - timedelta(days=RETENTION_DAYS))
    with db.transaction() as conn:
        cur = conn.execute("DELETE FROM metric_minutes WHERE minute_utc < ?", (cutoff,))
        return cur.rowcount


def series(db: Database, subject_kind: str, subject_id: str, metric: str,
           minutes: int) -> list[dict]:
    minutes = max(1, min(minutes, 60 * 24 * RETENTION_DAYS))
    since = minute_key(datetime.now(UTC) - timedelta(minutes=minutes))
    rows = db.query(
        "SELECT minute_utc, min_value, avg_value, max_value, samples FROM metric_minutes"
        " WHERE subject_kind=? AND subject_id=? AND metric=? AND minute_utc>=?"
        " ORDER BY minute_utc", (subject_kind, subject_id, metric, since))
    return [{"minute": r["minute_utc"], "min": r["min_value"], "avg": r["avg_value"],
             "max": r["max_value"], "samples": r["samples"]} for r in rows]


def trend(db: Database, subject_kind: str, subject_id: str, metric: str,
          minutes: int = 60, points: int = 12) -> list[float]:
    """Compact trend: `points` averages over the last `minutes`."""
    data = series(db, subject_kind, subject_id, metric, minutes)
    if not data:
        return []
    bucket = max(1, len(data) // points)
    out = []
    for i in range(0, len(data), bucket):
        chunk = data[i:i + bucket]
        out.append(round(sum(d["avg"] for d in chunk) / len(chunk), 3))
    return out[-points:]


def table_size(db: Database) -> int:
    return db.query("SELECT count(*) AS n FROM metric_minutes")[0]["n"]
