# PostgreSQL/v3 activation and recovery

## Current PostgreSQL/v3 activation

The authority migration happens once, after isolated full-source and migration
rehearsals. Build and verify the four stamped binaries: `devcoordinator2`,
`devcoordinator2-tooling`, `devcoordinator2-executor`, and
`devcoordinator2-migrate-authority`. Keep binaries, PostgreSQL configuration,
backups and raw evidence outside source. The complete release contract is
[acceptance-v3.md](acceptance-v3.md).

`install activate --candidate-manifest FILE --candidate-acceptance FILE --yes`
requires a root-owned mode-0600 acceptance receipt tied to the exact source
commit, all four binary hashes and completed workspace, skills, root acceptance,
migration, restart, critical API and functional checks. It drains accepted work,
fences the endpoint, stops old writers, creates the verified frozen SQLite
backup, imports the native PostgreSQL schema, restarts PostgreSQL and compares
the full import before candidate startup. The dedicated PostgreSQL 17 service
runs as `devcoordinator2-pg`, with a private Unix socket and no TCP listener.
The daemon reads a root-owned mode-0600 connection file.

While the fence remains, the candidate accepts read operations using exact
identities; path-based reads that could register a repository are refused.
Periodic maintenance, external delivery and deferred recovery wait for commit.
One controlled initial health observation supports candidate health checks.
Keep the Console on its matching source during the freeze: changing the canonical
checkout must never serve v3 JavaScript through the old live edge protocol.

Activation durably commits the PostgreSQL installation before admitting new
mutations. Its result is `activated_awaiting_final_acceptance`; the old SQLite
source and temporary rollback copy remain protected. A failure before admission
can restore the prior daemon and frozen SQLite state. Once admission opens,
recovery must preserve PostgreSQL writes and cannot restore stale SQLite.

Verify the live v3 browser journeys, edge/source identity, critical journeys and
API results, and reconnection of persistent MCP clients. Replacing the binary
on disk does not update an already-running stdio adapter. Retain that evidence
and create a fresh root-owned mode-0600 live acceptance receipt bound to the
same commit and four binaries, with an observation time after activation.

Only then run:

```sh
devcoordinator2-tooling install finalize \
  --transaction-dir /absolute/activation \
  --live-acceptance /absolute/live-acceptance.json --yes
```

Finalization verifies a fresh PostgreSQL dump through an isolated restore, checks
source identities, and retires only the verified frozen SQLite source and
temporary migration backup. It is idempotent and retains a finalization receipt.
Unrelated databases, repository test state, open bug files and external usage
collectors are never retirement targets. PostgreSQL recovery backups remain.

The following sections describe deployment proofs and historical snapshot inspection;
protocol-v3 calls use `devcoordinator2 call <operation> --params JSON` or a bounded
`--params-file FILE`.

Every step here that changes the live public edge, fences the legacy
system, removes direct Docker access, or imports live state is executed by
the owner, deliberately, with a rollback path. Concrete names (domain,
accounts, legacy unit names, paths) are instance data kept in the untracked
`instance/` directory; this runbook uses placeholders.

## Rust cutover preparation

- `devcoordinator2-tooling install configure` creates or preserves the client
  group, edge account, instance configuration, private source policies,
  direct skill/policy links, and the edge's read-only access to the canonical
  checkout. `--canary` keeps the edge HTTP-only on the selected private port.
- `devcoordinator2-tooling install build` accepts only a clean canonical
  `main` equal to the already-fetched `origin/main`. It builds all four Rust
  executables as the checkout owner, probes their embedded commit, hashes
  them, and writes a root-owned mode-0600 candidate manifest. There is no
  copied release tree.
- `devcoordinator2-tooling install verify` rechecks the candidate manifest,
  embedded commits, hashes, and executable identities. `install plan` shows
  the exact unit and direct-link targets without changing the live service.
- `devcoordinator2-tooling legacy export` can inspect older non-authority stores.
  Keep its export private and outside source; it is separate from the complete
  SQLite authority migration.
- `devcoordinator2-tooling legacy import --dry-run` prints what would be imported and a
  deployment declaration plan; without `--dry-run` it imports
  administrators, Telegram chat links/subscriptions, and open bugs.

## Pre-cutover proofs (handover §20)

### Inspect missing planning records without restoring authority

Use the compiled local tool against one explicitly selected activation directory:

```sh
devcoordinator2-tooling planning inspect-backup \
  --transaction-dir /absolute/saved/activation \
  --repository-id r0000000000000001 \
  --task-id p0000000000000001
```

It verifies the selected private activation snapshot. Historical schema-1/2
`authority-before.sqlite3` archives use immutable read-only SQLite without
migrations or WAL/SHM creation. Schema-3 `authority-before.pgdump` archives are
restored into a uniquely owned temporary PostgreSQL database, inspected read-only,
and removed without touching the configured authority database.
The result contains repository-scoped planning counts and the repository identity
of each explicitly requested task, not private record text or installation
contents. It checks file identity and hashes before and after inspection.
For requested tasks in the selected repository, it also reports the saved
status, update timestamp and list position when those columns are available.
Statuses and timestamps are validated before disclosure; unrelated tasks expose
only their repository identity. These observations do not choose a conflicting
version or authorize a live write.
Optional `--identities` adds up to 4,096 IDs and ordering numbers per planning
family, plus hashes of decision references, for collision analysis. It does not
export titles, outcomes, reference text or private installation values.
Schema-2 snapshots must match their recorded backup hash; schema-1 snapshots
without that historical binding are labelled `legacy_without_recorded_hash`.
The newly calculated hash identifies the inspected bytes but does not establish
their historical authenticity or completeness.

This command does not restore records, change live authority, or make a backup
the current completion ledger. Missing-record recovery requires a separately
reviewed plan that preserves newer authoritative records; do not use whole-state
installation rollback to merge missing planning history.

### Required proofs

1. **Public authentication/grant boundary** — register the console
   redirect URI with the identity provider for the canary origin, fill
   `/etc/devcoordinator2/edge/oidc.client_id|client_secret`, restart the
   edge, sign in as the bootstrap administrator, invite a second identity,
   confirm a granted route works and a revoked one is denied.
2. **One permanent heterogeneous deployment** (worker + PostgreSQL) — add
   `[deployment.*]` declarations (from the import plan) to one repository,
   `devcoordinator2 call deployment.apply --params-file /absolute/apply.json`, verify
   via Console; repeat for a `@worktree` instance if wanted.
3. **Immediate test with Docker/PostgreSQL cleanup** — `[test.<name>.postgres]`
   in that repository; `devcoordinator2 call test.start`, verify the container is
   gone afterwards.
4. **Attribution and health reconciliation** — `devcoordinator2 call health.repositories` / Console Health: managed + DevCoordinator + other = host.
5. **Telegram and bugs** — put the bot token in the file named by
   `DEVCOORDINATOR2_TELEGRAM_TOKEN_FILE` (mode 0600), restart the daemon,
   `/start` the bot, `devcoordinator2 call telegram.link --params-file /absolute/link.json`,
   subscribe, trigger a deployment event; `devcoordinator2 call bug.report --params-file /absolute/bug.json`.

## Recovery and later updates

Use the exact activation receipt to select an interrupted transaction:

```sh
devcoordinator2-tooling install recover \
  --transaction-dir /absolute/interrupted-activation --yes
```

Recovery rejects a missing target, a completed transaction, a mismatched
installation, or changed backup evidence. Before admission it restores the
captured installation and authority binding. After admission it cannot restore
old SQLite or discard newer PostgreSQL writes. Recovery of missing individual
planning records uses the repository-scoped inspection and conflict-review
operations above; it is not a whole-database rollback.

Activation and recovery reconcile stable route leases and live listeners,
publish above the durable route-generation floor, and verify the edge accepted
the publication. A running service alone does not prove API readiness. Idle
observers reconnect from their last cursor after the activation window; accepted
finite operations must finish before the old process stops. Failed draining
reopens admission and preserves the accepted work.

Later updates are developed and validated in worktrees, merged to `origin/main`,
and fast-forwarded into the clean canonical checkout. Each activation uses a
new owned PostgreSQL candidate database restored from a verified private backup.
It does not modify the prior authority database in place. Retained SQLite
retirement obligations must be finalized before they can be superseded.

Uninstallation is a separate owner-authorized operation. Select exact service,
configuration and marker-bound state identities; never remove state with a
wildcard. PostgreSQL recovery backups, credentials, unrelated services and user
data are preserved unless their removal is explicitly authorized.
