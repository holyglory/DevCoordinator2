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

[test.unit.postgres]        # optional: test-scoped ephemeral PostgreSQL
image = "postgres:16-alpine"   # official postgres:<tag> only (default shown)
database = "test"              # [a-z_][a-z0-9_]{0,62}
user = "test"
```

An ephemeral PostgreSQL is one throwaway instance per run: a Docker
container carrying the exact run identity in daemon-owned labels, data on
tmpfs, published on loopback only, credentials generated per run. The test
process receives `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`,
and `DATABASE_URL` through a caller-owned 0600 environment file (never via
argv or the unit's public environment). The container is removed on
completion, timeout, cancellation, supersession, daemon recovery, or the
next start. Declared and injected environment values never appear in
summaries, logs, metrics, or agent results.

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

## Phase 3 schema: deployments (implemented)

```toml
[deployment.web]
source = ["checkout", "worktree"]   # "worktree" (default; live checkout), "checkout" (immutable
                                    # generations), or both — each enabled source is an independent
                                    # deployment instance (own identity, ports, data, generations)
domain = { checkout = "app", worktree = "app-dev" }   # per-source labels under the instance base
                                                      # domain; a plain string is allowed with one source
components = ["db", "api", "worker", "cache"]   # declared order; stop is reverse
build = ["npm", "run", "build"]                 # optional argv run as the caller before components start
ttl_seconds = 86400          # optional: temporary (preview) deployment, auto-stopped when expired
public = false               # true: the route needs no sign-in at the edge

[deployment.web.component.db]
type = "postgres"            # dedicated instance; persistent named volume, never deleted by stop/redeploy
image = "postgres:16-alpine"
database = "app"
user = "app"
# shared_from = "<deployment_id>/<component>"   # use another deployment's dedicated instance instead

[deployment.web.component.api]
type = "process"
command = ["npm", "run", "start"]   # argv only
cwd = "."
port = true                  # daemon leases a host port, injected as PORT
route = true                 # the domain routes to this component (optional when exactly one process/docker component leases a port)
health = { path = "/healthz", timeout_seconds = 60 }   # or { tcp = true, timeout_seconds = 30 }
env = { NODE_ENV = "production" }
depends_on = ["db"]          # narrow ordering within the declared order
independent_control = true   # default true: may be started/stopped/restarted alone
persistent_paths = ["var/data"]   # repo-relative data the product must never delete

[deployment.web.component.worker]
type = "process"
command = ["npm", "run", "worker"]
depends_on = ["db"]

[deployment.web.component.cache]
type = "docker"
image = "valkey/valkey:9.1.0-alpine"
command = []                 # optional container argv
env = {}
port = 6379                  # optional container port; published on a leased loopback port
volumes = ["data:/data"]     # named persistent volumes (daemon-owned names), never deleted by stop/redeploy

[deployment.web.component.stack]
type = "compose"
file = "docker-compose.yml"  # repo-relative; project name derived from the deployment identity
services = []                # optional subset

[deployment.web.component.smtp]
type = "external"            # observed only, never owned or controlled
tcp = "127.0.0.1:25"
```

Environment injected into every component of a deployment:
`DC2_DEPLOYMENT`, `DC2_COMPONENT`, `DC2_GENERATION`, `PORT` (own leased
port when `port` is set), `DC2_PORT_<NAME>` for every leased port in the
deployment, `DC2_POSTGRES_<NAME>_URL` for every dedicated or shared
PostgreSQL component, and `DATABASE_URL` when exactly one exists. Generated
PostgreSQL credentials live in private daemon state and reach components
only through 0600 environment files or container environment.

Instances are addressed as `<name>@<source>` (`web@checkout`, `web@worktree`);
a single-source deployment may be addressed as just `<name>`. A worktree
instance may use the checkout instance's database via `shared_from`.

Validation adds to the Phase 1 rules: component names `[a-z0-9][a-z0-9-]{0,31}`;
every listed component has a table and vice versa; `depends_on` references
earlier components only; at most one `route = true`. When `domain` is set and
no component declares `route = true`, a deployment with exactly one
port-leasing process/docker component routes to it implicitly (2026-08-24);
with several, `route = true` is required. `domain` labels
`[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?`; docker images
`name[:tag]` without privileged flags, host mounts, or socket access;
`compose.file` inside the repository; `shared_from` is exclusive with
`image`/`database`/`user`.

## Reserved (later phases)

User, grant, Telegram, and route-publication settings are never repository
configuration.
