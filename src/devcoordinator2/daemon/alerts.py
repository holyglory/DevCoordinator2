"""Sustained-threshold alerts: open once after the condition holds for its
window, deduplicate while active, emit one recovery. Current alerts persist
in the authority database; resolved alerts are removed (no history)."""

from __future__ import annotations

import time
from dataclasses import dataclass
from datetime import UTC, datetime

from devcoordinator2.daemon import events
from devcoordinator2.daemon.db import Database


@dataclass(frozen=True)
class Condition:
    key: str  # unique per (kind, subject)
    kind: str
    subject_kind: str
    subject_id: str
    severity: str  # "warning" | "critical"
    message: str
    active: bool
    sustain_seconds: float


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


class AlertEngine:
    def __init__(self, db: Database):
        self._db = db
        self._since: dict[str, float] = {}
        self._open: set[str] = {
            r["alert_key"] for r in db.query("SELECT alert_key FROM alerts")}

    def evaluate(self, conditions: list[Condition]) -> None:
        now_mono = time.monotonic()
        seen = set()
        for c in conditions:
            seen.add(c.key)
            if c.active:
                self._since.setdefault(c.key, now_mono)
                if c.key not in self._open and \
                        now_mono - self._since[c.key] >= c.sustain_seconds:
                    self._open_alert(c)
                elif c.key in self._open:
                    with self._db.transaction() as conn:
                        conn.execute("UPDATE alerts SET last_seen_at=?, message=?"
                                     " WHERE alert_key=?", (_now(), c.message, c.key))
            else:
                self._since.pop(c.key, None)
                if c.key in self._open:
                    self._resolve(c.key, c)
        # Subjects that vanished (deployment removed, test finished) recover.
        for key in list(self._open):
            if key not in seen:
                self._resolve(key, None)

    def _open_alert(self, c: Condition) -> None:
        with self._db.transaction() as conn:
            conn.execute(
                "INSERT OR REPLACE INTO alerts(alert_key, kind, subject_kind, subject_id,"
                " severity, message, opened_at, last_seen_at) VALUES(?,?,?,?,?,?,?,?)",
                (c.key, c.kind, c.subject_kind, c.subject_id, c.severity, c.message,
                 _now(), _now()))
        self._open.add(c.key)
        events.publish("alert.opened", alert_key=c.key, alert_kind=c.kind,
                       subject_kind=c.subject_kind, subject_id=c.subject_id,
                       severity=c.severity, message=c.message)

    def _resolve(self, key: str, c: Condition | None) -> None:
        rows = self._db.query("SELECT * FROM alerts WHERE alert_key=?", (key,))
        with self._db.transaction() as conn:
            conn.execute("DELETE FROM alerts WHERE alert_key=?", (key,))
        self._open.discard(key)
        self._since.pop(key, None)
        row = dict(rows[0]) if rows else {}
        events.publish("alert.recovered", alert_key=key,
                       alert_kind=row.get("kind", c.kind if c else ""),
                       subject_kind=row.get("subject_kind", ""),
                       subject_id=row.get("subject_id", ""),
                       message=f"recovered: {row.get('message', '')}")

    def current(self) -> list[dict]:
        return [dict(r) for r in self._db.query(
            "SELECT * FROM alerts ORDER BY severity, opened_at")]
