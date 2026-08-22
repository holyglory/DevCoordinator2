# Health Metrics Notes (Phase 4 — design only)

Owner decision DC2-2026-08-22-METRICS. No code exists yet; this note fixes
the cadence and the schema direction so earlier phases don't contradict it.

## Cadence and retention

- CPU and memory sampled every 15 seconds from cgroups (managed units) and
  the Docker stats API (managed containers).
- One-minute aggregates persisted (min/avg/max per series).
- Storage sampled every 5 minutes (directory/volume/data-dir sizes).
- Retention: **30 days** (owner extended the original 7-day proposal).
  Expired rows are deleted directly; no downsampling tiers, no backup, no
  migration of metric history.

## Series model (sketch)

One bounded time-series table keyed by
`(subject_kind, subject_id, metric, minute_utc)` with `min/avg/max` REAL
columns. Subject kinds: host, repository, deployment, component, test,
container, postgres, shared/unattributed. Reconciliation invariant:

```
managed repositories + DevCoordinator + shared/unattributed = host total
```

Never invent per-database CPU/memory for shared PostgreSQL; report shared
overhead as shared. Never collect query text, row values, credentials, or
connection strings.

## Alerts

Sustained-threshold alerts with active-state deduplication and a single
recovery message; current alerts live in the authority DB (`database-ledger.md`).
