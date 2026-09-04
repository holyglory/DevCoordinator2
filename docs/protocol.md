# Local Unix-Socket JSON Protocol

Version: `protocol: 2`. One generated request/response/data contract serves
the CLI, MCP server, and Console API. The committed JSON Schema 2020-12 bundle
is `contracts/devcoordinator2-v2.schema.json`; `contract-commands.md` explains
the domain operations.

## Transport and framing

- Unix stream socket. Default path `/run/devcoordinator2/daemon.sock`,
  overridable via instance configuration (`docs/instance-configuration.md`).
- Socket mode 0666, owned by root and the daemon client group
  (DC2-2026-08-24-OPEN-LOCAL-ACCESS: every local Unix account is a trusted
  caller, connectable even from sandboxes whose user namespace maps the
  client group away; the group remains for organizational ownership only).
  The kernel peer credentials (`SO_PEERCRED`: pid, uid, gid) are read on
  accept; the **uid is the physical caller identity** for every request. Only
  the exact configured edge uid may add a signed-in public e-mail identity;
  every other body identity assertion is rejected.
- One connection carries exactly one request and one response, then closes.
  UTF-8 JSON, single line, terminated by `\n`. `event.wait` may remain open
  until an authorized event or filter deadline is due; its caller keeps the
  write side open so disconnect can cancel the subscription.
- Once the daemon accepts and begins a mutation, client disconnect or a lost
  response does not cancel it. The daemon completes the observable operation;
  the client re-queries status. Deployment mutations remain mutually exclusive
  and conflicting actions return `busy` while the original operation runs.
- Limits: request ≤ 65536 bytes, response ≤ 262144 bytes. Server request-read
  timeout is 5 s and response-write timeout is 10 s. Ordinary clients use a
  10-second response deadline; `event.wait` has no client response deadline
  beyond its per-filter deadlines and is still one bounded response, never a
  stream, session, or pipeline.

## Request

```json
{
  "protocol": 2,
  "id": "<client-generated opaque string, echoed back>",
  "operation": "test.start",
  "params": { },
  "client": {
    "kind": "codex",
    "session": "<optional task id>",
    "identity": "<edge-only signed-in e-mail>"
  }
}
```

- `client` is descriptive attribution only (`codex`, `claude`, `cursor`,
  `antigravity`, `human`, `other`, `edge`); only the edge identity assertion
  has authorization meaning, after peer-uid verification.
- Unknown envelope or operation-parameter fields are rejected. Protocol 1 is
  rejected with `protocol_unsupported` and is never translated or executed.

## Response

Success: `{"protocol": 2, "id": "…", "ok": true, "data": { }}`

Failure:

```json
{
  "protocol": 2, "id": "…", "ok": false,
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
| `protocol_invalid` | malformed JSON or invalid/missing envelope fields |
| `protocol_unsupported` | protocol is absent or is not version 2 |
| `request_too_large` | request frame over 64 KiB |
| `operation_unknown` | operation is not in the exhaustive registry |
| `params_invalid` | parameters fail the generated operation schema |
| `cursor_stale` | event/log cursor is older than retained history or ahead of its journal |
| `busy` | bounded wait or mutation admission is full |
| `repository_not_found` | path is not inside a registered/registerable Git repository |
| `repository_config_invalid` | `.devcoordinator.toml` fails validation |
| `repository_archived` | repository is historical; response names its active replacement when present |
| `repository_archive_blocked` | open work, live resources, or invalid replacement prevents archival |
| `worktree_busy` | per-worktree start lock not acquired within the bounded wait |
| `test_not_found` | no current test run for the worktree |
| `test_evidence_expired` | the exact retained visual run is unavailable under current retention |
| `test_evidence_not_found` | the requested screenshot identity is absent from that run |
| `test_evidence_tampered` | the screenshot no longer matches its recorded size, dimensions, or SHA-256 |
| `test_artifact_expired` | the exact retained artifact run is unavailable under current retention |
| `test_artifact_not_found` | the requested check, tree, or file is absent from that run |
| `test_artifact_tampered` | the retained manifest or file no longer matches its recorded identity or SHA-256 |
| `test_start_failed` | terminal launch failure (never `queued`) |
| `tests_draining` | a normal Coordinator upgrade has closed test admission; never queued |
| `unit_stop_failed` | prior unit would not stop / cgroup not proven empty |
| `task_not_found` | no ledger task with that id |
| `release_not_found` | no release with that id |
| `decision_not_found` | no decision with that id or ref |
| `internal_error` | unexpected daemon fault (bounded diagnostic in `detail`) |

## Result conventions

- Results are typed operation data. Private host paths and unrequested raw
  evidence are omitted; bounded log, screenshot, and artifact chunks require
  their dedicated exact-reference operations.
- Mutation success means the requested observable state was reached (e.g.
  `test.start` returns only after the unit's process exists), not merely
  that a handler ran or a row was saved.
- Timestamps are UTC ISO-8601 with seconds. Sizes are byte integers.
- IDs are opaque strings with stable one-letter prefixes
  (`r` repository, `w` worktree, `t` test run, `d` deployment,
  `p` plan task, `v` release, `n` decision — see `database-ledger.md`).
- `event.wait` returns only typed, bounded, authorization-filtered event
  metadata or heartbeat-due entries. It never returns raw logs, secrets,
  private paths, agent/task orchestration state, or an instruction to act.
