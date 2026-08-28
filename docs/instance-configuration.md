# Instance Configuration

All installation-specific values live outside the committed repository:

- Installed daemon: `/etc/devcoordinator2/instance.env`
  (root-owned, mode 0640, group = the daemon client group).
- Development: an untracked `.env` in the checkout root (gitignored), or
  `DEVCOORDINATOR2_*` environment variables directly.
- Human notes (accounts, legacy paths): the untracked `instance/`
  directory.

Environment variables override file values; file values override defaults.
Format: `KEY=value` lines, `#` comments, no quoting semantics beyond
stripping one pair of surrounding quotes.

## Variables (Phase 1)

| Variable | Default | Meaning |
|---|---|---|
| `DEVCOORDINATOR2_SOCKET` | `/run/devcoordinator2/daemon.sock` | daemon Unix socket path |
| `DEVCOORDINATOR2_STATE_DIR` | `/var/lib/devcoordinator2` | authority DB and runtime state |
| `DEVCOORDINATOR2_UNIT_PREFIX` | `devcoordinator2-test` | transient test unit prefix (dev instances use e.g. `devcoordinator2-dev`) |
| `DEVCOORDINATOR2_SLICE` | `devcoordinator2-tests.slice` | parent slice for test units |
| `DEVCOORDINATOR2_CLIENT_GROUP` | `devcoordinator2-clients` | Unix group granted socket access |
| `DEVCOORDINATOR2_PORT_RANGE` | `20000-29999` | host port range the daemon leases to deployment components |
| `DEVCOORDINATOR2_BASE_DOMAIN` | (empty) | base public domain; deployment `domain` labels resolve beneath it (Phase 3+) |
| `DEVCOORDINATOR2_EDGE_UID` | (unset) | Unix uid of the edge service; the only peer allowed to assert a public identity |
| `DEVCOORDINATOR2_ADMIN_EMAILS` | (empty) | comma-separated bootstrap administrators |
| `DEVCOORDINATOR2_TELEGRAM_TOKEN_FILE` | (unset) | private 0600 file holding the single server-owned bot token; unset disables notifications |
| `DEVCOORDINATOR2_TELEGRAM_API` | `https://api.telegram.org` | API base (tests point it at a fixture) |
| `DEVCOORDINATOR2_BUGS_DIR` | `/var/lib/devcoordinator2-bugs` | independent open-bug store; world-writable so any local account reports bugs (DC2-2026-08-24-OPEN-LOCAL-ACCESS) |
| `DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE` | (unset) | absolute private schema-1 JSON file authorizing exact repository-ID/relative-path Compose interpolation files |

Edge configuration lives in `/etc/devcoordinator2/edge.env` (`docs/edge.md`).

## Compose environment-file authorization

Repository `env_file` declarations grant no authority by themselves. The
allowlist is root-owned, regular, non-symlink, at most 64 KiB, and not
group/world writable:

```json
{
  "schema": 1,
  "authorizations": [
    {"repository_id": "r0123456789abcdef", "path": "deploy/dev.env"}
  ]
}
```

Each path is normalized, relative, and exact. Runtime use additionally proves
that the current file is regular, non-symlink, stays inside the worktree, and
remains Git-ignored. Missing or malformed policy refuses daemon startup;
missing authorization refuses the repository operation. Values are never read
into Coordinator metadata or results.

The installer accepts repeatable
`--compose-env-authorization REPOSITORY=RELATIVE_PATH`; it resolves the Git
common-root identity, proves the existing file is ignored and safe, merges the
private allowlist atomically, and adds the instance-file pointer without
removing prior authorizations.

## Enforcement

`scripts/check_no_instance_data.py` scans committed content against the
untracked pattern list `instance/forbidden-strings.txt` and fails on any
match. Run it before every commit and in the acceptance checklist.
