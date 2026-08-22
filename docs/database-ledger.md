# Database Completion Ledger

One SQLite authority database (default
`/var/lib/devcoordinator2/authority.sqlite3`; dev override via instance
configuration). WAL mode, foreign keys on. Table count is an architectural
budget, not a target. Tests use repository-local files — **no test tables,
ever**. Docker/process observations are current projections plus bounded
samples, not an append-only archive.

## Schema version 1 (Phase 1 — this delivery)

| Table | Fields | Status |
|---|---|---|
| `meta` | key PK, value | planned |
| `repositories` | repository_id PK, root_path UNIQUE, display_name, registered_at, registered_by_uid, last_seen_at | planned |
| `worktrees` | worktree_id PK, repository_id FK, worktree_path UNIQUE, registered_at, last_seen_at | planned |

## Reserved ID-prefix namespace

Deterministic opaque TEXT IDs; later phases never migrate existing IDs.

| Prefix | Entity | Derivation | Phase |
|---|---|---|---|
| `r` | repository | sha256("devcoordinator2.repository\0" + realpath(git common root))[:16] | 1 |
| `w` | worktree | sha256("devcoordinator2.worktree\0" + realpath(worktree root))[:16] | 1 |
| `t` | test run | UTC timestamp + random suffix (not stored in DB) | 1 |
| `d` | deployment | deterministic from repository_id + declared name | 3 |
| `c` | component | deterministic from deployment_id + declared name | 3 |
| `g` | generation | deployment_id + counter | 3 |
| `u` | public user | random at creation | 5 |
| `b` | bug | random at creation (independent store, not this DB) | 6 |

## Later-phase entities (from the handover's durable-state list)

| Entity | Phase | Notes |
|---|---|---|
| deployments, components | 3 | desired state, bindings, creating caller/client |
| deployment generations | 3 | current + previous only |
| port assignments, domain routes | 3 | transactionally unique |
| current managed native identities | 3 | exact unit/container/compose/db bindings |
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
