# Requirements and Acceptance Criteria

Derived from the product handover (local document, untracked). Tags are
stable and cited by tests and the acceptance checklist. Phases refer to the
development sequence (0–8). This delivery implements Phase 0 + Phase 1;
requirements owned by later phases are recorded here so the foundation does
not contradict them.

## Tests (REQ-TEST)

- **REQ-TEST-01** (P1/P2, in scope): A test starts immediately or fails with
  a terminal error. No `queued` state exists in code, schema, docs, or UI.
- **REQ-TEST-02** (P1/P2, in scope): One current test slot per Git worktree;
  a newer start supersedes the older run (latest-start-wins), kills the
  previous exact test workload (whole cgroup, TERM then KILL), proves the
  prior cgroup is empty, and removes exactly the prior repository-local test
  directory before launch.
- **REQ-TEST-03** (P1, in scope): Repository commands run as the physical
  non-root caller (all four UIDs), never as root or the daemon identity.
- **REQ-TEST-04** (P1, in scope): Timeout (`RuntimeMaxSec`) and cancellation
  kill every process in the unit cgroup; cleanup is proven, not assumed.
- **REQ-TEST-05** (P1, in scope): Successful status responses contain no log
  text. Diagnostics are explicitly requested as a bounded stdout/stderr tail
  (≤ 64 KiB) or read from the exact result file paths returned.
- **REQ-TEST-06** (P1, in scope): `summary.json` is atomically replaced and
  contains: schema version, run ID, named test, status (running | passed |
  failed | timed-out | cancelled | interrupted | superseded), start/finish/
  duration, exit code, observed and retained stdout/stderr byte counts,
  explicit truncation flags, plus caller UID and descriptive client
  (deliberate extension, see DecisionHistory).
- **REQ-TEST-07** (P1, in scope): Output capture retains at most 4 MiB per
  stream while continuing to drain and count, so a noisy process can never
  block or fill the server.
- **REQ-TEST-08** (P1, in scope): On daemon restart, a complete atomic
  summary is imported as-is; unfinished test units are stopped and marked
  `interrupted`. Nothing is resurrected, migrated, or retried.
- **REQ-TEST-09** (P2, done): A test-scoped ephemeral PostgreSQL and test
  containers carry the exact test identity and are removed on completion,
  timeout, supersession, or the next start. Declared shared/permanent
  database dependencies are never stopped and their data never deleted by
  test cleanup.

## Deployments (REQ-DEPLOY, P3)

- **REQ-DEPLOY-01** (P3, done): One permanent deployment containing HTTP, worker,
  Docker, and PostgreSQL components can apply, start, stop, restart,
  update, and roll back (current + immediately previous generation only).
- **REQ-DEPLOY-02** (P3, done): Concurrent mutation of the same deployment returns
  `busy`; it is never queued.
- **REQ-DEPLOY-03** (P3, done): Partial failure yields an honest `degraded` state
  listing exact running and failed components; no fake success.
- **REQ-DEPLOY-04** (P3, done): Stop/restart/redeploy never deletes persistent
  database or volume data; destructive removal is a separate explicit
  action.
- **REQ-DEPLOY-05** (P3, done): Port and domain assignments are transactionally unique
  and route only to healthy selected generations via atomic route-document
  publication.
- **REQ-DEPLOY-06** (P8, done; amended 2026-08-24): A reviewed migration may
  import exact currently running native identities as a replaceable
  observed-only projection. It exposes attribution, status, health, and
  verified routes; stopped, missing, temporary, validation, test, conflicting,
  and historical records are excluded. Per DC2-2026-08-24-OBSERVED-LIFECYCLE,
  start/stop/restart and bounded logs act on the exact recorded containers;
  configuration authority (apply, rollback, remove, recreation) still requires
  adoption through repository configuration.
- **REQ-DEPLOY-07** (2026-08-24, done): An administrator can set, change, or
  clear the routed domain of any deployment from the Console (pop-up on the
  list and detail pages), CLI, and MCP (`deployment.set_domain`). For managed
  deployments the override survives re-apply until cleared; for observed
  deployments it lives in the observed projection and is replaced by the next
  current-state re-import. Domain uniqueness is enforced across managed and
  observed routes; the change is atomic. A deployment with exactly one
  port-leasing process/docker component routes implicitly without
  `route = true` (DC2-2026-08-24-IMPLICIT-ROUTE); deployment listings and
  status name their repository so instances are always attributable.

## Accountability and health (REQ-HEALTH, P3/P4)

- **REQ-HEALTH-01** (P3/P4, done): Every managed process/container reports repository,
  deployment/test, component, physical caller UID, and descriptive creating
  client via daemon-owned labels/records that callers cannot override.
- **REQ-HEALTH-02** (P3, done): Every unlabeled Docker container appears as
  `unmanaged/unknown` within one observation interval; ownership is never
  inferred from names, ports, image tags, or paths.
- **REQ-HEALTH-03** (P4, done): Repository CPU/memory/storage aggregates reconcile:
  managed repositories + DevCoordinator + shared/unattributed = host total,
  without double-counting or invented per-database CPU/memory.
- **REQ-HEALTH-04** (P4, done): Metrics: 15 s CPU/memory sampling, 1-minute persisted
  aggregates, 5-minute storage sampling, 30-day retention, direct deletion
  of expired rows.
- **REQ-HEALTH-05** (P3, done): Automatic cleanup is limited to exact DevCoordinator-
  owned ephemeral work; unmanaged containers and persistent volumes are
  surfaced for explicit decisions; Docker prune is never a lifecycle
  operation.
- **REQ-HEALTH-06** (2026-08-24, done): The health surface answers "what exactly
  is unhealthy": every unhealthy deployment carries component-level reasons
  (state plus recorded error/healthcheck detail) and the Console offers the
  matching lifecycle actions next to them. Host CPU, memory, and storage
  expose 24h/7d/30d history (`health.history` with server-side downsampling
  via `points`; host storage is persisted every storage tick).

## Public access (REQ-ACCESS, P5)

- **REQ-ACCESS-01** (P5, done): An invited user authenticates at the edge and accesses
  only granted deployments; grants bind to immutable deployment IDs.
- **REQ-ACCESS-02** (P5, done): Roles are exactly access / viewer / operator /
  administrator; no per-action permission matrices.
- **REQ-ACCESS-03** (P5, done): Revocation takes effect on the next edge/API request.
- **REQ-ACCESS-04**: Local agent calls via the Unix socket are independent
  of public deployment grants; the kernel peer UID is the caller identity
  and request bodies cannot assert identity (P1, in scope).

## Planning, completion ledger, and decisions (REQ-PLAN, Schema 8)

- **REQ-PLAN-01** (S8, done): DevCoordinator owns the single authoritative
  completion ledger. Anything an agent stubs, fakes, skips, or finds
  improvable becomes a `tasks` row at once; a daemon or database error
  blocks the affected completion claim and never authorizes a file or
  chat-memory fallback.
- **REQ-PLAN-02** (S8, done): One task tree of arbitrary depth per
  repository (parents are summary rows), sized in estimated lines of code,
  with append-only permanent history: every mutation appends `plan_events`
  in the same transaction and no code path deletes planning rows.
- **REQ-PLAN-03** (S8, done): Plain language first — bounded required
  title/outcome (and decision title/body) written for a non-technical
  owner; `technical_note` is a separate agent-facing field the daemon never
  substitutes for the plain account.
- **REQ-PLAN-04** (S8, done): `plan.overview` is one bounded call carrying
  releases with leaf-based progress, the compact active task projection
  (unfinished tasks never truncated away), pending preview requests, and
  decision-summary state.
- **REQ-PLAN-05** (S8, done): The owner can move tasks between releases,
  reorder them within a release (mutable `position`, immutable `seq`), and
  drop them; each is an append-only transition (`release_move`, `reorder`,
  `status`).
- **REQ-PLAN-06** (S8, done): Delivering a release records a real
  deployment's evidence — commit, dirty flag, fingerprint, routed URL or
  leased host port — as a permanent snapshot that outlives generation
  pruning and deployment removal; a delivered preview is always reachable
  by URL or port.
- **REQ-PLAN-07** (S8, done): Decisions are per-repository, aspect-tagged,
  permanent, and support explicit supersession; agents load the rolling
  summary plus the last N; at 25 unsummarized decisions every decision read
  reports `summary_due` and the working agent stores the next summary — the
  daemon never generates text; all summaries are kept.
- **REQ-PLAN-08** (S8, done): Owner feedback from previews enters the same
  ledger as `user_feedback` tasks; the independent bug store remains
  coordinator-defect intake only.
- **REQ-PLAN-09** (S8, done): Every decision ever recorded is full-text
  searchable (SQLite FTS5 over title, body, technical note, ref) by both
  the owner and agents; FTS5 absence is a refused start, not a silent
  degrade.
- **REQ-PLAN-10** (S8): `DecisionHistory.md` is retired in favor of the
  database: `scripts/decision_import.py` imports the existing entries with
  their `DC2-…` refs preserved, after which the file is a pointer stub and
  new DC2-repository decisions are recorded only in the database
  (owner-executed import).

## Reliability (REQ-REL)

- **REQ-REL-01** (P1, in scope): `devcoordinatord` and the stable edge are
  the only persistent control-plane services; periodic-task failures are
  isolated and never block test or deployment mutations.
- **REQ-REL-02** (P5, done): Edge routes remain available across daemon restarts
  from the last valid atomic route document.
- **REQ-REL-03** (P6, done): Bug intake works while the daemon, database, or edge
  is unavailable (independent store, open records only).
- **REQ-REL-04** (P1, in scope): Durable state uses atomic file replacement
  and SQLite transactions; test/metric histories are disposable and their
  loss never affects deployments, routes, users, grants, or persistent
  data.
- **REQ-REL-05** (P1, in scope): All subprocesses are argv arrays; no
  caller-authored shell strings anywhere.
- **REQ-REL-06** (P1, in scope): One shared protocol and result schema
  across CLI, MCP, and Console; model-facing responses stay small and
  reference files for detail.

## Phase 1 acceptance checklist (executed in this delivery)

1. `test start` on a sample repo returns `running` only after the process
   exists; a broken command returns a terminal `test_start_failed`
   (REQ-TEST-01).
2. Two rapid starts on one worktree: the first run ends `superseded`,
   exactly one unit remains, the prior test directory is gone
   (REQ-TEST-02).
3. Child process runs with all four UIDs of the caller (REQ-TEST-03).
4. A sleeping test with a short timeout ends `timed-out` with an empty
   cgroup; `test stop` ends `cancelled` with an empty cgroup (REQ-TEST-04).
5. `test status` after success carries no log text; `test output` returns a
   bounded tail with truncation flags (REQ-TEST-05, REQ-TEST-07).
6. `summary.json` matches the schema and is replaced atomically under crash
   injection (REQ-TEST-06, REQ-REL-04).
7. Daemon killed mid-test and restarted: run is `interrupted`, unit
   stopped, never restarted (REQ-TEST-08).
8. Socket peer UID recorded on mutations; body-asserted identity ignored
   (REQ-ACCESS-04).
9. CLI and MCP return byte-identical result JSON for the same call
   (REQ-REL-06).
10. `scripts/check_no_instance_data.py` passes over all committed content
    (DC2-…-NO-INSTANCE-DATA).
