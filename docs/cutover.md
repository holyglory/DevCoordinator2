# Migration and Cutover Runbook (Phase 8)

Every step here that changes the live public edge, fences the legacy
system, removes direct Docker access, or imports live state is executed by
the owner, deliberately, with a rollback path. Concrete names (domain,
accounts, legacy unit names, paths) are instance data kept in the untracked
`instance/` directory; this runbook uses placeholders.

## State of play after Phase 8 tooling

- A **canary** instance is installed beside the legacy system
  (`scripts/install.py --canary`): daemon on `/run/devcoordinator2/daemon.sock`,
  edge http-only on a private port, release under `/opt/devcoordinator2`,
  CLI shim `/usr/local/bin/devcoordinator2`, client group and edge system
  user created, instance configuration templates in `/etc/devcoordinator2/`.
  The legacy edge keeps 80/443. Nothing legacy was modified.
- `scripts/legacy_export.py` produced a reviewable read-only export of the
  legacy stores (no secrets) at `instance/legacy-export.json`.
- `scripts/legacy_import.py --dry-run` prints what would be imported and a
  deployment declaration plan; without `--dry-run` it imports
  administrators, Telegram chat links/subscriptions, and open bugs.

## Pre-cutover proofs (handover §20)

1. **Public authentication/grant boundary** — register the console
   redirect URI with the identity provider for the canary origin, fill
   `/etc/devcoordinator2/edge/oidc.client_id|client_secret`, restart the
   edge, sign in as the bootstrap administrator, invite a second identity,
   confirm a granted route works and a revoked one is denied.
2. **One permanent heterogeneous deployment** (worker + PostgreSQL) — add
   `[deployment.*]` declarations (from the import plan) to one repository,
   `devcoordinator2 deployment apply <path> --name <name>@checkout`, verify
   via Console; repeat for a `@worktree` instance if wanted.
3. **Immediate test with Docker/PostgreSQL cleanup** — `[test.<name>.postgres]`
   in that repository; `devcoordinator2 test start`, verify the container is
   gone afterwards.
4. **Attribution and health reconciliation** — `devcoordinator2 health
   repositories` / Console Health: managed + DevCoordinator + other = host.
5. **Telegram and bugs** — put the bot token in the file named by
   `DEVCOORDINATOR2_TELEGRAM_TOKEN_FILE` (mode 0600), restart the daemon,
   `/start` the bot, `devcoordinator2 telegram link --code … --email …`,
   subscribe, trigger a deployment event; `devcoordinator2 bug report …`.

## Cutover

6. **Backup** legacy state (authority DB, edge publication, access control,
   Telegram state) to a dated directory outside both repositories.
7. **Fence legacy mutations**: stop the legacy API/broker/test units
   (names in `instance/legacy-notes.md`) but leave its edge running.
8. **Import reviewed state**: `scripts/legacy_import.py --export
   instance/legacy-export.json --state-dir /var/lib/devcoordinator2
   --bugs-dir /var/lib/devcoordinator2-bugs`; apply the reviewed deployment
   declarations so every public route of step 2 exists with its domain;
   re-decide the pending access request (invite or ignore).
9. **Switch the edge**: reinstall without `--canary` (TLS credentials in
   `/etc/devcoordinator2/edge/`, `EDGE_HTTP_ONLY=0`, ports 80/443), then
   `scripts/edge_switch.py --to devcoordinator2 --legacy-units <legacy edge
   units> --yes`. Rollback at any time: `--to legacy --yes`.
10. **Docker authoritative mode** (owner decision DC2-…-DOCKER-MODE): remove
    agent accounts from the `docker` group, restart their sessions, verify
    `devcoordinator2 health containers` attributions; the observational
    label disappears from the security assumptions.
11. **Rollback window**: keep the stopped legacy release and its backup
    until the new Console and agent journeys are verified through their
    installed surfaces; only then decommission (`docs/legacy-deletion-map.md`).

## Uninstalling the canary

`systemctl disable --now devcoordinator2 devcoordinator2-edge`, remove
`/etc/systemd/system/devcoordinator2*.service`, `/etc/tmpfiles.d/devcoordinator2.conf`,
`/usr/local/bin/devcoordinator2`, `/opt/devcoordinator2`, and — only if the
state is not wanted — `/var/lib/devcoordinator2*`, `/etc/devcoordinator2`,
the `devcoordinator2-edge` user and `devcoordinator2-clients` group.
