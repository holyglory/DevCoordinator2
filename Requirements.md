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
- **REQ-TEST-05** (P1, in scope; revised 2026-09-02): Successful status
  responses contain no log text. Terminal responses contain only bounded
  structured diagnostics and stable log references; raw stdout, stderr, stack
  traces, and arbitrary error prose require an explicit bounded log request.
- **REQ-TEST-06** (P1, in scope; revised 2026-09-02): `summary.json` is
  atomically replaced and contains: schema version, run ID, named test, status
  (running | passed | failed | timed-out | cancelled | interrupted |
  superseded), start/finish/duration, exit code, complete per-stream byte and
  line counts, content hashes and log references, structured diagnostics,
  caller UID, and descriptive client. Retained-byte and truncation fields are
  not part of the current contract because governed logs are complete.
- **REQ-TEST-07** (P1, superseded 2026-09-02): The former 4 MiB per-stream
  storage cap is removed. REQ-TEST-19 and REQ-TEST-20 now bound storage by
  completed-history retention and bound model-facing retrieval separately.
- **REQ-TEST-08** (P1, in scope): On daemon restart, a complete atomic
  summary is imported as-is; unfinished test units are stopped and marked
  `interrupted`. Nothing is resurrected, migrated, or retried.
- **REQ-TEST-09** (P2, done): A test-scoped ephemeral PostgreSQL and test
  containers carry the exact test identity and are removed on completion,
  timeout, supersession, or the next start. Declared shared/permanent
  database dependencies are never stopped and their data never deleted by
  test cleanup.
- **REQ-TEST-10** (2026-08-28, done): An extension-dependent test may use a
  reviewed PostgreSQL-compatible image pinned by immutable SHA-256 digest.
  The root daemon alone pulls and verifies the exact digest; generated
  credentials, loopback publication, disposable storage, attribution, and
  exact cleanup remain identical to the official-image fixture.
- **REQ-TEST-11** (2026-09-02, done; supersedes
  DC2-2026-09-01-INDEPENDENT-CHECKS-NO-BUDGET): A repository test declaration
  uses schema 2 and a finite acyclic graph. Schema 1, legacy single-command
  declarations, translation, and fallback are rejected. Every dependency-ready
  leaf enters DevCoordinator's host-wide adaptive admission queue immediately;
  `after` waits for terminal completion and `requires` additionally requires
  success. Repositories do not encode host capacity as fake dependencies or
  run a second worker-budget system.
- **REQ-TEST-12** (2026-09-02, done; supersedes
  DC2-2026-09-01-DETERMINISTIC-CHECK-COMPLETION): Normal check progression comes only
  from the exact process exit or a dedicated inherited completion event bound
  to the run and check identity. Elapsed time is never success or readiness.
  A check or expanded case may declare `timeout_seconds` only as a failure
  ceiling; expiration records `timed_out`, terminates its complete process
  group, and never advances as success. The test-level systemd deadline remains
  the outer containment watchdog.
- **REQ-TEST-13** (2026-09-01, done): Live and terminal status includes a
  bounded, atomically replaced check report: proof kind, selection, exact
  state and monotonic duration per check, declared artifact receipts, and one
  ordered failure index. Ordinary failures do not stop independent checks or
  completion-only successors; success-dependent checks become explicitly not
  meaningful. Only an explicitly unsafe/stop failure cancels remaining work,
  and every path completes the existing cgroup/container cleanup contract.
- **REQ-TEST-14** (2026-09-01, done): Explicit check selections and a
  failed-check retry are diagnostic only. Retry begins only after an original
  complete run finishes, includes the target's prerequisite closure, reuses
  only exact matching regular-file artifact receipts, reruns non-reusable
  setup, and rejects changed source, config, artifacts, origin, or target
  state. Bounded evidence stores no source, commands, environment values,
  credentials, raw output, or caller identity. Only a fresh complete passing
  graph is release-quality proof (DC2-2026-09-01-DIAGNOSTIC-CHECK-EVIDENCE).
- **REQ-TEST-15** (2026-09-01, done): A normal DevCoordinator2 source restart
  requires the canonical checkout to be a clean `main` exactly matching the
  fetched `origin/main`, atomically closes test admission, waits on exact
  active-run receipt events until every current test and cleanup finishes, and
  only then restarts from that checkout. Abort restores admission and leaves
  the existing process running. A stale installer lease recovers. Explicit cancellation records a
  bounded operational reason; unexpected daemon restart retains REQ-TEST-08
  and never reconnects or resurrects work
  (DC2-2026-09-01-UPGRADE-TEST-DRAIN,
  DC2-2026-09-01-TRUSTED-LIVE-CHECKOUT).
- **REQ-TEST-16** (2026-09-02, done): Host-wide Auto capacity begins at
  twice the online logical CPU count and remains stable during one workload
  epoch. After an epoch containing a run of at least ten minutes, it increases
  25% when admission was saturated for at least half the samples and CPU and
  memory p95 were both below 90%; it decreases 25% when CPU or memory stayed at
  or above 98% for four 15-second samples spanning at least 45 seconds. Memory
  uses `MemAvailable`. Sustained pressure pauses new grants without killing
  active work until two samples put both measures below 95%. Missing evidence
  causes no adjustment. An administrator may set or clear one host-wide
  maximum; lowering it never kills active work. Learned/effective capacity,
  the cap, active/waiting counts, pause state, and append-only adjustment
  evidence are available through CLI, MCP, and Console.
- **REQ-TEST-17** (2026-09-02, done): Every schema-2 check declares its
  minimum validation tier: `development`, `pre-merge`, or `release`.
  Development runs development checks; pre-merge adds pre-merge checks; release
  runs all three. `test.start` defaults to release when the caller omits the
  tier. Selected checks and development/pre-merge runs are diagnostic; only a
  fresh complete passing release run is readiness evidence.
- **REQ-TEST-18** (2026-09-02, done): A check may be a preflight whose
  declared invalidation targets become real success dependencies. All safe
  independent preflights finish; a failed preflight prevents each target from
  launching and reports it as `invalidated`, while unrelated branches continue.
  One bounded discovery step may expand a reviewed command into at most 4,096
  cases from a 2 MiB inherited-descriptor JSON manifest. Cases may append only
  bounded IDs and arguments to that command, cannot replace cwd/environment or
  recursively expand, and each receives central admission, process ownership,
  deadline, logs, result, and cleanup.
- **REQ-TEST-19** (2026-09-02, done): The Rust execution plane drains and
  retains every byte of stdout and stderr for every governed check, discovery
  step, and expanded case, with no per-stream byte or line limit. Each leaf's
  streams live in its own stable run-relative folder and carry exact byte and
  line counts, first/last timestamps, and SHA-256 hashes. A storage write or
  sync failure makes the affected evidence unsafe rather than silently
  truncating it. Aggregate views may index or reference leaf streams but must
  not become a second lossy source of truth.
- **REQ-TEST-20** (2026-09-02, done): DevCoordinator tracks every governed
  log folder and regularly removes only inactive completed history that is
  older than either configured boundary: 24 hours or the newest three retained
  runs for the same repository/test/check/case identity by default. Both
  boundaries are administrator-configurable, persist across restart, and are
  visible through CLI, MCP, and Console. Cleanup is exact-target, path-contained,
  crash-safe, and never removes the active run or the newest protected history.
- **REQ-TEST-21** (2026-09-02, done): Normal completion exposes a bounded
  ordered diagnostic index containing failed check/case identity, exit status,
  timeout or cancellation category, structured source file and line when
  supplied, error category, structured expected and actual values, a stable
  duplicate-group fingerprint, and supporting log references. It contains no
  raw stdout/stderr, stack trace, or unclassified error prose. Rust ingests
  declared JUnit XML, Playwright JSON, Rust JSON records, and a bounded inherited
  DevCoordinator diagnostic-event channel; malformed or unavailable evidence
  is reported as a structured evidence condition rather than scraped from
  console text. Ranking is deterministic: explicit failure events, assertions,
  compiler diagnostics, panic/exception headers, first relevant stack frames,
  process termination, browser console/network failures, final non-empty lines;
  repeated entries collapse by fingerprint and count.
- **REQ-TEST-22** (2026-09-02, done): `test log catalog`, `tail`, `search`,
  `range`, and `failure-context` expose progressive log disclosure for one
  authorized run/check/case/stream. Catalogue results contain names, exact byte
  and line counts, time range, complete/truncation state, SHA-256, expiry, and
  structured-evidence availability without log content. Retrieval always has a
  strict response-byte ceiling and stable line/cursor coordinates: tail defaults
  to 50 lines; search is fixed-string by default with bounded matches and
  context; range reads an exact line or byte interval; failure-context applies
  the deterministic REQ-TEST-21 ranking. No command performs language-model
  summarization or returns an unbounded stream.
- **REQ-TEST-23** (2026-09-03, done): A formal Web UI check running under the
  governed executor receives one private run/leaf evidence directory and
  publishes `journey-evidence.json` there. A caller that keeps reports at
  explicit artifact paths retains those outputs and also publishes a unique,
  atomic, immutable bundle below `formal-runs/`; concurrent formal batches do
  not replace one another. The strict manifest orders every
  declared route/state/viewport cell, records only action kinds/outcomes and
  finding kinds, and binds its masked viewport/full-page PNGs by actual size,
  dimensions, and SHA-256. It contains no action values, selectors,
  credentials, entered values, placeholders, or absolute paths. The complete
  bundle is retained and pruned with that exact log leaf.
- **REQ-TEST-24** (2026-09-02, done): An authenticated Console administrator or
  trusted local caller can list one exact retained run's privacy-safe journey
  metadata and request one exact screenshot in bounded chunks. Every request
  is repository/worktree/run/check/leaf/image-bound, uses no-follow contained
  reads, validates PNG dimensions and the manifest SHA-256 before disclosure,
  returns no private filesystem path, and distinguishes absent/expired,
  unknown, and tampered evidence. The edge keeps no repository filesystem
  authority.
- **REQ-TEST-25** (2026-09-02, done): A Console annotation never modifies its
  evidence PNG. Creating one validates bounded normalized geometry and plain
  feedback text, atomically creates an ordinary repository `user_feedback`
  Plan task, and stores the screenshot anchor and discussion alongside that
  task. Replies, author edits, resolve/reopen, and explicit author deletion are
  persistent; resolving/reopening/dropping the annotation updates the linked
  Plan task, and the original task/event history is never erased.
- **REQ-TEST-26** (2026-09-03, done): A successful direct process check may
  declare up to eight required non-secret evidence directories, each with a
  caller-selected byte ceiling no greater than 1 GiB and no more than 4,096
  regular files; the combined declared ceiling is at most 2 GiB. The executor
  copies the trees without following links into that exact check leaf, records
  sorted per-file size/SHA-256 plus one domain-separated tree digest, and binds
  the manifest to run, test, check, source, configuration, proof kind, and
  requested tier. Empty, overlapping, reserved, changed, oversized, aliased,
  special-file, and fan-out declarations fail the check. Retention removes the
  copy atomically with its run. Trusted local callers and authenticated
  administrators can list a bounded manifest page and read one exact verified
  file chunk; the CLI can reconstruct selected trees only in a new caller-owned
  destination and revalidates every file and tree hash. No operation exposes a
  private source/storage path or writes a caller-selected server-side export.
- **REQ-TEST-27** (2026-09-03, done): Opening a retained test stream in the
  Console immediately renders its newest bounded text. The ordinary reading
  journey never asks for line or byte coordinates: people can load earlier
  output, return to the newest output, search literal text, or show likely
  failure context with plainly labelled controls. Technical stream metadata is
  available on request without preceding the output. Switching streams loads
  the selected stream automatically, active streams expose a refresh action,
  and progressive pages preserve reading position without repeating excerpts.
  The Console continues to compose the bounded REQ-TEST-22 operations; it does
  not request or render an unbounded stream.

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
- **REQ-DEPLOY-08** (2026-08-28, done): A native Compose component may use a
  reviewed file set, an instance-authorized ignored interpolation file, and
  explicit finite services. A changed apply reruns finite services and
  requires exit 0 with a bounded generation receipt; unchanged convergence
  and ordinary start never rerun them. Every declared long-running service
  must remain running/healthy.
- **REQ-DEPLOY-09** (2026-08-28, done): A Compose declaration may explicitly
  permit exact start/stop/restart of selected long-running services. Finite
  and undeclared services are refused; unrelated services, completion
  receipts, volumes, and routes remain untouched. Live service and aggregate
  state distinguish an intentional stop from failure, and CLI, MCP, and
  Console expose the same reviewed controls.
- **REQ-DEPLOY-10** (2026-09-03, done): Every repository section and every
  deployment on the Deployments dashboard starts expanded and can be collapsed
  independently. A collapsed repository retains its identity, deployment
  count, and overall condition; a collapsed deployment retains its identity
  and state while routing facts and lifecycle actions remain hidden until it
  is expanded. The choice survives same-session Console rerenders, controls are
  keyboard operable with truthful `aria-expanded`/`aria-controls`, and no API
  call occurs merely to change density. Repository Tests additionally show the
  selected real run's tier, elapsed time, output size, recency, and proof type;
  Health shows current CPU, memory, storage, and deployment count. Missing or
  restricted facts remain absent or explicitly unavailable and never borrow
  another repository's data.

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
- **REQ-HEALTH-07** (2026-08-29): The Console exposes combined, content-free
  Codex usage by registered repository for fixed 24h/7d/30d windows. Every
  explicitly configured same-owner collector is read in place and contributes
  only measured facts; missing, unmapped, or unsupported collectors produce
  a visible warning that some usage may be missing. The Console calls these
  inputs configured Codex environments. The visible status stays concise; an
  adjacent keyboard-accessible information hint explains that each environment
  is a separate local Codex setup with its own usage history and that environments
  which supplied no data and unmeasured values are excluded rather than counted
  as zero. The Console never exposes internal collector terminology. Administrators see every repository and operators
  see only repositories where they hold operator-or-higher deployment access;
  viewers and individual collector identities remain excluded.
- **REQ-HEALTH-08** (2026-09-01): `usage.repositories` reads each configured
  Codex environment once for all visible repositories, uses indexed compact
  facts rather than repeating detail-only scans per row, and settles the live
  repository table in under one second without copying usage records or
  persisting aggregates. Not-connected setup states are neutral; partial
  measured data is amber, complete data green, no measurements neutral, and
  red is reserved for an actual source/read failure.

## Repository progress and forecasting (REQ-PROGRESS)

- **REQ-PROGRESS-01** (2026-08-30, done): An operator or administrator can
  choose a repository and view aligned hourly, daily, or Monday-aligned weekly
  buckets for terminal task completions, current planned task lines completed,
  terminal test outcomes, and provider-reported total tokens. Missing test or
  token evidence remains visibly missing and is never converted to zero.
  Discrete completions render as bars and their running total as a separate
  thin line with an explicit legend; no filled area implies another measure.
  The line paints behind bars and haloed value labels so it cannot hide a
  daily number at an intersection.
- **REQ-PROGRESS-02** (2026-08-30, done): Terminal test summaries are copied
  into a symlink-safe, repository-local metadata history bounded to the latest
  1,000 runs. It contains no output, caller identity, private path, or command;
  malformed history makes test coverage partial/unavailable without changing
  the authoritative current test result.
- **REQ-PROGRESS-03** (2026-08-30, done): The next open release receives a
  deterministic likely-date range and confidence only when completion pace is
  measurable. The result exposes remaining sized and unsized work, recent
  velocity, test stability, scope movement, assumptions, and the absence of a
  target date. No release or insufficient pace produces an honest unavailable
  state instead of a guessed date.
- **REQ-PROGRESS-04** (2026-08-31, done): Open work for the next release follows
  the same depth-first sibling order as the Plan and uses the owner-facing task
  title, recorded status, estimate, elaboration request, and a simple reopened
  state. Raw outcome, impact, unblock, verification, technical, and event-note
  prose does not render in this compact surface. Progress does not present a
  heuristic as owner priority and does not infer dependency, release-impact
  days, or a what-if ordering effect until those inputs are explicitly modeled
  and recorded.
- **REQ-PROGRESS-05** (2026-08-31, done): One responsive Progress destination
  implements the owner-selected factual release-work design. Repository,
  period, task selection, exact-value, project-menu, and exact Plan-continuation
  controls all work through the rendered UI; task selection is a local DOM
  update without another API read or page replacement. The named collection
  and selected repository remain primary across loading, empty, partial,
  denied, error, populated, long-content, desktop, 799 px, and narrow states.

## Console navigation (REQ-CONSOLE)

- **REQ-CONSOLE-01** (2026-08-30, done): The global Console header remains one
  row at every supported width. Primary destinations remain ordinary links;
  at 1240 px and below those same links move into an accessible custom DOM
  hamburger menu with truthful expanded state, keyboard dismissal, and focus
  restoration instead of wrapping the shell.
- **REQ-CONSOLE-02** (2026-08-30, done): Every destination heading is a real
  link back to that destination's collection. Repository-scoped Plan,
  Progress, Decisions, and Codex Usage details show the current project beside a compact
  button that opens a custom DOM menu of every visible project, supports arrow
  keys and Escape, and navigates through real same-destination project links.
- **REQ-CONSOLE-03** (2026-08-30, done): Health leads with one aligned host
  capacity group and a separate compact operational-status group before
  incidents, history, and repository attribution. Unhealthy cards keep their
  natural height. Repository attribution becomes a labelled stacked layout at
  960 px and below, and every shared-storage category keeps its label and value
  visible without document-level horizontal scrolling.
- **REQ-CONSOLE-04** (2026-09-03, done): Tests keeps the run collection as
  its primary content, lets an administrator start development, pre-merge, or
  release validation with release selected by default, and places host-wide
  Capacity in a focused dialog showing learned/effective capacity, cap,
  active/waiting work, pause state, and last-adjustment evidence. Saving or
  clearing the cap acts immediately. While a run remains active, the collection
  refreshes without moving focus or closing dialogs; evidence is labelled
  pending, available with an image count, invalid, unavailable, or not produced
  from current retained state instead of exposing one unconditional action.
  Every authorized administrator action
  acts without a second confirmation dialog; destructive controls name their
  target and effect, and deployment removal has separate keep-data and
  delete-data actions.

## Public access (REQ-ACCESS, P5)

- **REQ-ACCESS-01** (P5, done): An invited user authenticates at the edge and accesses
  only granted deployments; grants bind to immutable deployment IDs.
- **REQ-ACCESS-02** (P5, done): Roles are exactly access / viewer / operator /
  administrator; no per-action permission matrices.
- **REQ-ACCESS-03** (P5, done): Revocation takes effect on the next edge/API request.
- **REQ-ACCESS-04**: Local agent calls via the Unix socket are independent
  of public deployment grants; the kernel peer UID is the caller identity
  and request bodies cannot assert identity (P1, in scope).

## Planning, completion ledger, and decisions (REQ-PLAN, Schemas 8, 11, and 13)

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
- **REQ-PLAN-11** (S11, done): An administrator can mark any task as needing
  elaboration. The mark persists with the task and appends a permanent
  request event. Every repository-scoped planning, task, release, and
  decision response carries the complete outstanding request projection,
  independent of the compact task cap. An agent may clear the mark only in
  the same update that changes the task title or outcome into clearer
  owner-facing language; completion appends its own event.
- **REQ-PLAN-12** (2026-09-02, done): A due rolling decision summary is
  append-only administrative maintenance. An authorized working agent stores
  it directly without requesting another user approval; every decision and
  prior summary remains permanent.

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
- **REQ-REL-07** (2026-08-28, done): A lost client reply never cancels an
  accepted deployment mutation; callers re-query observable status. HTTP/TCP
  readiness aborts promptly when the underlying unit or container becomes
  irrecoverably terminal after its restart policy, while recoverable restarts
  retain the configured readiness window.
- **REQ-REL-08** (2026-09-02, done): The three exhaustive audit skills
  resolve their installed direct links to the one root `full_repo_harness` in
  the canonical live checkout. No vendored harness tree, synchronization tool,
  standalone skill package, fallback import, or standalone-package validation
  remains.

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
5. `test status` carries only structured diagnostics and logical log
   references; catalogue-first tail/search/range/failure-context requests are
   bounded while their stored per-case streams remain complete
   (REQ-TEST-05, REQ-TEST-19 through REQ-TEST-22).
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
