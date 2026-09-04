"""SQLite authority database: open, schema, transactions.

The table inventory is tracked in docs/database-ledger.md; every version bump
updates that ledger in the same change. Tests deliberately have no tables
(repository-local files only).
"""

from __future__ import annotations

import sqlite3
import threading
from contextlib import contextmanager
from pathlib import Path

SCHEMA_VERSION = 15

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

# Schema 8: planning, completion ledger, and decision history
# (DC2-2026-08-24-PLANNING-LEDGER). The product's first append-only permanent
# history: rows are never deleted; every task/release mutation appends
# plan_events in the same transaction, and decisions/summaries only accumulate
# (the single later UPDATE sets decisions.superseded_by once). Enums are
# daemon-validated, not CHECKed — schema 7 showed CHECK changes force a
# table rebuild. Release delivery snapshots generation evidence because
# generations are pruned to current+previous.
_SCHEMA_V8 = """
CREATE TABLE IF NOT EXISTS releases (
  release_id        TEXT PRIMARY KEY,
  repository_id     TEXT NOT NULL REFERENCES repositories(repository_id),
  seq               INTEGER NOT NULL,
  name              TEXT NOT NULL,
  kind              TEXT NOT NULL,
  status            TEXT NOT NULL,
  note              TEXT,
  requested_at      TEXT,
  delivered_at      TEXT,
  deployment_id     TEXT,
  generation_number INTEGER,
  commit_hash       TEXT,
  dirty             INTEGER,
  fingerprint       TEXT,
  url               TEXT,
  port              INTEGER,
  created_at        TEXT NOT NULL,
  created_by        TEXT NOT NULL,
  updated_at        TEXT NOT NULL,
  UNIQUE(repository_id, seq)
);
CREATE TABLE IF NOT EXISTS tasks (
  task_id           TEXT PRIMARY KEY,
  repository_id     TEXT NOT NULL REFERENCES repositories(repository_id),
  parent_task_id    TEXT REFERENCES tasks(task_id),
  release_id        TEXT REFERENCES releases(release_id),
  seq               INTEGER NOT NULL,
  position          INTEGER NOT NULL,
  title             TEXT NOT NULL,
  outcome           TEXT NOT NULL,
  impact            TEXT,
  unblock_condition TEXT,
  verification      TEXT,
  technical_note    TEXT,
  kind              TEXT NOT NULL,
  status            TEXT NOT NULL,
  estimated_loc     INTEGER,
  elaboration_needed INTEGER NOT NULL DEFAULT 0,
  created_at        TEXT NOT NULL,
  created_by        TEXT NOT NULL,
  updated_at        TEXT NOT NULL,
  UNIQUE(repository_id, seq)
);
CREATE INDEX IF NOT EXISTS tasks_repository_status ON tasks(repository_id, status);
CREATE INDEX IF NOT EXISTS tasks_release ON tasks(release_id);
CREATE INDEX IF NOT EXISTS tasks_parent ON tasks(parent_task_id);
CREATE TABLE IF NOT EXISTS plan_events (
  event_id      INTEGER PRIMARY KEY AUTOINCREMENT,
  repository_id TEXT NOT NULL REFERENCES repositories(repository_id),
  subject_kind  TEXT NOT NULL,
  subject_id    TEXT NOT NULL,
  event         TEXT NOT NULL,
  from_value    TEXT,
  to_value      TEXT,
  actor         TEXT NOT NULL,
  at            TEXT NOT NULL,
  note          TEXT
);
CREATE INDEX IF NOT EXISTS plan_events_subject ON plan_events(subject_kind, subject_id);
CREATE TABLE IF NOT EXISTS decisions (
  decision_id    TEXT PRIMARY KEY,
  repository_id  TEXT NOT NULL REFERENCES repositories(repository_id),
  seq            INTEGER NOT NULL,
  ref            TEXT,
  aspect         TEXT NOT NULL,
  title          TEXT NOT NULL,
  body           TEXT NOT NULL,
  technical_note TEXT,
  superseded_by  TEXT REFERENCES decisions(decision_id),
  created_at     TEXT NOT NULL,
  created_by     TEXT NOT NULL,
  UNIQUE(repository_id, seq)
);
CREATE INDEX IF NOT EXISTS decisions_repository_aspect ON decisions(repository_id, aspect);
CREATE UNIQUE INDEX IF NOT EXISTS decisions_ref ON decisions(repository_id, ref)
  WHERE ref IS NOT NULL;
CREATE TABLE IF NOT EXISTS decision_summaries (
  repository_id      TEXT NOT NULL REFERENCES repositories(repository_id),
  covers_through_seq INTEGER NOT NULL,
  body               TEXT NOT NULL,
  created_at         TEXT NOT NULL,
  created_by         TEXT NOT NULL,
  PRIMARY KEY(repository_id, covers_through_seq)
);
"""

# Decision full-text search (owner and agents both search every decision).
# Kept out of _SCHEMA_V8 so a missing FTS5 module fails with a clear message
# instead of half-applying the script.
_SCHEMA_V8_FTS = """
CREATE VIRTUAL TABLE IF NOT EXISTS decisions_fts USING fts5(
  title, body, technical_note, ref,
  content='decisions', content_rowid='rowid'
);
CREATE TRIGGER IF NOT EXISTS decisions_fts_insert AFTER INSERT ON decisions BEGIN
  INSERT INTO decisions_fts(rowid, title, body, technical_note, ref)
  VALUES (new.rowid, new.title, new.body, new.technical_note, new.ref);
END;
"""

# Schema 9: bounded receipts for successful finite Compose services. These are
# control evidence for the current/previous deployment generations, not logs.
_SCHEMA_V9 = """
CREATE TABLE IF NOT EXISTS compose_completions (
  deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
  component     TEXT NOT NULL,
  service       TEXT NOT NULL,
  generation    INTEGER NOT NULL,
  container_id  TEXT NOT NULL,
  image_id      TEXT,
  exit_code     INTEGER NOT NULL,
  started_at    TEXT,
  finished_at   TEXT,
  recorded_at   TEXT NOT NULL,
  PRIMARY KEY(deployment_id, component, service, generation)
);
CREATE INDEX IF NOT EXISTS compose_completions_generation
  ON compose_completions(deployment_id, generation);
CREATE TABLE IF NOT EXISTS compose_service_desires (
  deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
  component     TEXT NOT NULL,
  service       TEXT NOT NULL,
  desired_state TEXT NOT NULL,
  updated_at    TEXT NOT NULL,
  PRIMARY KEY(deployment_id, component, service)
);
"""

# Schema 10: rebuildable links from a Coordinator repository to the
# privacy-preserving repository key in one configured Codex usage collector.
# The usage facts stay canonical in that collector and are never copied here.
_SCHEMA_V10 = """
CREATE TABLE IF NOT EXISTS codex_usage_repository_links (
  source_uid          INTEGER NOT NULL,
  repository_id       TEXT NOT NULL REFERENCES repositories(repository_id),
  codex_repository_id TEXT NOT NULL CHECK(length(codex_repository_id) = 64),
  source_schema       INTEGER NOT NULL,
  taxonomy_version    INTEGER NOT NULL,
  resolved_at         TEXT NOT NULL,
  PRIMARY KEY(source_uid, repository_id)
);
CREATE INDEX IF NOT EXISTS codex_usage_links_repository
  ON codex_usage_repository_links(repository_id);
"""

# Schema 12: preserve retired repository history while removing obsolete
# repositories from normal product collections.
_SCHEMA_V12 = """
CREATE TABLE IF NOT EXISTS repository_events (
  event_id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  repository_id             TEXT NOT NULL REFERENCES repositories(repository_id),
  event                     TEXT NOT NULL CHECK(event IN ('archived','unarchived')),
  merged_into_repository_id TEXT,
  actor_uid                 INTEGER NOT NULL,
  at                        TEXT NOT NULL,
  note                      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS repository_events_repository
  ON repository_events(repository_id, event_id);
"""

# Schema 13: host-wide governed-test capacity. The singleton is permanent
# control state; every automatic or administrator adjustment is append-only
# evidence. Host samples themselves are deliberately not retained.
_SCHEMA_V13 = """
CREATE TABLE IF NOT EXISTS test_capacity_state (
  singleton        INTEGER PRIMARY KEY CHECK(singleton = 1),
  learned_capacity INTEGER NOT NULL CHECK(learned_capacity >= 1),
  cap              INTEGER CHECK(cap IS NULL OR cap >= 1),
  updated_at       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS test_capacity_events (
  event_id            INTEGER PRIMARY KEY AUTOINCREMENT,
  at                  TEXT NOT NULL,
  actor               TEXT NOT NULL,
  reason              TEXT NOT NULL,
  previous_capacity   INTEGER NOT NULL,
  new_capacity        INTEGER NOT NULL,
  cap                 INTEGER,
  p95_cpu_percent     REAL,
  p95_memory_percent  REAL,
  saturation_fraction REAL,
  epoch_seconds       REAL
);
CREATE INDEX IF NOT EXISTS test_capacity_events_time
  ON test_capacity_events(event_id);
"""

# Schema 14: host-wide governed-test log retention policy. Test logs and
# catalogues remain repository-local disposable evidence; only the owner's
# policy and its append-only administrative history live in the authority DB.
_SCHEMA_V14 = """
CREATE TABLE IF NOT EXISTS test_log_retention_state (
  singleton               INTEGER PRIMARY KEY CHECK(singleton = 1),
  max_age_seconds         INTEGER NOT NULL CHECK(max_age_seconds >= 1),
  case_depth              INTEGER NOT NULL CHECK(case_depth >= 1),
  updated_at              TEXT NOT NULL,
  updated_by              TEXT NOT NULL,
  last_cleanup_at         TEXT,
  last_cleanup_error_code TEXT
);
INSERT OR IGNORE INTO test_log_retention_state(
  singleton, max_age_seconds, case_depth, updated_at, updated_by
) VALUES(1, 86400, 3, strftime('%Y-%m-%dT%H:%M:%SZ','now'), 'schema-default');
CREATE TABLE IF NOT EXISTS test_log_retention_events (
  event_id                INTEGER PRIMARY KEY AUTOINCREMENT,
  at                      TEXT NOT NULL,
  actor                   TEXT NOT NULL,
  previous_max_age_seconds INTEGER NOT NULL,
  max_age_seconds         INTEGER NOT NULL,
  previous_case_depth     INTEGER NOT NULL,
  case_depth              INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS test_log_retention_events_time
  ON test_log_retention_events(event_id);
"""

# Schema 15: immutable screenshot anchors and their owner/agent discussion.
# The linked Plan task remains the authoritative completion item; these tables
# retain only the visual context and comment projection needed to act on it.
_SCHEMA_V15 = """
CREATE TABLE IF NOT EXISTS visual_feedback (
  feedback_id       TEXT PRIMARY KEY,
  task_id           TEXT NOT NULL UNIQUE REFERENCES tasks(task_id),
  repository_id     TEXT NOT NULL REFERENCES repositories(repository_id),
  worktree_id       TEXT NOT NULL,
  run_id            TEXT NOT NULL,
  check_name        TEXT NOT NULL,
  phase             TEXT NOT NULL,
  case_id           TEXT,
  formal_run_id     TEXT NOT NULL,
  cell_id           TEXT NOT NULL,
  review_cell_key   TEXT,
  screenshot_kind   TEXT NOT NULL,
  screenshot_sha256 TEXT NOT NULL,
  image_id          TEXT NOT NULL,
  geometry_json     TEXT NOT NULL,
  root_comment_id   TEXT NOT NULL,
  created_at        TEXT NOT NULL,
  created_by        TEXT NOT NULL,
  updated_at        TEXT NOT NULL,
  deleted_at        TEXT,
  deleted_by        TEXT
);
CREATE INDEX IF NOT EXISTS visual_feedback_run
  ON visual_feedback(repository_id, worktree_id, run_id);
CREATE INDEX IF NOT EXISTS visual_feedback_image
  ON visual_feedback(image_id);
CREATE TABLE IF NOT EXISTS visual_feedback_comments (
  comment_id  TEXT PRIMARY KEY,
  feedback_id TEXT NOT NULL REFERENCES visual_feedback(feedback_id),
  seq         INTEGER NOT NULL,
  body        TEXT NOT NULL,
  created_at  TEXT NOT NULL,
  created_by  TEXT NOT NULL,
  updated_at  TEXT NOT NULL,
  deleted_at  TEXT,
  deleted_by  TEXT,
  UNIQUE(feedback_id, seq)
);
CREATE INDEX IF NOT EXISTS visual_feedback_comments_thread
  ON visual_feedback_comments(feedback_id, created_at);
CREATE TABLE IF NOT EXISTS visual_feedback_events (
  event_id    INTEGER PRIMARY KEY AUTOINCREMENT,
  feedback_id TEXT NOT NULL REFERENCES visual_feedback(feedback_id),
  event       TEXT NOT NULL,
  comment_id  TEXT,
  from_value  TEXT,
  to_value    TEXT,
  actor       TEXT NOT NULL,
  at          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS visual_feedback_events_thread
  ON visual_feedback_events(feedback_id, event_id);
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
            self._conn.executescript(_SCHEMA_V8)
            try:
                self._conn.executescript(_SCHEMA_V8_FTS)
            except sqlite3.OperationalError as exc:
                raise SchemaMismatch(
                    "SQLite FTS5 is required for decision search (schema 8) and is"
                    f" missing from this SQLite build: {exc}"
                ) from exc
            self._conn.executescript(_SCHEMA_V9)
            self._conn.executescript(_SCHEMA_V10)
            # Schema 11: owner clarification requests stay attached to tasks
            # until an agent saves clearer owner-facing wording.
            self._ensure_column(
                "tasks", "elaboration_needed", "INTEGER NOT NULL DEFAULT 0")
            # Schema 12: archival is reversible and every transition remains
            # in repository_events. The replacement id is deliberately a
            # nullable scalar because SQLite cannot add a foreign key with an
            # additive ALTER TABLE migration.
            self._ensure_column("repositories", "archived_at", "TEXT")
            self._ensure_column("repositories", "archived_by_uid", "INTEGER")
            self._ensure_column("repositories", "archive_note", "TEXT")
            self._ensure_column("repositories", "merged_into_repository_id", "TEXT")
            self._conn.executescript(_SCHEMA_V12)
            self._conn.executescript(_SCHEMA_V13)
            self._conn.executescript(_SCHEMA_V14)
            self._conn.executescript(_SCHEMA_V15)
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
