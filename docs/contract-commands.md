# Command Contract

Envelope and error model: `protocol.md`. Full schemas below exist for the
commands implemented in this delivery (`test.*`, `repository.*`, `ping`).
Later command families are sketched to reserve names and result shapes; they
have no implementation and must not be exposed before their end-to-end
behavior exists.

The CLI maps `devcoordinator2 test start …` to command `test.start`; the MCP
server exposes the same commands as tools `test_start`, `test_status`,
`test_output`, `test_stop`, `repository_list`. All three surfaces return the
identical result JSON.

## ping

Args: none.
Result: `{"daemon_version": "<semver>", "schema_version": 1, "socket": "<path>"}`

## test.start

Args:
- `path` (string, required) — any path inside the target worktree.
- `test` (string, optional) — named test from `.devcoordinator.toml`;
  defaults to the file's declared default.

Result (only after the process exists):

```json
{
  "run_id": "t20260822T120000Z-1a2b3c",
  "repository_id": "r…", "worktree_id": "w…",
  "test": "unit",
  "status": "running",
  "unit": "devcoordinator2-test-<worktree_id>-<suffix>.service",
  "summary_path": "<worktree>/.devcoordinator/test/current/summary.json"
}
```

Errors: `repository_not_found`, `repository_config_invalid`,
`worktree_busy`, `unit_stop_failed`, `test_start_failed`. Never `queued`.
Latest-start-wins: a concurrent earlier run ends `superseded`.

## test.status

Args: `path` (required).
Result: the current `summary.json` fields (see below) plus `summary_path`.
Successful runs carry **no log text**. Error: `test_not_found`.

summary.json fields (result schema 1): `schema_version`, `run_id`, `test`,
`status` (`running|passed|failed|timed-out|cancelled|interrupted|superseded`),
`started_at`, `finished_at` (null while running), `duration_seconds`,
`exit_code` (null unless exited), `stdout_bytes_observed`,
`stdout_bytes_retained`, `stderr_bytes_observed`, `stderr_bytes_retained`,
`stdout_truncated`, `stderr_truncated`, `caller_uid`, `client`.

## test.output

Args:
- `path` (required)
- `stream` — `"stdout"` or `"stderr"` (required)
- `tail_bytes` — 1..65536, default 16384

Result: `{"run_id", "stream", "tail": "<utf-8, lossy-decoded>",
"tail_bytes": n, "truncated_before_tail": bool, "log_path": "…"}`

## test.stop

Args: `path` (required).
Result: `{"run_id", "status": "cancelled"}` after the cgroup is proven
empty, or `{"run_id", "status": "<terminal>", "already_finished": true}`.
Errors: `test_not_found`, `unit_stop_failed`.

## repository.register

Args: `path` (required). Registration also happens implicitly on first
`test.start`.
Result: `{"repository_id", "worktree_id", "root_path", "worktree_path",
"display_name", "registered": true|false}` (`false` = already known).

## repository.list

Args: none.
Result: `{"repositories": [{"repository_id", "root_path", "display_name",
"registered_at", "last_seen_at", "worktrees": [{"worktree_id",
"worktree_path"}]}]}`

## repository.status

Args: `path`.
Result: one repository object as above plus, when present, the current test
summary reference (`summary_path`, `status`).

## deployment.* (Phase 3, implemented)

Reference args on every command except `list`: `path` (required) plus
`name` (`web` or `web@checkout`) or `deployment_id`.

- `deployment.list {path?}` → `{deployments: [...], declared: [...]}` — all
  applied deployments; with `path`, also the declared-but-not-applied
  `name`/`source`/`deployment_id` triples of that repository.
- `deployment.apply` → status (below) after: validate, fingerprint (spec +
  commit + dirty flag), reserve ports/domain transactionally, prepare the
  candidate (checkout worktree + optional build), start components in
  declared order (blue/green for generation-scoped components, in-place for
  stable data-owning ones), prove health, publish the route atomically,
  retire the previous generation in reverse order. Errors:
  `deployment_apply_failed` (detail = JSON of component states; candidate
  components are stopped and removed; nothing is routed), `busy`,
  `repository_config_invalid`. An identical running specification returns
  the status with `unchanged: true`.
- `deployment.status` → `{deployment_id, name, source, repository_id, state
  (running|stopped|degraded|applying|failed), current_generation,
  previous_generation, domain, route_port, ttl_expires_at, components:
  [{name, type, state, health, generation, binding{kind, identity}, port,
  restarts, owned, independent_control, last_error}], log_dir}`.
- `deployment.start | stop | restart {component?}` → status. Whole
  deployment in declared/reverse order, or one component whose declaration
  permits independent control. Stop withdraws the route, never deletes data.
- `deployment.logs {component, tail_lines<=5000}` → `{component, tail,
  log_path | container_id}`.
- `deployment.rollback` → status with `rolled_back_from`/`rolled_back_to`;
  checkout source only (`rollback_unavailable` otherwise).
- `deployment.remove {delete_data=false}` → `{removed, data_deleted,
  deleted_volumes}`. Stops everything, removes containers/units/generation
  checkouts and records; named volumes and PostgreSQL data survive unless
  `delete_data` is true. Repository `persistent_paths` are never touched.

## health.containers (Phase 3, implemented)

Args: none. Result: `{containers: [{id, name, image, state, status, created,
repository_id, deployment_id, component, run_id, caller_uid, client,
ttl_seconds, data, classification}], counts}` where classification is one
of `managed-test`, `managed-preview`, `managed-permanent`,
`orphaned-managed`, `unmanaged`. Classification uses daemon-owned labels
and recorded bindings only — never names, ports, images, or paths.

## health.summary | repositories | repository | history (Phase 4, implemented)

- `health.summary` → `{host: {cpu_percent, memory_total/used/available,
  swap_total/used, load_1/5/15, fs_size/free/used, ncpu, reconciliation:
  {managed_cpu_percent, daemon_cpu_percent, other_cpu_percent,
  managed_memory, daemon_memory, other_memory}}, storage: {fs_used,
  managed_repositories, devcoordinator_state, docker_shared, docker_images,
  docker_build_cache, docker_shared_volumes, other}, unhealthy_deployments,
  active_tests, container_counts, alerts, sampling}`.
- `health.repositories` → one row per repository with live `cpu_percent`,
  `memory_bytes`, `storage` buckets (checkout, test_scratch,
  deployment_artifacts, container_layers, volumes, postgres_data, total),
  `health`, deployments, and 12-point `trend_cpu`/`trend_memory`; plus the
  `devcoordinator` and `shared_unattributed` rows that complete the
  reconciliation `managed + DevCoordinator + shared/unattributed = host`.
- `health.repository {path}` → every measured subject of one repository
  (component, container, test) with live cgroup metrics and storage; dedicated
  PostgreSQL components carry `pg_connections`, `pg_wal_bytes`,
  `pg_temp_bytes`, `pg_database_bytes` (numbers only, never content).
- `health.history {subject_kind, subject_id, metric, minutes<=43200}` →
  one-minute `{minute, min, avg, max, samples}` points, at most 1440 per
  call (`truncated` flag), from the 30-day bounded store.

Alerts (in `health.summary.alerts` and as `alert.opened`/`alert.recovered`
events): host CPU > 90% for 5 min, host memory available < 10% for 5 min,
root filesystem < 10% free, component not running for 2 min while desired
running, ≥ 3 restarts in 10 min (crash loop), test scratch > 10 GiB.
Deduplicated while active; one recovery each.

## Reserved sketches (later phases)

- `bug.report | list | close` — backed by the independent open-only store
  (Phase 6).

All follow the same envelope, error model, and file-reference conventions.
