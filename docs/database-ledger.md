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
| `u` | public user | random at creation | 5 |
| `b` | bug | random at creation (independent store, not this DB) | 6 |

## Later-phase entities (from the handover's durable-state list)

| Entity | Phase | Notes |
|---|---|---|
| deployments, components | 3 | done |
| deployment generations | 3 | done (current + previous only) |
| port assignments, domain routes | 3 | done |
| current managed native identities | 3 | done (components.binding_*) |
| current Docker observations | 3/4 | projection, rebuildable |
| bounded health samples | 4 | 1-min aggregates, 30-day retention, direct deletion |
| current alerts | 4 | active + dedup state |
| users, invitations, deployment grants | 5 | |
| Telegram subscriptions + bounded outbox | 6 | single server-owned bot; token in instance config, never in DB |

## Change rules

- Schema changes may rebuild disposable projections and metrics but must
  preserve repositories, active deployments, ports/domains,
  users/invites/grants, Telegram configuration, persistent-data identities,
  and current route state.
- Every schema version bump updates this ledger in the same change.
