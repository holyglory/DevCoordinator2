"""SQLite authority database: open, schema, transactions.

Schema version 1; the table inventory is tracked in docs/database-ledger.md.
Tests deliberately have no tables (repository-local files only).
"""

from __future__ import annotations

import sqlite3
import threading
from contextlib import contextmanager
from pathlib import Path

SCHEMA_VERSION = 7

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

# Schema 2 (Phase 3): deployments. Repositories/worktrees are preserved.
_SCHEMA_V2 = """
CREATE TABLE IF NOT EXISTS deployments (
  deployment_id       TEXT PRIMARY KEY,
  repository_id       TEXT NOT NULL REFERENCES repositories(repository_id),
  worktree_id         TEXT NOT NULL REFERENCES worktrees(worktree_id),
  name                TEXT NOT NULL,
  source              TEXT NOT NULL,
  domain              TEXT,
  spec_fingerprint    TEXT NOT NULL,
  spec_json           TEXT NOT NULL,
  state               TEXT NOT NULL,
  current_generation  INTEGER,
  previous_generation INTEGER,
  created_at          TEXT NOT NULL,
  created_by_uid      INTEGER NOT NULL,
  client              TEXT NOT NULL,
  updated_at          TEXT NOT NULL,
  ttl_expires_at      TEXT,
  UNIQUE(worktree_id, name, source)
);
CREATE TABLE IF NOT EXISTS generations (
  deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
  number        INTEGER NOT NULL,
  commit_hash   TEXT,
  dirty         INTEGER NOT NULL DEFAULT 0,
  path          TEXT NOT NULL,
  fingerprint   TEXT NOT NULL,
  created_at    TEXT NOT NULL,
  state         TEXT NOT NULL,
  PRIMARY KEY(deployment_id, number)
);
CREATE TABLE IF NOT EXISTS components (
  deployment_id       TEXT NOT NULL REFERENCES deployments(deployment_id),
  name                TEXT NOT NULL,
  type                TEXT NOT NULL,
  order_index         INTEGER NOT NULL,
  spec_fingerprint    TEXT NOT NULL,
  desired_state       TEXT NOT NULL,
  state               TEXT NOT NULL,
  health              TEXT NOT NULL,
  generation          INTEGER,
  binding_kind        TEXT,
  binding_identity    TEXT,
  restarts            INTEGER NOT NULL DEFAULT 0,
  last_error          TEXT,
  updated_at          TEXT NOT NULL,
  PRIMARY KEY(deployment_id, name)
);
CREATE TABLE IF NOT EXISTS port_assignments (
  port          INTEGER PRIMARY KEY,
  deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
  component     TEXT NOT NULL,
  generation    INTEGER NOT NULL,
  assigned_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS domain_routes (
  domain        TEXT PRIMARY KEY,
  deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
  component     TEXT NOT NULL,
  port          INTEGER,
  generation    INTEGER,
  published_at  TEXT
);
"""

# Schema 3 (Phase 4): bounded health samples and current alerts. Disposable:
# may be rebuilt or dropped without affecting control data.
_SCHEMA_V3 = """
CREATE TABLE IF NOT EXISTS metric_minutes (
  subject_kind TEXT NOT NULL,
  subject_id   TEXT NOT NULL,
  metric       TEXT NOT NULL,
  minute_utc   TEXT NOT NULL,
  min_value    REAL NOT NULL,
  avg_value    REAL NOT NULL,
  max_value    REAL NOT NULL,
  samples      INTEGER NOT NULL,
  PRIMARY KEY(subject_kind, subject_id, metric, minute_utc)
);
CREATE INDEX IF NOT EXISTS metric_minutes_time ON metric_minutes(minute_utc);
CREATE TABLE IF NOT EXISTS alerts (
  alert_key    TEXT PRIMARY KEY,
  kind         TEXT NOT NULL,
  subject_kind TEXT NOT NULL,
  subject_id   TEXT NOT NULL,
  severity     TEXT NOT NULL,
  message      TEXT NOT NULL,
  opened_at    TEXT NOT NULL,
  last_seen_at TEXT NOT NULL
);
"""

# Schema 4 (Phase 5): public users, invitations, deployment grants. Control
# data: always preserved.
_SCHEMA_V4 = """
CREATE TABLE IF NOT EXISTS users (
  user_id       TEXT PRIMARY KEY,
  email         TEXT NOT NULL UNIQUE,
  subject       TEXT,
  display_name  TEXT,
  administrator INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT NOT NULL,
  created_by    TEXT NOT NULL,
  last_seen_at  TEXT
);
CREATE TABLE IF NOT EXISTS invitations (
  invitation_id TEXT PRIMARY KEY,
  email         TEXT NOT NULL UNIQUE,
  administrator INTEGER NOT NULL DEFAULT 0,
  grants_json   TEXT NOT NULL,
  created_at    TEXT NOT NULL,
  created_by    TEXT NOT NULL,
  expires_at    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS grants (
  user_id       TEXT NOT NULL REFERENCES users(user_id),
  deployment_id TEXT NOT NULL,
  role          TEXT NOT NULL,
  granted_at    TEXT NOT NULL,
  granted_by    TEXT NOT NULL,
  PRIMARY KEY(user_id, deployment_id)
);
"""

# Schema 5 (Phase 6): Telegram chats, link codes, subscriptions, bounded outbox.
_SCHEMA_V5 = """
CREATE TABLE IF NOT EXISTS telegram_chats (
  chat_id    INTEGER PRIMARY KEY,
  email      TEXT NOT NULL,
  label      TEXT,
  linked_at  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS telegram_links (
  code       TEXT PRIMARY KEY,
  chat_id    INTEGER NOT NULL,
  label      TEXT,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS telegram_subscriptions (
  chat_id    INTEGER NOT NULL REFERENCES telegram_chats(chat_id),
  scope      TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(chat_id, scope)
);
CREATE TABLE IF NOT EXISTS telegram_outbox (
  message_id      INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id         INTEGER NOT NULL,
  text            TEXT NOT NULL,
  created_at      TEXT NOT NULL,
  attempts        INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT NOT NULL,
  last_error      TEXT
);
"""

# Schema 6: current observed-only resources imported from the retired
# coordinator. This is a replaceable projection, never configuration authority
# or history. Exact native identities disappear from these tables on the next
# current-state import when they are no longer present.
# Schema 7 relaxed the state constraints: the daemon may start/stop/restart
# the exact recorded containers (DC2-2026-08-24-OBSERVED-LIFECYCLE), so
# stopped/failed states are recordable.
_SCHEMA_V6 = """
CREATE TABLE IF NOT EXISTS observed_deployments (
  observed_deployment_id TEXT PRIMARY KEY,
  repository_id          TEXT NOT NULL REFERENCES repositories(repository_id),
  name                   TEXT NOT NULL,
  native_project         TEXT NOT NULL UNIQUE,
  state                  TEXT NOT NULL
    CHECK(state IN ('running', 'degraded', 'stopped', 'failed')),
  health                 TEXT NOT NULL CHECK(health IN ('healthy', 'unhealthy', 'unknown')),
  source                 TEXT NOT NULL,
  evidence_json          TEXT NOT NULL,
  observed_at            TEXT NOT NULL,
  imported_at            TEXT NOT NULL,
  UNIQUE(repository_id, native_project)
);
CREATE TABLE IF NOT EXISTS observed_containers (
  container_id           TEXT PRIMARY KEY,
  observed_deployment_id TEXT NOT NULL
    REFERENCES observed_deployments(observed_deployment_id) ON DELETE CASCADE,
  repository_id          TEXT NOT NULL REFERENCES repositories(repository_id),
  name                   TEXT NOT NULL,
  image                  TEXT NOT NULL,
  compose_service        TEXT NOT NULL,
  state                  TEXT NOT NULL
    CHECK(state IN ('running', 'stopped', 'failed', 'starting', 'missing')),
  status                 TEXT NOT NULL,
  health                 TEXT NOT NULL
    CHECK(health IN ('healthy', 'unhealthy', 'starting', 'unknown', 'none')),
  observed_at            TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS observed_containers_deployment
  ON observed_containers(observed_deployment_id);
CREATE INDEX IF NOT EXISTS observed_containers_repository
  ON observed_containers(repository_id);
CREATE TABLE IF NOT EXISTS observed_routes (
  domain                 TEXT PRIMARY KEY,
  observed_deployment_id TEXT NOT NULL
    REFERENCES observed_deployments(observed_deployment_id) ON DELETE CASCADE,
  component              TEXT NOT NULL,
  port                   INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
  public                 INTEGER NOT NULL CHECK(public IN (0, 1)),
  evidence_json          TEXT NOT NULL,
  observed_at            TEXT NOT NULL
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
            current = int(row["value"]) if row is not None else None
            if current is not None and current > SCHEMA_VERSION:
                raise SchemaMismatch(
                    f"database schema {current} is newer than daemon {SCHEMA_VERSION}"
                )
            # Additive upgrades only: control data (repositories, deployments,
            # ports, domains) is always preserved (docs/database-ledger.md).
            self._conn.executescript(_SCHEMA_V2)
            self._conn.executescript(_SCHEMA_V3)
            self._conn.executescript(_SCHEMA_V4)
            self._conn.executescript(_SCHEMA_V5)
            self._conn.executescript(_SCHEMA_V6)
            self._ensure_column("deployments", "public", "INTEGER NOT NULL DEFAULT 0")
            # Schema 7: an administrator may override the routed domain from
            # the Console/CLI; the override survives re-apply until cleared.
            self._ensure_column("deployments", "domain_override", "TEXT")
            self._relax_observed_state_checks()
            self._conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', ?)",
                (str(SCHEMA_VERSION),),
            )
            self._conn.commit()

    def _ensure_column(self, table: str, column: str, ddl: str) -> None:
        cols = {r[1] for r in self._conn.execute(f"PRAGMA table_info({table})")}
        if column not in cols:
            self._conn.execute(f"ALTER TABLE {table} ADD COLUMN {column} {ddl}")

    def _relax_observed_state_checks(self) -> None:
        """Schema 6→7: rebuild the two observed tables whose CHECK constraints
        only allowed 'running' states, preserving every imported row.
        legacy_alter_table keeps observed_routes' REFERENCES pointing at the
        table name (i.e. the rebuilt table), not the renamed original."""
        row = self._conn.execute(
            "SELECT sql FROM sqlite_master WHERE type='table'"
            " AND name='observed_deployments'").fetchone()
        if row is None or "'stopped'" in row["sql"]:
            return
        self._conn.execute("PRAGMA foreign_keys=OFF")
        self._conn.execute("PRAGMA legacy_alter_table=ON")
        try:
            self._conn.executescript(
                "ALTER TABLE observed_deployments RENAME TO observed_deployments_v6;\n"
                "ALTER TABLE observed_containers RENAME TO observed_containers_v6;\n"
                + _SCHEMA_V6 +
                "\nINSERT INTO observed_deployments SELECT * FROM observed_deployments_v6;"
                "\nINSERT INTO observed_containers SELECT * FROM observed_containers_v6;"
                "\nDROP TABLE observed_containers_v6;"
                "\nDROP TABLE observed_deployments_v6;"
                # The renames dragged the indexes to the _v6 tables (making the
                # CREATE INDEX IF NOT EXISTS above a no-op), so the drops above
                # removed them; recreate them on the rebuilt table.
                "\nCREATE INDEX IF NOT EXISTS observed_containers_deployment"
                " ON observed_containers(observed_deployment_id);"
                "\nCREATE INDEX IF NOT EXISTS observed_containers_repository"
                " ON observed_containers(repository_id);")
        finally:
            self._conn.execute("PRAGMA legacy_alter_table=OFF")
            self._conn.execute("PRAGMA foreign_keys=ON")

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
