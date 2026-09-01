# Command Contract

Envelope and error model: `protocol.md`. Full schemas below exist for the
commands implemented in this delivery (`test.*`, `repository.*`, `ping`).
Later command families are sketched to reserve names and result shapes; they
have no implementation and must not be exposed before their end-to-end
behavior exists.

The CLI maps `devcoordinator2 test start …` to command `test.start`; the MCP
server exposes the same commands as tools `test_start`, `test_retry`,
`test_status`, `test_output`, `test_stop`, `test_capacity_get`,
`test_capacity_set`, `repository_list`. All three
surfaces return the identical result JSON.

## ping

Args: none.
Result: `{"daemon_version": "<semver>", "schema_version": 13, "socket": "<path>"}`

## test.start

Args:
- `path` (string, required) — any path inside the target worktree.
- `test` (string, optional) — named test from `.devcoordinator.toml`;
  defaults to the file's declared default.
- `checks` (array of unique check names, optional) — diagnostic selection plus
  its transitive prerequisites. Omit for the complete graph.
- `tier` (`development|pre-merge|release`, optional; default `release`) — the
  cumulative validation tier to run.

Omitting `checks` produces proof `complete`; supplying it produces proof
`selected` and always sets `readiness_eligible` false.

Result (only after the process exists):

```json
{
  "run_id": "t20260822T120000Z-1a2b3c",
  "repository_id": "r…", "worktree_id": "w…",
  "test": "unit",
  "requested_tier": "release",
  "readiness_eligible": true,
  "status": "running",
  "proof": "complete",
  "selection": [],
  "origin_run_id": null,
  "unit": "devcoordinator2-test-<worktree_id>-<suffix>.service",
  "summary_path": "<worktree>/.devcoordinator/test/current/summary.json"
}
```

Errors: `repository_not_found`, `repository_config_invalid`,
`worktree_busy`, `tests_draining`, `unit_stop_failed`, `test_start_failed`.
Never `queued`.
Latest-start-wins: a concurrent earlier run ends `superseded`.

Every dependency-ready leaf enters host-wide adaptive admission immediately.
`after` requires terminal completion; `requires` requires success. Failed
preflights mark their declared targets `invalidated`; unrelated branches
continue. Process exit or an inherited exact completion event advances the
graph. A leaf `timeout_seconds` is a failure ceiling and produces `timed_out`;
the test-level value is the outer watchdog. Neither can produce success.

## test.retry

Args: `path`, optional `test`, `run_id` of an original completed full run, and
one failed `check`. Result has the same running shape as `test.start`, with
`proof: "retry"`, `selection: [check]`, and `origin_run_id` set. The
target's prerequisite closure runs; a matching process-completed prerequisite
with declared artifact receipts may be `reused`. Missing, unfinished,
diagnostic, non-failed, source/config-changed, or artifact-stale origins fail
before the current slot is changed. A retry never constitutes complete proof.

## test.status

Args: `path` (required).
Result: the current `summary.json` fields (see below) plus `summary_path`.
Successful runs carry **no log text**. Error: `test_not_found`.

Graph status additionally carries `requested_tier`, `readiness_eligible`, `proof`
(`complete|selected|retry`), `selection`,
`origin_run_id`, `check_summary`, ordered `checks` (state, monotonic duration,
exit code, bounded reason, declared artifact receipts, output reference), a
bounded `failure_index`, `source_changed`, `unsafe_reason`, and
`check_report_path`. The referenced Rust check report is schema 2 only; schema
1/Python reports are rejected rather than translated. It includes
`capacity {learned_capacity, effective_capacity, capacity_wait_count}` plus
bounded preflight/check/case counts. States are `pending|running|passed|failed|reused|`
`timed_out|invalidated|not_meaningful|cancelled|unsafe`. Capacity-wait counts,
expanded-case totals, and preflight totals remain bounded summary fields.
Measurements never control success progression.

summary.json fields (result schema 2; schema 1 is not read or translated):
`schema_version`, `run_id`, `test`, `requested_tier`, `readiness_eligible`,
`proof`, `selection`, `origin_run_id`,
`status` (`running|passed|failed|timed-out|cancelled|interrupted|superseded`),
`started_at`, `finished_at` (null while running), `duration_seconds`,
`exit_code` (null unless exited), `stdout_bytes_observed`,
`stdout_bytes_retained`, `stderr_bytes_observed`, `stderr_bytes_retained`,
`stdout_truncated`, `stderr_truncated`, `caller_uid`, `client`.

## test.capacity.get | test.capacity.set

- `test.capacity.get` takes no arguments.
- `test.capacity.set` takes `{cap: integer|null}`. An integer sets the
  administrator maximum; `null` restores uncapped Auto admission. Lowering a
  cap never kills active work.

Both return `{learned_capacity, effective_capacity, cap, active, waiting,
paused, last_adjustment}`. `last_adjustment` is null or `{event_id, at, actor,
reason, previous_capacity, new_capacity, cap, p95_cpu_percent,
p95_memory_percent, saturation_fraction, epoch_seconds}`. Evidence values may
be null when unavailable. CLI equivalents are `test capacity show|set|clear`.

## test.output

Args:
- `path` (required)
- `stream` — `"stdout"` or `"stderr"` (required)
- `tail_bytes` — 1..65536, default 16384
- `check` — optional governed check name; omitted returns the aggregate stream

Result: `{"run_id", "stream", "check", "tail": "<utf-8, lossy-decoded>",
"tail_bytes": n, "truncated_before_tail": bool, "log_path": "…"}`

## test.stop

Args: `path` (required), optional one-line `reason` (3..256 characters).
Result: `{"run_id", "status": "cancelled"}` after the cgroup is proven
empty, or `{"run_id", "status": "<terminal>", "already_finished": true}`.
Errors: `test_not_found`, `unit_stop_failed`.
The reason is stored as `termination_reason` operational metadata and never
changes cancellation into a test failure.

## repository.register

Args: `path` (required). Registration also happens implicitly on first
`test.start`.
Result: `{"repository_id", "worktree_id", "root_path", "worktree_path",
"display_name", "registered": true|false}` (`false` = already known).

## repository.list

Args: optional `include_archived` (boolean, default false).
Result: `{"repositories": [{"repository_id", "root_path", "display_name",
"registered_at", "last_seen_at", "archived_at", "archive_note",
"merged_into_repository_id", "worktrees": [{"worktree_id",
"worktree_path"}]}]}`. Normal collections exclude archived repositories.

## repository.archive / repository.unarchive

- `repository.archive {repository_id, merged_into_repository_id, note}` retires
  an inactive repository while preserving its permanent task and decision
  history. It refuses open tasks, elaboration requests, open releases, active
  deployments, or an active test.
- `repository.unarchive {repository_id, note}` restores an archived repository
  only while its original checkout exists.

Both transitions append `repository_events`. New mutations against an archived
repository return `repository_archived` and name its replacement; explicit
historical plan, task-history, and decision reads remain available.

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
  Native Compose applies validate every declared service. Finite services are
  recreated for a changed candidate, must exit 0, and produce bounded
  generation receipts; long-running services must be running/healthy. An
  instance-authorized ignored `env_file` is passed by validated path only.
- `deployment.status` → `{deployment_id, name, source, repository_id, state
  (running|stopped|degraded|applying|failed), current_generation,
  previous_generation, domain, route_port, ttl_expires_at, components:
  [{name, type, state, health, generation, binding{kind, identity}, port,
  restarts, owned, independent_control, last_error, services?: [{name, role
  (`finite|running`), state, desired_state, containers, independent}],
  completed_services?: [{service, generation, container_id, image_id,
  exit_code, started_at, finished_at, recorded_at}]}], log_dir}`.
- `deployment.start | stop | restart {component?}` → status. Whole
  deployment in declared/reverse order, or one component whose declaration
  permits independent control. A declared independent Compose service is
  addressed as `<component>/<service>`; only exact containers carrying the
  recorded project+service identity are controlled, dependencies and finite
  setup are not started, and unrelated services/routes remain. Stop withdraws
  the route for a whole routed component, never for an unrelated Compose
  worker, and never deletes data.
- `deployment.logs {component, tail_lines<=5000}` → `{component, tail,
  log_path | container_id}`.
- `deployment.rollback` → status with `rolled_back_from`/`rolled_back_to`;
  checkout source only (`rollback_unavailable` otherwise).
- `deployment.remove {delete_data=false}` → `{removed, data_deleted,
  deleted_volumes}`. Stops everything, removes containers/units/generation
  checkouts and records; named volumes and PostgreSQL data survive unless
  `delete_data` is true. Repository `persistent_paths` are never touched.
- `deployment.set_domain {deployment_id, domain|null, port?, component?,
  public?}` (administrator) → `{deployment_id, domain, ...}`. Sets, changes,
  or clears the routed domain and republishes the route document. Managed:
  the label persists as an override that wins over the declared domain on
  every apply until cleared (clearing falls back to the declared domain);
  `port`/`component` are rejected. The route target is the declared
  `route = true` component or, implicitly, the single port-leasing
  process/docker component; the change is atomic — a refusal persists
  nothing. Observed: edits the observed route; when
  none exists yet, `port` is required (and `component` if several containers
  exist); a current-state re-import replaces such edits. Uniqueness is
  enforced across managed and observed routes.
- Imported current deployments return `observed_only=true`. Status, list,
  start/stop/restart, and logs work — lifecycle acts on the exact recorded
  container IDs and never recreates anything
  (DC2-2026-08-24-OBSERVED-LIFECYCLE); a missing container is reported, not
  replaced. apply/rollback/remove return `observed_only`; reviewed
  repository configuration is the explicit transition to configuration
  authority.

## health.containers (Phase 3, implemented)

Args: none. Result: `{containers: [{id, name, image, state, status, created,
repository_id, deployment_id, component, run_id, caller_uid, client,
ttl_seconds, data, classification}], counts}` where classification is one
of `managed-test`, `managed-preview`, `managed-permanent`,
`observed-current`, `orphaned-managed`, `unmanaged`. Classification uses
daemon-owned labels or an exact current observed-import identity
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
- `health.history {subject_kind, subject_id, metric, minutes<=43200,
  points?}` → one-minute `{minute, min, avg, max, samples}` points, at most
  1440 per call (`truncated` flag), from the 30-day bounded store. With
  `points` (2..1440) the daemon downsamples server-side into that many
  buckets preserving the min/max envelope and sample-weighted averages —
  the Console's 24h/7d/30d charts use this. Host storage
  (`host/host/storage_bytes`) is persisted every storage tick.
- `health.summary.unhealthy_deployments[*]` carries `reasons:
  [{component, state, detail}]` naming exactly which component is unhealthy
  and why (recorded error or failing container healthcheck).

Alerts (in `health.summary.alerts` and as `alert.opened`/`alert.recovered`
events): host CPU > 90% for 5 min, host memory available < 10% for 5 min,
root filesystem < 10% free, component not running for 2 min while desired
running, ≥ 3 restarts in 10 min (crash loop), test scratch > 10 GiB.
Deduplicated while active; one recovery each.

## usage.repositories | repository (implemented)

- `usage.repositories {range?}` returns the registered repositories visible to
  the caller with compact combined Codex totals. `range` is exactly `24h`,
  `7d`, or `30d` and defaults to `24h`. The collection opens each configured
  source once for all requested repositories, reads only row-level token/count/
  execution facts through indexed identities, and has an 800 ms server-side
  query budget so repository count cannot multiply an individual timeout.
- `usage.repository {repository_id, range?}` returns one content-free combined
  report: coverage/source counts, provider-native top-level token categories,
  fixed UTC phase buckets, activity totals, separate timing unions, tool
  outcomes/families, and measurement semantics.
- Administrators may read every repository. Public operators may read only a
  repository where they hold operator-or-higher deployment access; viewers are
  denied. Results never include collector identity, paths, per-user values,
  raw Codex entity IDs, prompts, model output, source, commands, or payloads.
- `total_tokens` alone is the stacked/activity basis. Cached input and
  reasoning remain labelled subsets; missing collectors and unsupported source
  versions are partial/unavailable coverage, never synthetic zeroes.

## progress.repositories | repository (implemented)

- `progress.repositories {}` returns compact operator-visible repository rows:
  `{repository_id, display_name, open_tasks, tasks_done, planned_lines_done,
  planned_lines_total, next_release}`. Public scope is injected by the access
  guard; callers cannot pass `_repository_ids` themselves.
- `progress.repository {repository_id, period?}` accepts `period` exactly
  `hour`, `day`, or `week` (default `day`). Hour uses 24 one-hour buckets, day
  uses seven one-day buckets, and week uses eight Monday-aligned seven-day
  buckets; each response also compares the immediately preceding matching
  period.
- The result carries `series` buckets with task completions/arrivals/reopens,
  current planned lines completed and scope movement, terminal test counts and
  pass rate, provider `total_tokens`, and per-bucket token coverage. It also
  carries current/previous totals, current scope, source-specific coverage,
  a deterministic release forecast, open release work in depth-first Plan
  order, and explicit counting semantics. Each open-work row exposes only the
  owner-facing title and recorded status, estimate, elaboration request,
  unblock condition, and latest reopening note. The compact Console summary
  deliberately renders only title, status, estimate, elaboration, and a simple
  reopened state; full raw planning/event prose remains outside that surface.
- “Planned lines completed” means the current estimates attached to tasks whose
  permanent status event reached `done`; it is not measured Git churn. Test
  history is bounded repository-local terminal metadata. Missing histories,
  unestimated tasks, no release, zero pace, and missing target dates remain
  explicit. The response does not infer priority, dependency, impact days, or
  an ordering scenario from those facts.
- Administrators may read every repository. Public operators may read only a
  repository where they hold operator-or-higher deployment access; viewers are
  denied because the result includes combined private token analytics.

## Public identities and roles (Phase 5, implemented)

A request carries `client.identity` only when the configured edge uid sends
it; any other peer is refused (`permission_denied`). Local callers stay
unrestricted. For identities: administrators may do everything;
`operator` may start/stop/restart granted deployments; `viewer` may read
status/logs/health of granted deployments; `access` only uses the deployed
application. `deployment.list` and `health.repositories` are filtered to
granted deployments; `health.summary`, `health.containers`, `test.*`,
`repository.*`, apply/rollback/remove, and user administration require an
administrator. Public callers address deployments by `deployment_id`.

- `user.whoami` → `{local, identity, user_id, administrator, grants}`.
- `user.list` → `{users: [{email, administrator, grants, …}], invitations,
  roles, owners}`.
- `user.invite {email, administrator?, grants?: [{deployment_id, role}]}`
  → `{invitation_id, email, expires_at}` (14 days; one exact identity).
- `user.accept_invitation {email, subject?, display_name?}` — edge only,
  for the signed-in identity itself → `{accepted, administrator, …}`.
- `user.remove {email}` → removes the user (and grants) or the invitation.
- `grant.set {email, deployment_id, role}` / `grant.remove {email,
  deployment_id}`.

Every user/grant change republishes the route document (owners + grants),
so edge enforcement changes on the next request.

## telegram.* and bug.* (Phase 6, implemented)

One server-owned bot (token in a private instance file, never in the
database or results). A Telegram user sends `/start` and receives a link
code; `telegram.link {code, email}` binds the chat to an identity (self or
administrator). `telegram.subscribe {chat_id, scope}` with scope `server`
(administrators), `deployment:<id>` (viewer or better), or
`repository:<id>` (a viewable deployment in it); `telegram.unsubscribe`;
`telegram.list` (own chats for public users; all for administrators).
Events: deployment apply/failure/rollback/start/stop/restart/remove,
component failure, preview expiry, test failure/timeout/supersession/
interruption (successful tests are silent), alert open/recover, new
unmanaged/orphaned container, daemon start, bug open/close, user and grant
changes. Delivery uses a bounded durable outbox (≤ 1000 rows, ≤ 10
attempts with backoff, ≤ 24 h) — reliability, never a work queue.

`bug.report {component, summary, expected, actual, steps, correlations?}`,
`bug.list`, `bug.close {bug_id}` — the daemon path only adds event emission;
the CLI and MCP write the same independent store directly so intake works
while the daemon, database, or edge is unavailable, then notify the daemon
best-effort (`notified: false` when it was down). Records are bounded atomic
text without secrets, raw logs, or private host paths; a recurrence
increments `occurrences` instead of duplicating; closing removes the record.

## plan.* / task.* / release.* / decision.* (Schemas 8, 11, and 12, implemented)

DC2-owned planning, completion ledger, and decision history
(DC2-2026-08-24-PLANNING-LEDGER). Append-only: every task/release mutation
appends `plan_events` rows in the same transaction; nothing is ever deleted.
A daemon or database error from these commands blocks the affected
completion claim — there is no file fallback. Repository reference on
repo-scoped commands: `path` (agents; implicit registration like
`test.start`) or `repository_id` (Console). Plain-language fields (`title`,
`outcome`, decision `body`, release `name`) are structurally bounded and
written for a non-technical reader; `technical_note` is the separate
agent-facing field and never substitutes for them (the daemon cannot detect
jargon — the register rule lives in the agent instructions).

- `plan.overview {}` (no repo) → `{repositories: [{repository_id,
  display_name, open_tasks, loc_done, loc_total, current_release: {name,
  kind, status} | null, preview_requested, elaboration_request_count}]}` — the plan picker; for public
  identities filtered to repositories with a viewable deployment.
- `plan.overview {path | repository_id}` → `{repository_id, display_name,
  archived, merged_into_repository_id,
  releases: [{release_id, name, kind (preview|release), status
  (planned|requested|delivered|dropped), seq, note, requested_at,
  delivered_at, url, port, tasks_total, tasks_done, loc_total, loc_done}],
  tasks: [{task_id, parent_task_id, release_id, seq, position, title,
  impact (bounded excerpt), status, kind, estimated_loc,
  elaboration_needed}], tasks_truncated, elaboration_requests: [{task_id,
  title, outcome (bounded excerpt), status, kind, requested_at}],
  preview_requested: [{release_id, name, requested_at, note}], decisions:
  {unsummarized_count, summary_due}}`. One bounded call for the Console
  Gantt and agents; dropped tasks/releases are excluded (full row via
  `task.history`); aggregates count leaf tasks (a parent is a summary row);
  when the cap (500) cuts, every unfinished task is kept and
  `tasks_truncated` is true. `elaboration_requests` is independent of that
  cap, so a completed task's request cannot disappear with older chart rows.
- `task.create {path|repository_id, title, kind
  (goal|stub|improvement|user_feedback), outcome?, parent_task_id?,
  release_id?, estimated_loc?, impact?, unblock_condition?, verification?,
  technical_note?}` → `{task_id, repository_id, seq, position, status:
  "planned", release_id, elaboration_needed: false, preview_requested,
  elaboration_requests}`. `outcome` defaults to the
  title. The target release must still be open. Appends `created`.
- `task.update {task_id, title?, outcome?, impact?, unblock_condition?,
  verification?, technical_note?, estimated_loc?, status?, release_id?
  (null = backlog), parent_task_id? (null = root), position? (0-based order
  among siblings), elaboration_needed?, note?}` → compact task projection +
  `preview_requested` + repository `elaboration_requests`.
  Folds edits (`edited` event naming the fields), status changes (`status`;
  reopen is `done→in_progress`), estimate changes (`estimate`), release
  moves (`release_move`), reparenting (`reparent`; cycles rejected), and
  reordering (`reorder`; the sibling group is renumbered transactionally).
  `elaboration_needed: true` appends `elaboration_requested`.
  `elaboration_needed: false` is accepted only when the same update changes
  `title` or `outcome`, and appends `elaboration_completed` atomically with
  the `edited` event.
  `note` is stored on each appended event. Errors: `task_not_found`,
  `release_not_found`, `args_invalid` ("nothing to change" when a no-op).
- `task.history {task_id}` → `{task: <full row>, events: [{event, from, to,
  actor, at, note}], events_truncated, elaboration_requests}` — the bounded permanent history
  (last 200), loaded on concrete need.
- `release.create {path|repository_id, name, kind (preview|release), note?,
  seq?}` → `{release_id, repository_id, seq, name, kind, status:
  "planned"}`; an explicit `seq` must be free.
- `release.update {release_id, name?, seq?, note?, status?}` (administrator/
  CLI recovery) → compact release row. `status` may only move
  planned↔dropped; requesting and delivering are their own commands.
- `release.request {path|repository_id, name?, note?}` — the owner's ASAP
  button (Console-first; deliberately not an MCP tool): creates a
  `kind=preview, status=requested` release (default name "Preview
  (requested YYYY-MM-DD)"); a second pending request is refused. Agents see
  it in `plan.overview.preview_requested` and as `preview_requested` on
  every task mutation result.
- `release.deliver {release_id, deployment_id, note?}` → `{release_id,
  status: "delivered", delivered_at, url, port, commit_hash, dirty,
  generation_number}`. The deployment must belong to the same repository
  and have a current generation. Snapshots permanent reachability evidence
  (routed-domain URL when one exists, leased host port either way) plus
  commit/dirty/fingerprint — generations are pruned, the snapshot is not.
  A delivered release is immutable evidence; the next preview is a new
  release. Emits `release.delivered` (Telegram: owner gets the URL/port).
- `decision.record {path|repository_id, aspect (ui|architecture|algorithms|
  business_logic|data|testing|deployment|security|performance|process|
  other), title, body, technical_note?, ref?, supersedes?}` →
  `{decision_id, seq, ref, unsummarized_count, summary_due}`. `title`+`body`
  are the management-facing account; `ref` is a stable citation key (unique
  per repo); `supersedes` (id or ref) sets the old row's forward pointer —
  the record itself is never edited or deleted.
- `decision.tail {path|repository_id, aspect?, n? (1..50, default 10),
  before_seq?}` → `{repository_id, display_name, summary: {body,
  covers_through_seq, created_at} | null, decisions: [...], has_more,
  unsummarized_count, summary_due}`. The normal context load: rolling
  summary + last N; `before_seq` pages older windows (decisions with a
  lower sequence).
- `decision.search {path|repository_id, query, aspect?, n?}` →
  `{repository_id, query, decisions: [...], has_more}` — FTS5 full-text
  search over every decision (title, body, technical note, ref), bm25
  ranked; user text is quoted so FTS operators are literal.
- `decision.summarize {path|repository_id, body, covers_through_seq}` →
  `{repository_id, covers_through_seq, unsummarized_count, summary_due}`.
  Stores the agent-written rolling summary; all summaries are kept. When
  `unsummarized_count` reaches 25, every decision read reports
  `summary_due: true` and the working agent writes the next summary — the
  daemon never generates text. This authorized append-only maintenance acts
  directly and does not require a separate user confirmation.

Every repository-scoped task, release, and decision result also includes
`elaboration_requests`. Agents treat a non-empty list as owner input: load
each task, rewrite its title and/or outcome in everyday language, and clear
the flag in the same update. No automatic text generator or stand-in success
is implied.

Access: reads (`plan.overview`, `task.history`, `decision.tail`,
`decision.search`) require viewer on a deployment of the repository (public
callers reference by `repository_id`; a `path` would implicitly register
and is refused); all mutations require administrator; local socket callers
are unrestricted. New error codes: `task_not_found`, `release_not_found`,
`decision_not_found`.

MCP tools: `plan_overview`, `task_create`, `task_update`, `task_history`,
`release_create`, `release_deliver`, `decision_record`, `decision_tail`,
`decision_search`, `decision_summarize`. `release.request` and
`release.update` are owner controls (Console + CLI only). CLI:
`devcoordinator2 plan overview [--all]`, `task create|update|history`,
`release create|update|request|deliver`, `decision
record|tail|search|summarize`.

All follow the same envelope, error model, and file-reference conventions.
