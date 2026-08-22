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
- **REQ-TEST-09** (P2): A test-scoped ephemeral PostgreSQL and test
  containers carry the exact test identity and are removed on completion,
  timeout, supersession, or the next start. Declared shared/permanent
  database dependencies are never stopped and their data never deleted by
  test cleanup.

## Deployments (REQ-DEPLOY, P3)

- **REQ-DEPLOY-01**: One permanent deployment containing HTTP, worker,
  Docker, and PostgreSQL components can apply, start, stop, restart,
  update, and roll back (current + immediately previous generation only).
- **REQ-DEPLOY-02**: Concurrent mutation of the same deployment returns
  `busy`; it is never queued.
- **REQ-DEPLOY-03**: Partial failure yields an honest `degraded` state
  listing exact running and failed components; no fake success.
- **REQ-DEPLOY-04**: Stop/restart/redeploy never deletes persistent
  database or volume data; destructive removal is a separate explicit
  action.
- **REQ-DEPLOY-05**: Port and domain assignments are transactionally unique
  and route only to healthy selected generations via atomic route-document
  publication.

## Accountability and health (REQ-HEALTH, P3/P4)

- **REQ-HEALTH-01**: Every managed process/container reports repository,
  deployment/test, component, physical caller UID, and descriptive creating
  client via daemon-owned labels/records that callers cannot override.
- **REQ-HEALTH-02**: Every unlabeled Docker container appears as
  `unmanaged/unknown` within one observation interval; ownership is never
  inferred from names, ports, image tags, or paths.
- **REQ-HEALTH-03**: Repository CPU/memory/storage aggregates reconcile:
  managed repositories + DevCoordinator + shared/unattributed = host total,
  without double-counting or invented per-database CPU/memory.
- **REQ-HEALTH-04**: Metrics: 15 s CPU/memory sampling, 1-minute persisted
  aggregates, 5-minute storage sampling, 30-day retention, direct deletion
  of expired rows.
- **REQ-HEALTH-05**: Automatic cleanup is limited to exact DevCoordinator-
  owned ephemeral work; unmanaged containers and persistent volumes are
  surfaced for explicit decisions; Docker prune is never a lifecycle
  operation.

## Public access (REQ-ACCESS, P5)

- **REQ-ACCESS-01**: An invited user authenticates at the edge and accesses
  only granted deployments; grants bind to immutable deployment IDs.
- **REQ-ACCESS-02**: Roles are exactly access / viewer / operator /
  administrator; no per-action permission matrices.
- **REQ-ACCESS-03**: Revocation takes effect on the next edge/API request.
- **REQ-ACCESS-04**: Local agent calls via the Unix socket are independent
  of public deployment grants; the kernel peer UID is the caller identity
  and request bodies cannot assert identity (P1, in scope).

## Reliability (REQ-REL)

- **REQ-REL-01** (P1, in scope): `devcoordinatord` and the stable edge are
  the only persistent control-plane services; periodic-task failures are
  isolated and never block test or deployment mutations.
- **REQ-REL-02** (P5): Edge routes remain available across daemon restarts
  from the last valid atomic route document.
- **REQ-REL-03** (P6): Bug intake works while the daemon, database, or edge
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
