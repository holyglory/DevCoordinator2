# Security Assumptions

Last reviewed: 2026-08-28 (digest fixtures and reviewed Compose environment path)

Installation-specific values (the concrete accounts, groups, domain, and
owner identity) are deliberately not in this file. They live in the
untracked `instance/` directory and in the installed instance configuration
(`docs/instance-configuration.md`).

## Confirmed local repository boundary

- A small set of local Unix accounts (enumerated in `instance/local-accounts.md`)
  is controlled by one owner and is intentionally not security-isolated
  within this repository.
- Every participating account requires equal full read, write, traversal,
  and Git-metadata access across the complete checkout, including `.git`.
- One shared Unix group (named in instance configuration) is the
  repository's single access mechanism. Access uses inherited group
  permissions and no named per-user ACL entries. New directories inherit the
  shared group and group `rwx`; new files inherit group read/write while
  preserving executable intent.
- No unrelated local users require access. World access is unnecessary.

## Runtime trust boundary

- One owner controls all trusted local Unix accounts and coding agents.
  Local accounts are attribution and execution identities, not mutually
  distrusting tenants.
- Public Console users and Internet clients are untrusted until
  authenticated at the edge and granted access to specific deployments.
- Repository source and permanent deployment/database data may be valuable.
  Test results and test scratch data are disposable and reproducible.
- Credentials, bot tokens, identity assertions, database passwords, and
  upstream secrets must not enter source, ordinary metadata, logs, metrics,
  or agent-facing results. They live only in instance configuration files
  outside the repository, systemd credentials, or private mode-0600 state.
- A repository declaration alone never authorizes an ignored Compose
  interpolation environment file. Private root-owned instance configuration
  must separately allow the deterministic repository identity and exact
  relative path, after that repository's confirmed assumptions deliberately
  accept disposable development credentials inside the same-owner checkout
  boundary. The daemon validates the authorization, ignored-file state, and
  realpath on every use; it passes only the path to Compose and never copies
  values into configuration metadata, results, logs, metrics, or argv. This
  policy is loaded only by the root daemon; thin CLI/MCP clients and deployed
  application APIs receive neither its contents nor filesystem authority. This
  narrow exception does not permit committed credentials, symlinks/path
  escape, unrelated-account access, or production secrets, and must be
  re-reviewed when the repository trust boundary changes
  (DC2-2026-08-28-COMPOSE-REPOSITORY-ENV).
- Runaway processes, containers, storage growth, stale work, malformed
  input, path escape, and lost replies are credible operational failures and
  are handled as such, not as security incidents.

## Codex usage analytics boundary

- The owner explicitly authorizes the root daemon to read the content-free
  usage databases of every same-owner Unix account listed in a private,
  explicitly configured source policy and to combine those measurements by
  registered repository (DC2-2026-08-29-CODEX-USAGE-ACCESS).
- Public Console access to these combined measurements requires
  administrator access or an operator-or-higher grant on a deployment of the
  repository. Viewer grants are insufficient. Local Unix-socket callers keep
  the existing trusted local authority.
- Results never identify the contributing Unix account or Codex account and
  never return source paths, repository HMAC material, credentials, prompts,
  model output, source, commands, tool payloads, raw errors, thread or agent
  identifiers, or per-user values. Missing, incompatible, or unmapped
  collectors are disclosed as partial coverage rather than zero usage.
- Each Codex usage database remains the only writable accounting truth.
  DevCoordinator opens it read-only and stores only the privacy-preserving
  link between its registered repository and that collector's repository key
  (DC2-2026-08-29-CODEX-USAGE-SOURCE).

## Operating mode

- `devcoordinatord` (the DevCoordinator2 daemon) runs as root in a hardened
  systemd unit. It is the only component that changes system state.
- Repository code never runs as root or as the daemon identity. The daemon
  launches repository commands as the physical non-root caller via systemd
  transient units (`--uid`/`--gid` + explicit supplementary groups).
- Local API calls use a Unix socket. The kernel peer UID (`SO_PEERCRED`) is
  the physical caller identity; request bodies cannot assert identity.
  The socket is mode 0666: every local account is a full caller by owner
  decision (DC2-2026-08-24-OPEN-LOCAL-ACCESS); the client group remains
  only as the repository-access mechanism.
- No local per-repository or per-agent permissions are consulted. Any
  trusted local account may invoke any local command.
- The public edge authenticates users and enforces per-deployment grants.
  Public authority never derives from the local trust boundary.

## Docker: authoritative mode (since the 2026-08-25 cutover)

- No agent account is a member of the `docker` group
  (DC2-2026-08-22-DOCKER-MODE, executed 2026-08-25 on explicit owner
  approval). The root daemon is the Docker authority: unprivileged
  container work goes through DevCoordinator2's commands, and every
  container it creates is attributed to a repository, deployment or test,
  and caller.
- Containers that predate the cutover and were not created by the daemon
  remain visible and honestly classified (`observed-current` for adopted
  legacy stacks, `unmanaged` otherwise) until adopted through reviewed
  repository configuration or removed.
- Root/sudo held by the owner's accounts remains inside the single-owner
  trust boundary above; this section governs the unprivileged path, not
  root.

## Root daemon hardening obligations

Because the daemon is root and writes inside caller-writable repository
trees (`<repo>/.devcoordinator/`), it must:

- create and delete those paths with `O_NOFOLLOW`/dirfd-relative operations
  and refuse traversal outside the repository realpath;
- build every subprocess as an argv array, never a shell string; and
- treat all request content and repository configuration as untrusted input
  with strict validation.

## Review triggers

Review this file before: adding an account not controlled by the same
owner; making local accounts mutually distrusting; changing the repository
or client group model; placing credentials or private runtime state in the
checkout; exposing the daemon socket beyond the local trust boundary;
re-granting any account direct Docker access (reversing the authoritative
cutover); or exposing the repository through any network service.
Also review it before exposing Codex usage to viewers, returning per-user or
raw collector detail, adding an exporter or network collector, supporting a
Codex usage schema or taxonomy beyond the explicitly reviewed versions, or
configuring a usage source owned outside the confirmed same-owner boundary.
