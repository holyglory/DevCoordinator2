# Legacy State: Import / Never-Import Map

The legacy installation (paths in the untracked `instance/legacy-notes.md`,
referenced here as `<legacy-repo>` and `<legacy-state-dir>`) is read-only
evidence. Cutover imports only reviewed live product state. Nothing is
imported wholesale; every import is an explicit, reviewed transformation
into the new schema.

## Import at cutover (reviewed, transformed)

| What | Legacy store | Notes |
|---|---|---|
| Active repositories required by deployments | legacy authority DB, `repositories` table | Re-register by path; new deterministic IDs |
| Active deployment definitions + exact native identities | legacy authority DB (`server_definitions`, `docker_resources`, `database_bindings`, compose definitions) | Only currently-live deployments; re-declared in `.devcoordinator.toml` + adopted identities |
| Current + previous deployment generations where usable | legacy authority DB | Only if cleanly mappable |
| Active ports and domains | legacy authority DB `port_assignments` + edge route document | 6 active port assignments at review time |
| Public users, outstanding valid invites, deployment grants | legacy Console access-control store | One owner; one pending access request to re-decide |
| Telegram bot/subscription configuration | legacy notifications state file (mode 0600) | Via secret-safe workflow into instance configuration; single-bot model |
| Current open bugs | legacy bug store | Open records only |
| Persistent data/volume identities that must survive | legacy authority DB + Docker labels | Never deleted, only adopted |

## Never import (delete with the legacy system at decommission)

| What | Legacy store |
|---|---|
| Test plans, runs, attempts, cases, artifacts, queues, retries, rollups | legacy `tests.sqlite3`, test-run state dirs |
| Operation journals and compatibility history | legacy authority DB `operations` and journal tables |
| Generic resource archives/tombstones | legacy authority DB |
| Old metrics history | legacy observer state |
| Legacy local account/repository grants | legacy authority DB (already removed by a late legacy decision) |
| Inactive cutover/handoff state | legacy handoff units/state |
| Pre-cutover store (multi-GB) and old per-user coordinator state | see `instance/legacy-notes.md` |
| Release archive (hundreds of immutable release dirs) | legacy install root; retain only the rollback-window release at cutover |

## Rules

- Never reset, clean, rebase, overwrite, or import legacy source files
  wholesale; small algorithms may be ported only after review against the
  handover.
- Never use the installed legacy system to test or deploy DevCoordinator2.
- Secrets move only through the secret-safe workflow into instance
  configuration; they never transit the repository, logs, or agent results.
- The concrete import inventory (exact rows, routes, identities) is
  prepared at cutover time and reviewed with the owner; this document fixes
  only the categories.
