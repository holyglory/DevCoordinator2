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
