# Repository Configuration: `.devcoordinator.toml`

One small reviewed file at the repository root. Configuration is canonical
for commands and component meaning; the coordinator database is canonical
for live assignments, identities, state, users, grants, and observations.

## Phase 1 schema (implemented)

```toml
schema = 1

[test]
default = "unit"            # optional; required if more than one test

[test.unit]
command = ["python3", "-m", "pytest", "-q"]   # argv array, REQUIRED
cwd = "."                   # optional, repo-relative, default "."
timeout_seconds = 600       # optional, 1..21600, default 600
env = { CI = "1" }          # optional, string→string; additive only
```

Validation rules:

- `schema` must be `1`.
- `command` is a non-empty array of non-empty strings. A single string is
  rejected: shell strings are forbidden everywhere.
- `cwd` must resolve (realpath, after joining) inside the repository; `..`
  or symlink escape is rejected.
- `timeout_seconds` integer in [1, 21600].
- `env` values must not look like secrets (no key names matching
  token/secret/password/key patterns with literal values — reference
  secrets held outside the repository instead).
- Test names: `[a-z0-9][a-z0-9-]{0,31}`.
- Unknown keys anywhere are rejected (`repository_config_invalid`), so
  typos never silently change meaning.

## Forbidden content (rejected by validation)

- Host filesystem paths outside the repository or approved deployment
  roots.
- Literal secrets of any kind.
- Docker socket operations, privileged flags, or raw Docker options.
- CPU/memory/PID admission values.
- Test retry, queue, evidence, history, or retention policies.
- Public users and grants.

## Reserved sections (later phases, schema unchanged until decided)

`[deployment.<name>]` with ordered `components` (process/service, docker,
compose, postgres dedicated|shared, external passive), health checks,
persistent vs disposable paths/volumes, requested ports and domains
(domain values themselves come from instance configuration), narrow startup
dependencies, and references to secrets held outside the repository. Final
deployment schema requires its own recorded decision before code.
