# Database Completion Ledger

One SQLite authority database (default
`/var/lib/devcoordinator2/authority.sqlite3`; dev override via instance
configuration). WAL mode, foreign keys on. Table count is an architectural
budget, not a target. Tests use repository-local files — **no test tables,
ever**. Docker/process observations are current projections plus bounded
samples, not an append-only archive.

## Schema version 1 (Phase 1)

| Table | Fields | Status |
|---|---|---|
| `meta` | key PK, value (`schema_version`, `route_generation`) | done |
| `repositories` | repository_id PK, root_path UNIQUE, display_name, registered_at, registered_by_uid, last_seen_at | done |
| `worktrees` | worktree_id PK, repository_id FK, worktree_path UNIQUE, registered_at, last_seen_at | done |

## Schema version 2 (Phase 3) — additive upgrade, schema 1 rows preserved

| Table | Fields | Status |
|---|---|---|
| `deployments` | deployment_id PK, repository_id FK, worktree_id FK, name, source, domain, spec_fingerprint, spec_json (secret-free), state, current_generation, previous_generation, created_at, created_by_uid, client, updated_at, ttl_expires_at; UNIQUE(worktree_id, name, source) | done |
| `generations` | (deployment_id, number) PK, commit_hash, dirty, path, fingerprint, created_at, state (candidate/current/previous/failed) | done |
| `components` | (deployment_id, name) PK, type, order_index, spec_fingerprint, desired_state, state, health, generation, binding_kind (unit/container/compose), binding_identity (exact unit name / full container ID / compose project), restarts, last_error, updated_at | done |
| `port_assignments` | port PK, deployment_id, component, generation (0 = stable component), assigned_at | done |
| `domain_routes` | domain PK, deployment_id, component, port (NULL = withdrawn), generation, published_at | done |

Secrets (generated PostgreSQL credentials) are **not** in the database: they
live in root-only 0600 files under the instance secrets directory.

## Schema version 3 (Phase 4) — disposable, rebuildable

| Table | Fields | Status |
|---|---|---|
| `metric_minutes` | (subject_kind, subject_id, metric, minute_utc) PK, min/avg/max, samples; index on minute_utc; rows older than 30 days deleted directly | done |
| `alerts` | alert_key PK, kind, subject_kind, subject_id, severity, message, opened_at, last_seen_at — current alerts only; resolved rows are deleted | done |

## Schema version 4 (Phase 5) — control data, always preserved

| Table | Fields | Status |
|---|---|---|
| `users` | user_id PK, email UNIQUE, subject, display_name, administrator, created_at, created_by, last_seen_at | done |
| `invitations` | invitation_id PK, email UNIQUE, administrator, grants_json, created_at, created_by, expires_at | done |
| `grants` | (user_id, deployment_id) PK, role, granted_at, granted_by | done |
| `deployments.public` | added column (route needs no sign-in) | done |

## Schema version 5 (Phase 6)

| Table | Fields | Status |
|---|---|---|
| `telegram_chats` | chat_id PK, email, label, linked_at | done |
| `telegram_links` | code PK, chat_id, label, created_at, expires_at (15 min) | done |
| `telegram_subscriptions` | (chat_id, scope) PK, created_at | done |
| `telegram_outbox` | message_id PK, chat_id, text, created_at, attempts, next_attempt_at, last_error — bounded (1000 rows / 10 attempts / 24 h) | done |

Bugs are **not** in this database: `DEVCOORDINATOR2_BUGS_DIR` holds one
atomic JSON file per open bug.

## Reserved ID-prefix namespace

Deterministic opaque TEXT IDs; later phases never migrate existing IDs.

| Prefix | Entity | Derivation | Phase |
|---|---|---|---|
| `r` | repository | sha256("devcoordinator2.repository\0" + realpath(git common root))[:16] | 1 |
| `w` | worktree | sha256("devcoordinator2.worktree\0" + realpath(worktree root))[:16] | 1 |
| `t` | test run | UTC timestamp + random suffix (not stored in DB) | 1 |
| `d` | deployment | sha256("devcoordinator2.deployment\0" + worktree_id + name + source)[:16] | 3 (done) |
| `c` | component | not needed: components are keyed (deployment_id, name) | — |
| `g` | generation | (deployment_id, number) counter | 3 (done) |
| `u` | public user | random at creation | 5 (done); `i` invitation |
| `b` | bug | random at creation (independent store, not this DB) | 6 |

## Later-phase entities (from the handover's durable-state list)

| Entity | Phase | Notes |
|---|---|---|
| deployments, components | 3 | done |
| deployment generations | 3 | done (current + previous only) |
| port assignments, domain routes | 3 | done |
| current managed native identities | 3 | done (components.binding_*) |
| current Docker observations | 3/4 | projection, rebuildable |
| bounded health samples | 4 | done |
| current alerts | 4 | done |
| users, invitations, deployment grants | 5 | done |
| Telegram subscriptions + bounded outbox | 6 | done |

## Change rules

- Schema changes may rebuild disposable projections and metrics but must
  preserve repositories, active deployments, ports/domains,
  users/invites/grants, Telegram configuration, persistent-data identities,
  and current route state.
- Every schema version bump updates this ledger in the same change.
