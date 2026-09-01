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
command = ["python3", "-m", "pytest", "-q"]   # argv array; one-check form
cwd = "."                   # optional, repo-relative, default "."
timeout_seconds = 600       # outer runaway watchdog only; never readiness
env = { CI = "1" }          # optional, string→string; additive only

[test.unit.postgres]        # optional: test-scoped ephemeral PostgreSQL
image = "postgres:16-alpine"   # official postgres:<tag> (default), or a compatible
                                # image pinned as name@sha256:<64 lowercase hex>
database = "test"              # [a-z_][a-z0-9_]{0,62}
user = "test"

[test.complete]             # graph form: use check tables instead of command
timeout_seconds = 21600

[[test.complete.check]]
name = "build"
command = ["npm", "run", "build"]
produces = ["dist/app.js"]  # immutable regular-file receipts, repo-relative

[[test.complete.check]]
name = "server"
command = ["./scripts/start-test-server"]
requires = ["build"]        # waits for build and requires it to pass
completion = "event"        # emits its exact event, then may stay alive
on_failure = "stop"         # only for evidence-invalidating/unsafe failure

[[test.complete.check]]
name = "browser"
command = ["node", "verify.mjs"]
requires = ["server"]

[[test.complete.check]]
name = "package-report"
command = ["./scripts/package-report"]
after = ["browser"]         # runs after browser even when browser failed
```

In graph form every ready check starts concurrently. There is deliberately no
`max_parallel`, worker-budget, resource-lock, CPU, memory, or client-override
field. If two checks cannot safely overlap, declare their real completion or
success dependency. Every check receives an isolated
`DEVCOORDINATOR_CHECK_SCRATCH`, the shared
`DEVCOORDINATOR_SHARED_ARTIFACTS`, and its exact run/check identity.

`completion = "process"` (default) uses the exact exit status. A long-lived
setup uses `completion = "event"` and emits one identity-bound result with
`devcoordinator2 test event passed|failed|unsafe`; the inherited descriptor,
not elapsed time, binds the event to that check. A passed long-lived process
stays available to dependents and is terminated during final cleanup. If it
exits early, downstream evidence is unsafe. `produces` paths are content-hashed
regular files; symlinks, missing files, path escape, and mutable receipts fail.

An ephemeral PostgreSQL is one throwaway instance per run: a Docker
container carrying the exact run identity in daemon-owned labels, data on
tmpfs, published on loopback only, credentials generated per run. The test
process receives `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`,
and `DATABASE_URL` through a caller-owned 0600 environment file (never via
argv or the unit's public environment). The container is removed on
completion, timeout, cancellation, supersession, daemon recovery, or the
next start. Declared and injected environment values never appear in
summaries, logs, metrics, or agent results.

Official PostgreSQL tags use the established preloaded-image path. A compatible
image outside that namespace must be immutable: the daemon pulls the exact
digest when absent, verifies the local repository digest, and then runs with
`--pull never`. Mutable derived-image tags are rejected. The image must honor
the standard `POSTGRES_USER`, `POSTGRES_PASSWORD`, and `POSTGRES_DB` entrypoint
contract and provide `pg_isready`; extensions remain repository-specific.

Validation rules:

- `schema` must be `1`.
- A test declares either one `command` or one or more `[[test.<name>.check]]`
  tables, never both. Every check command is a non-empty argv array. Shell
  strings are forbidden everywhere.
- `cwd` must resolve (realpath, after joining) inside the repository; `..`
  or symlink escape is rejected.
- `timeout_seconds` integer in [1, 21600].
- `env` values must not look like secrets (no key names matching
  token/secret/password/key patterns with literal values — reference
  secrets held outside the repository instead).
- Test names: `[a-z0-9][a-z0-9-]{0,31}`.
- Check names: `[a-z0-9][a-z0-9-]{0,63}`; names are unique, dependencies must
  exist, self-dependency and cycles are rejected, and `after`/`requires` may
  not repeat the same edge.
- Check `completion` is `process|event`; `on_failure` is `continue|stop`.
- `DEVCOORDINATOR_*` environment names are reserved for exact runner identity,
  scratch, artifact, and event delivery.
- Unknown keys anywhere are rejected (`repository_config_invalid`), so
  typos never silently change meaning.

## Forbidden content (rejected by validation)

- Host filesystem paths outside the repository or approved deployment
  roots.
- Literal secrets of any kind.
- Docker socket operations, privileged flags, or raw Docker options.
- CPU/memory/PID admission values.
- Repository-selected retry, queue, evidence-retention, concurrency-budget,
  or resource-quota policies. Diagnostic selection/retry is a Coordinator
  command with fixed semantics, not repository authority.
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
files = ["compose.yml", "compose.dev.yml"]  # or singular file = "compose.yml"
env_file = "deploy/dev.env"  # optional ignored repo-relative interpolation file;
                             # requires separate private instance authorization
services = ["bootstrap", "api", "worker"]  # optional subset; required with roles below
finite_services = ["bootstrap"]             # must exit 0 once per changed apply
independent_services = ["worker"]           # reviewed long-running service controls
build = true                 # build on apply; ordinary start never builds
port = true                  # leases PORT for Compose interpolation
route = true
timeout_seconds = 300        # finite + long-running readiness, 1..900

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
every `compose.files`/`compose.env_file` path stays inside the repository;
`finite_services` and `independent_services` are duplicate-free subsets of an
explicit `services` list and never overlap; at least one long-running service
remains. A changed apply removes/recreates the finite service containers,
requires successful completion, and retains bounded generation receipts.
Unchanged convergence and ordinary start do not rerun finite services. An
independent service is addressed as `<component>/<service>` and exact-container
start/stop/restart never follows dependencies or touches unrelated services.
`env_file` remains inert unless private instance configuration separately
authorizes the exact deterministic repository ID and normalized relative path;
the file must remain ignored, regular, and non-symlink on every use. Its values
never enter repository configuration, Coordinator metadata, results, logs, or
argv. Because ignored files are deliberately absent from immutable checkout
generations, this exception is available only to worktree deployments.
`shared_from` is exclusive with `image`/`database`/`user`.

## Reserved (later phases)

User, grant, Telegram, and route-publication settings are never repository
configuration.
