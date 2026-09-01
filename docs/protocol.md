# Local Unix-Socket JSON Protocol

Version: `protocol: 1`. One shared request/response/result schema serves the
CLI, the MCP server, and (later) the Console API. Command-level schemas are
in `contract-commands.md`.

## Transport and framing

- Unix stream socket. Default path `/run/devcoordinator2/daemon.sock`,
  overridable via instance configuration (`docs/instance-configuration.md`).
- Socket mode 0666, owned by root and the daemon client group
  (DC2-2026-08-24-OPEN-LOCAL-ACCESS: every local Unix account is a trusted
  caller, connectable even from sandboxes whose user namespace maps the
  client group away; the group remains for organizational ownership only).
  The kernel peer credentials (`SO_PEERCRED`: pid, uid, gid) are read on
  accept; the **uid is the physical caller identity** for every request.
  Nothing in a request body can assert or override identity.
- One connection carries exactly one request and one response, then closes.
  UTF-8 JSON, single line, terminated by `\n`.
- Once the daemon accepts and begins a mutation, client disconnect or a lost
  response does not cancel it. The daemon completes the observable operation;
  the client re-queries status. Deployment mutations remain mutually exclusive
  and conflicting actions return `busy` while the original operation runs.
- Limits: request ≤ 65536 bytes, response ≤ 262144 bytes. Server read
  timeout 5 s, write timeout 10 s. No streaming, sessions, or pipelining.

## Request

```json
{
  "protocol": 1,
  "id": "<client-generated opaque string, echoed back>",
  "command": "test.start",
  "args": { },
  "client": {"kind": "codex", "session": "<optional task id>"}
}
```

- `client` is descriptive attribution only (`codex`, `claude`, `cursor`,
  `antigravity`, `human`, `other`); it is recorded, never trusted.
- Unknown top-level or `args` keys are rejected (`args_invalid`).

## Response

Success: `{"protocol": 1, "id": "…", "ok": true, "result": { }}`

Failure:

```json
{
  "protocol": 1, "id": "…", "ok": false,
  "error": {
    "code": "test_start_failed",
    "message": "<one sentence>",
    "detail": "<bounded diagnostic, ≤ 4096 bytes, may be empty>"
  }
}
```

## Error codes

Stable snake_case, terminal (no retry/queue semantics):

| code | meaning |
|---|---|
| `protocol_invalid` | not JSON, wrong `protocol`, missing envelope fields |
| `request_too_large` | request frame over 64 KiB |
| `command_unknown` | command not in the registry |
| `args_invalid` | args fail the command schema |
| `repository_not_found` | path is not inside a registered/registerable Git repository |
| `repository_config_invalid` | `.devcoordinator.toml` fails validation |
| `worktree_busy` | per-worktree start lock not acquired within the bounded wait |
| `test_not_found` | no current test run for the worktree |
| `test_start_failed` | terminal launch failure (never `queued`) |
| `tests_draining` | a normal Coordinator upgrade has closed test admission; never queued |
| `unit_stop_failed` | prior unit would not stop / cgroup not proven empty |
| `task_not_found` | no ledger task with that id |
| `release_not_found` | no release with that id |
| `decision_not_found` | no decision with that id or ref |
| `internal_error` | unexpected daemon fault (bounded diagnostic in `detail`) |

## Result conventions

- Results are compact conclusions plus exact file or continuation
  references (`summary_path`, `log_path`); never raw logs or metric series.
- Mutation success means the requested observable state was reached (e.g.
  `test.start` returns only after the unit's process exists), not merely
  that a handler ran or a row was saved.
- Timestamps are UTC ISO-8601 with seconds. Sizes are byte integers.
- IDs are opaque strings with stable one-letter prefixes
  (`r` repository, `w` worktree, `t` test run, `d` deployment,
  `p` plan task, `v` release, `n` decision — see `database-ledger.md`).
