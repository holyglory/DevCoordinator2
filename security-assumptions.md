# Security Assumptions

Last reviewed: 2026-08-22

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
- Runaway processes, containers, storage growth, stale work, malformed
  input, path escape, and lost replies are credible operational failures and
  are handled as such, not as security incidents.

## Operating mode

- `devcoordinatord` (the DevCoordinator2 daemon) runs as root in a hardened
  systemd unit. It is the only component that changes system state.
- Repository code never runs as root or as the daemon identity. The daemon
  launches repository commands as the physical non-root caller via systemd
  transient units (`--uid`/`--gid` + explicit supplementary groups).
- Local API calls use a Unix socket. The kernel peer UID (`SO_PEERCRED`) is
  the physical caller identity; request bodies cannot assert identity.
  Socket access is gated by a dedicated client group (named in instance
  configuration), socket mode 0660.
- No local per-repository or per-agent permissions are consulted. Any
  trusted local account may invoke any local command.
- The public edge authenticates users and enforces per-deployment grants.
  Public authority never derives from the local trust boundary.

## Docker: observational mode (current, explicit)

- Some agent accounts currently retain direct Docker socket access
  (enumerated in `instance/local-accounts.md`). DevCoordinator2 therefore
  runs in observational mode: containers it did not create are visible and
  honestly classified as `unmanaged/unknown`.
- The authoritative mode — only the daemon touches the Docker socket, agent
  accounts lose direct access — is a consequential host permission change.
  It happens only as an explicit reviewed cutover step during migration,
  never silently during development.

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
performing the Docker authoritative-mode cutover; or exposing the
repository through any network service.
