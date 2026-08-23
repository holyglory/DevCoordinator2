# Decision History

Compact record of owner-level decisions. Each entry states the decision,
the alternatives considered, and what context the owner was given (per the
informed-owner-decision policy). Installation-specific values are referenced
via the untracked `instance/` directory.

Format: `DC2-YYYY-MM-DD-TOPIC — Decision`.

## DC2-2026-08-22-LANGUAGE — Python 3 daemon + static browser UI

**Decision.** Implement `devcoordinatord` in Python 3 (stdlib-first) with a
static browser Console. One server implementation only.

**Alternatives.** Rust single binary: no runtime dependency, lower memory,
but noticeably longer development, harder quick fixes, no reuse of reviewed
legacy Python algorithms, and systemd/Docker/SQLite integration rebuilt from
scratch.

**Owner context.** Presented in plain language with costs/risks of both and
the observation that legacy slowness came from orchestration layers, not the
interpreter. Owner chose Python (the handover's recommendation).

## DC2-2026-08-22-TELEGRAM-BOT — One server-owned bot

**Decision.** The product operates one server-owned Telegram bot; the token
is instance configuration. The existing bot is migrated at cutover.

**Alternatives.** Retain the legacy multi-bot model (any authorized user
registers a bot): more code and state (bot registry, per-bot polling,
approval queues) with no current need — in practice exactly one bot exists,
owned by the owner, covering 8 repositories.

**Owner context.** Presented with the live single-bot evidence and the
future cost of re-adding user-owned bots if ever needed. Owner chose the
single server bot.

## DC2-2026-08-22-DOCKER-MODE — Observational first, authoritative at cutover

**Decision.** Development and first launch run in observational mode: agent
accounts keep their current direct Docker access; containers the daemon did
not create are honestly `unmanaged/unknown`. Removing direct Docker group
access is an explicit reviewed cutover step in the migration phase.

**Alternatives.** Authoritative from the start (remove agent accounts from
the docker group immediately): cleaner attribution but breaks current agent
workflows and the legacy system's own Docker operations during coexistence.

**Owner context.** Presented with the concrete current group membership
(see `instance/local-accounts.md`) and the consequence of early removal.
Owner chose observational-first.

## DC2-2026-08-22-SCOPE — First delivery = Phase 0 records + Phase 1 foundation

**Decision.** This delivery produces the Phase 0 product records and then
continues directly into the Phase 1 foundation (repository IDs, Unix-socket
daemon, systemd test launch, result files, CLI/MCP) without a separate
approval pause between them.

**Alternatives.** Stop after Phase 0 for record review (the handover's
default). Owner accepted the risk that record changes may rework early code.

**Owner context.** Both options and the rework risk were stated explicitly.

## DC2-2026-08-22-EDGE — Node.js edge reusing trimmed legacy parts

**Decision.** The stable public edge remains Node.js and may reuse parts of
the proven legacy edge program (TLS termination, sign-in, sessions, atomic
route document, last-known-good behavior), trimmed and without
overengineering. Edge implementation is Phase 5; only the route-document
contract (`docs/route-document.md`) is defined now.

**Alternatives.** (a) Small custom Python edge: one language everywhere but
rewrites all proven sign-in/session/proxy logic. (b) Off-the-shelf proxy
(Caddy/nginx) + separate auth service: battle-tested TLS but a third piece
of software, awkward dynamic routes/grants, and the auth service must be
written anyway.

**Owner context.** All three presented with costs; owner explicitly chose
Node.js reuse, conditioned on correct routes and no overengineering.

## DC2-2026-08-22-METRICS — 15 s sampling, 1-min aggregates, 30-day retention

**Decision.** Sample CPU/memory every 15 seconds, persist one-minute
aggregates, sample storage every five minutes, retain **30 days**, delete
expired rows directly. No downsampling tiers, no analytics product.

**Alternatives.** The handover proposed 7-day retention; owner chose 30 for
month-over-month comparison, accepting ~4× (still modest) metric storage.
Recorded as a deliberate extension of the handover's proposal.

**Owner context.** Sampling cadence, storage cost, and query impact stated.

## DC2-2026-08-22-NO-INSTANCE-DATA — Zero installation data in the repository

**Decision.** The committed repository contains no installation-specific
data: no domain, owner e-mail, local account or group names, host paths of
the legacy installation, or certificate locations. Committed docs use
placeholders; concrete values live in the untracked `instance/` directory
and in installed instance configuration (`docs/instance-configuration.md`).
`scripts/check_no_instance_data.py` enforces this against a pattern list
that itself lives in `instance/` (so the patterns are not committed either).

**Alternatives.** Committing instance facts (the initial handover drafts did
this) — rejected by the owner: the repository must be reusable and safe to
publish to a remote.

**Owner context.** Owner stated the requirement directly.

## DC2-2026-08-22-LAUNCH-MODE — Root daemon + systemd-run --uid/--gid

**Decision.** The daemon runs as root in a hardened unit and launches each
test/deployment process as the physical caller via one argv-built
`systemd-run` transient unit (`--uid`/`--gid`, explicit
`SupplementaryGroups=`, `KillMode=control-group`, `RuntimeMaxSec`,
`TimeoutStopSec=10s`, `UMask=0077`). PID 1 applies the credential change;
the daemon never manipulates credentials itself.

**Alternatives.** (a) Non-root daemon + polkit rule for transient units:
the required polkit action is root-equivalent anyway (start any unit as any
user) expressed through an extra, easily-misread artifact; note the legacy
"test rules" file sometimes cited as polkit precedent is actually agent
approval configuration — the legacy authority simply ran as root. (b)
Per-user systemd managers: needs lingering managers per account and splits
cgroup accounting. (c) setuid launcher: worst auditability.

**Owner context.** Presented in the approved implementation plan with the
polkit-premise correction; approved with the plan.

**Obligations.** Symlink-attack hardening for root file operations inside
caller-writable trees (see `security-assumptions.md`).

## DC2-2026-08-22-OUTPUT-CAPTURE — systemd-run --pipe with daemon-side draining

**Decision.** Test output is captured via `systemd-run --pipe`: the daemon
holds the unit's stdout/stderr pipes, drains continuously, retains up to a
fixed cap per stream in repository-local log files, and keeps counting
observed bytes past the cap so a noisy child can never block or fill the
server. `systemd-run --pipe` also returns the unit's exit status. Timeout is
detected from the unit's `Result=timeout`.

**Alternatives.** Journal capture (`StandardOutput=journal`, the legacy
approach): survives daemon death but violates the handover's repository-
local bounded log files and adds journal extraction complexity. If the
daemon dies, the unit survives and restart recovery marks it interrupted —
consistent with the handover.

**Owner context.** In the approved implementation plan.

## DC2-2026-08-22-SMALL-CHOICES — Attribution, socket gate, registration, caps

Approved as part of the implementation plan:

- **Attribution**: `summary.json` includes `caller_uid` and `client` in
  addition to the handover §8 field list — a deliberate extension so §23
  accountability holds with one file.
- **Socket gate**: a dedicated daemon client group (name from instance
  configuration; proposed `devcoordinator2-clients`), socket mode 0660.
- **Registration**: repositories register implicitly on first `test.start`;
  an explicit `repository.register` also exists.
- **Caps**: 4 MiB retained log per stream; 64 KiB maximum tail per
  `test.output` request; 64 KiB request / 256 KiB response frames.
- **Repository identity**: deterministic ID from the realpath of the Git
  common root. Moving a repository directory yields a new identity —
  acceptable for a single-owner server; revisit before Phase 3 builds
  deployments on these IDs.

## DC2-2026-08-22-MCP — Hand-rolled stdlib STDIO MCP server

**Decision.** The MCP surface is a small stdlib JSON-RPC 2.0 STDIO server
(`initialize`, `notifications/initialized`, `ping`, `tools/list`,
`tools/call`) returning the exact shared protocol result JSON as text
content.

**Alternatives.** The official `mcp` package brings pydantic/anyio/httpx
into a stdlib-first product for five tools and one transport — not
justified. **Revisit trigger:** a real client requires MCP features
(resources, sampling, server notifications) the thin server lacks, or
protocol-version negotiation breaks against a supported client.

## DC2-2026-08-22-TEST-POSTGRES — Ephemeral PostgreSQL as a labeled Docker container

**Decision.** A test's `[test.<name>.postgres]` provisions one throwaway
PostgreSQL per run as a Docker container (official `postgres:<tag>` images
only), data on tmpfs, loopback-published random port, per-run generated
credentials delivered through a caller-owned 0600 `EnvironmentFile`. The
daemon records the exact full container ID beside the summary
(`containers.json`) and removes it on completion, timeout, cancellation,
supersession, daemon recovery, or the next start — by exact ID or by its own
instance+purpose labels only. Docker prune is never used.

**Alternatives.** (a) A systemd-managed `postgres` process per run: avoids
Docker but needs a host PostgreSQL install of every wanted version and
manual initdb/cleanup. (b) A shared long-lived test database with per-run
schemas: fast but violates exact per-run ownership and cleanup. Shared
permanent PostgreSQL as a declared test dependency is a Phase 3 deployment
concept and is not stopped or cleaned by tests.

**Owner context.** Follows the handover's stated model (test-scoped
PostgreSQL carrying the exact test identity, removed with the run). Applied
autonomously under the "continue towards phase 8" instruction; recorded for
review.

## DC2-2026-08-22-DEPLOYMENT-SOURCE — Live-worktree deployments by default, immutable checkouts on request

**Decision.** A deployment declares `source = "worktree"` (default) or
`source = "checkout"`. Worktree deployments run directly from the live
repository checkout so an agent's edits can be applied and observed
immediately (`apply`/`restart` picks up the current tree); they have no
previous generation and the product never pretends otherwise. Checkout
deployments create an immutable per-generation copy (a detached git
worktree at the applied commit under a daemon-owned deployment root, plus an
optional build step) and keep the current and immediately previous
generation for exact rollback. Components, ports, domains, health checks,
and controls are identical in both modes. A declaration may enable both
sources at once (`source = ["checkout", "worktree"]`) with a domain per
source; each source is then an independent deployment instance
(`web@checkout`, `web@worktree`) with its own identity, ports, generations,
and data, sharing one component declaration.

**Alternatives.** Immutable checkouts only (exact rollback, isolation from
in-progress edits, ~2 extra copies of each deployed repository on disk) or
live checkout only (simplest, but no real rollback). The owner wants both:
live-worktree for guiding development by immediately seeing results, and
checkouts for permanent deployments.

**Owner context.** The rollback/isolation trade-off was presented in plain
language; the owner chose both modes and stated live-worktree will be the
most used. Applied under the "continue towards phase 8" instruction.

## DC2-2026-08-22-DEPLOYMENT-SCHEMA — `[deployment.<name>]` configuration schema

**Decision.** Recorded before Phase 3 code, per the handover. The schema is
specified in `docs/repository-config.md` (Phase 3 section): named
deployments with `source`, an optional `domain` label resolved under the
instance base domain, ordered `components`, and component tables of type
`process`, `docker`, `compose`, `postgres` (dedicated, or `shared_from` an
existing deployment component), or `external` (observed, never owned).
Ports are leased by the daemon from an instance-configured range and
injected as environment; repositories never claim host ports or domains
literally. Persistent data is declared explicitly (`persistent = true` /
named volumes / paths) and is never deleted by stop, restart, or redeploy.

**Alternatives.** Re-using the legacy two-file JSON declarations
(`dev-runtime.json` + `tests.json`): rejected by the handover (one small
reviewed TOML file, no sealed templates, no admission values).

**Owner context.** Schema content follows the handover's allowed/forbidden
lists; the source-mode choice above is the owner decision within it.

## DC2-2026-08-23-PUBLIC-IDENTITY — Edge-asserted identity, roles in the daemon

**Decision.** The edge authenticates users (OIDC, reusing the reviewed
legacy client) and enforces route access from the route document's
`access` section (owners + grants bound to deployment IDs). For Console/API
calls the edge forwards the signed-in e-mail as `client.identity`; the
daemon accepts an identity only from the configured edge uid
(`DEVCOORDINATOR2_EDGE_UID`) and applies the four-role model per request
from its database. Bootstrap administrators come from instance
configuration; invitations admit one exact identity and are accepted by the
edge after a verified sign-in. Actions performed on behalf of a public
identity execute as the Unix account that created the deployment; public
callers address deployments by ID so no git ever runs as the edge user.

**Alternatives.** Shared secret between edge and daemon (an extra secret to
manage; peer uid is already kernel-verified). Sessions validated by the
daemon (makes the edge depend on the daemon on every request — violates
edge availability across daemon restarts).

**Owner context.** Follows the handover's access model; applied under the
"continue towards phase 8" instruction; the identity provider remains the
one in instance configuration (Google by default, as legacy).
