"""SQLite authority database: open, schema, transactions.

Schema version 1; the table inventory is tracked in docs/database-ledger.md.
Tests deliberately have no tables (repository-local files only).
"""

from __future__ import annotations

import sqlite3
import threading
from contextlib import contextmanager
from pathlib import Path

SCHEMA_VERSION = 1

_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS repositories (
  repository_id     TEXT PRIMARY KEY,
  root_path         TEXT NOT NULL UNIQUE,
  display_name      TEXT NOT NULL,
  registered_at     TEXT NOT NULL,
  registered_by_uid INTEGER NOT NULL,
  last_seen_at      TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS worktrees (
  worktree_id   TEXT PRIMARY KEY,
  repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
  worktree_path TEXT NOT NULL UNIQUE,
  registered_at TEXT NOT NULL,
  last_seen_at  TEXT NOT NULL
);
"""


class SchemaMismatch(Exception):
    pass


class Database:
    """Single shared connection guarded by a lock (few, short transactions)."""

    def __init__(self, path: Path):
        path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(str(path), check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._lock = threading.Lock()
        with self._lock:
            self._conn.execute("PRAGMA journal_mode=WAL")
            self._conn.execute("PRAGMA foreign_keys=ON")
            self._conn.execute("PRAGMA synchronous=FULL")
            self._conn.executescript(_SCHEMA)
            row = self._conn.execute(
                "SELECT value FROM meta WHERE key='schema_version'"
            ).fetchone()
            if row is None:
                self._conn.execute(
                    "INSERT INTO meta(key, value) VALUES('schema_version', ?)",
                    (str(SCHEMA_VERSION),),
                )
                self._conn.commit()
            elif int(row["value"]) != SCHEMA_VERSION:
                raise SchemaMismatch(
                    f"database schema {row['value']}, daemon expects {SCHEMA_VERSION}"
                )

    @contextmanager
    def transaction(self):
        with self._lock:
            try:
                yield self._conn
                self._conn.commit()
            except BaseException:
                self._conn.rollback()
                raise

    def query(self, sql: str, params: tuple = ()) -> list[sqlite3.Row]:
        with self._lock:
            return self._conn.execute(sql, params).fetchall()

    def close(self) -> None:
        with self._lock:
            self._conn.close()
