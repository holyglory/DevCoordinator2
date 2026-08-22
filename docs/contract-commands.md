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

## Reserved sketches (later phases)

- `deployment.list | apply | status | start | stop | restart | logs` — apply
  validates config, fingerprints the spec, reserves ports/domains
  transactionally, converges generations, publishes routes atomically;
  concurrent mutation of one deployment returns error code `busy`.
- `health.summary | repositories | containers` — host condition, per-repo
  aggregates, full container inventory with ownership classification.
- `bug.report | list | close` — backed by the independent open-only store.

All follow the same envelope, error model, and file-reference conventions.
