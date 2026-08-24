# Database Completion Ledger

One SQLite authority database (default
`/var/lib/devcoordinator2/authority.sqlite3`; dev override via instance
configuration). WAL mode, foreign keys on. Table count is an architectural
budget, not a target. Tests use repository-local files — **no test tables,
ever**. Docker/process observations are current projections plus bounded
samples, not an append-only archive. The planning/decision tables (schema 8)
are the deliberate exception: the product's first append-only permanent
history — their rows are never deleted (DC2-2026-08-24-PLANNING-LEDGER).

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

## Schema version 6 (current observed-only import)

| Table | Fields | Status |
|---|---|---|
| `observed_deployments` | exact repository + native Compose project, current state/health, source, bounded evidence, observation/import time | done |
| `observed_containers` | full live container ID PK, observed deployment/repository, name/image/service, current state/status/health | done |
| `observed_routes` | domain label PK, observed deployment/component, verified live port, public flag, bounded evidence | done |

This is a replaceable current projection, not configuration authority or
history. Every current-state import atomically replaces all three tables
(including any domain set through `deployment.set_domain`). Stopped, removed,
missing, temporary, validation, test, and conflicting resources are not
retained by an import.

## Schema version 7 (owner-driven lifecycle and domain UX, 2026-08-24)

| Change | Reason | Status |
|---|---|---|
| `deployments.domain_override TEXT` | administrator-set routed domain that wins over the declared one on every apply until cleared (DC2-2026-08-24-DOMAIN-EDIT) | done |
| `observed_deployments.state` CHECK relaxed to running/degraded/stopped/failed; `observed_containers.state` to running/stopped/failed/starting/missing; `observed_containers.health` gains `none` | start/stop/restart now act on exact recorded containers, so non-running states must be recordable (DC2-2026-08-24-OBSERVED-LIFECYCLE) | done |

The CHECK relaxation rebuilds the two observed tables in place, preserving
every imported row and the two indexes.

## Schema version 8 (planning, completion ledger, decision history, 2026-08-24)

DC2-owned agent planning, the completion ledger (same tables), and per-repo
decision history (DC2-2026-08-24-PLANNING-LEDGER). Append-only permanent
history: no code path deletes rows; every task/release mutation appends
`plan_events` in the same transaction. Enums are daemon-validated, not
CHECKed (schema 7 showed CHECK changes force a table rebuild).

| Table | Fields | Status |
|---|---|---|
| `releases` | release_id PK, repository_id FK, seq (UNIQUE per repo), name (plain), kind (preview/release), status (planned/requested/delivered/dropped), note, requested_at, delivered_at, delivery-evidence snapshot (deployment_id without FK, generation_number, commit_hash, dirty, fingerprint, url, port — copied because generations are pruned), created_at/by, updated_at | done |
| `tasks` | task_id PK, repository_id FK, parent_task_id self-FK (tree of arbitrary depth), release_id FK (NULL = backlog), seq (immutable per-repo identity, UNIQUE), position (mutable sibling order), title/outcome (required plain language), impact, unblock_condition, verification, technical_note (agent-facing, never substitutes the plain fields), kind (goal/stub/improvement/user_feedback), status (planned/in_progress/done/dropped), estimated_loc (size in lines of code), created_at/by, updated_at; indexes on (repository_id,status), release_id, parent_task_id | done |
| `plan_events` | event_id PK AUTOINCREMENT, repository_id FK, subject_kind (task/release), subject_id, event (created/status/release_move/reparent/reorder/estimate/edited/requested/delivered), from_value, to_value, actor, at, note; index on (subject_kind, subject_id) | done |
| `decisions` | decision_id PK, repository_id FK, seq (UNIQUE per repo), ref (stable citation key, UNIQUE per repo when present), aspect (daemon enum), title/body (required management-facing plain language), technical_note, superseded_by (forward pointer, set once — the only UPDATE), created_at/by | done |
| `decisions_fts` | FTS5 external-content index over title/body/technical_note/ref, insert trigger (decision text is immutable); FTS5 availability is checked at open and refused with a clear error when missing | done |
| `decision_summaries` | (repository_id, covers_through_seq) PK, body, created_at/by — all summaries kept; the newest is "the story so far" | done |

## Reserved ID-prefix namespace

Deterministic opaque TEXT IDs; later phases never migrate existing IDs.

| Prefix | Entity | Derivation | Phase |
|---|---|---|---|
| `r` | repository | sha256("devcoordinator2.repository\0" + realpath(git common root))[:16] | 1 |
| `w` | worktree | sha256("devcoordinator2.worktree\0" + realpath(worktree root))[:16] | 1 |
| `t` | test run | UTC timestamp + random suffix (not stored in DB) | 1 |
| `d` | deployment | sha256("devcoordinator2.deployment\0" + worktree_id + name + source)[:16] | 3 (done) |
| `d` | observed deployment | sha256("devcoordinator2.observed-deployment\0" + repository_id + native project)[:16] | 6 (done; disjoint namespace) |
| `c` | component | not needed: components are keyed (deployment_id, name) | — |
| `g` | generation | (deployment_id, number) counter | 3 (done) |
| `u` | public user | random at creation | 5 (done); `i` invitation |
| `b` | bug | random at creation (independent store, not this DB) | 6 |
| `p` | plan task | random at creation | 8 (done) |
| `v` | release / preview release | random at creation | 8 (done) |
| `n` | decision | random at creation | 8 (done) |

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
- The schema 8 planning tables (`releases`, `tasks`, `plan_events`,
  `decisions`, `decision_summaries`) are permanent history: schema changes
  must preserve every row, and no code path may delete from them.
- Every schema version bump updates this ledger in the same change.
