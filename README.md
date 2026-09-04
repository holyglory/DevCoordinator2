# DevCoordinator2

DevCoordinator2 is the server-wide authority for governed development tests,
deployments, health, planning work, and agent-facing operational coordination.
This repository is also the canonical source for six reusable agent skills and
one universal agent policy.

## Canonical agent assets

- `skills/dev-coordinator`: Coordinator CLI, MCP, planning, and runtime usage.
- `skills/formal-web-ui-verification`: deterministic rendered Web verification.
- `skills/full-repo-audit`: exhaustive implementation and contract audit.
- `skills/full-repo-test-coverage-audit`: structural and empirical test audit.
- `skills/ui-implementation-audit`: explicit implemented-UI audit.
- `skills/user-journey-docs-audit`: journey-documentation readiness audit.
- `reference/universal/AGENTS.md`: runtime-neutral universal agent policy.

The five audit/verification skills use the portable Rust tooling package. The
Coordinator skill is checked against this repository's Rust CLI and MCP
contracts.

## One live source checkout

`/home/DevCoordinator2` is the live source for the daemon, edge, command line,
skills, and policy. It remains a clean `main` fast-forwarded to `origin/main`.
All development happens in linked worktrees.

After a change is merged and validated:

1. fetch `origin` in the live checkout;
2. fast-forward `main` only;
3. verify the checkout is clean and equals `origin/main`;
4. drain active tests and restart services when runtime code or schema changed;
5. verify CLI, daemon, edge, database, skills, and policy.

Rollback is a new revert commit merged to `main`, followed by the same
fast-forward and restart sequence. Do not rewrite the live branch.

## Installation

The repository-owned installer configures systemd, the CLI shim, private
instance files, and direct agent links. It does not copy source into an
immutable release directory.

Use the reviewed managers for explicit runtime roots and policy targets:

```bash
devcoordinator2-tooling skills links plan \
  --repo-root /home/DevCoordinator2 \
  --target-root /absolute/runtime/skills

devcoordinator2-tooling skills policy plan \
  --repo-root /home/DevCoordinator2 \
  --transaction-dir /absolute/private/transaction \
  --codex-target /absolute/runtime/AGENTS.md
```

Apply only the reviewed plan using its required private transaction and digest.
Installed entries are direct absolute links to this checkout. Unrelated skills
and runtime files are preserved.

## Development and validation

Product checks:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets \
  --features devcoordinator2-tooling/selftest-fixtures -- -D warnings
cargo test --locked --workspace \
  --features devcoordinator2-tooling/selftest-fixtures -- --test-threads=1
cargo build --locked --release \
  --package devcoordinator2-control \
  --package devcoordinator2-tooling \
  --package devcoordinator2-executor \
  --features devcoordinator2-tooling/selftest-fixtures
node --test edge/test/edge.test.mjs
node console/verify.mjs
```

Complete agent-skill and policy gate:

```bash
npm ci --ignore-scripts --prefix ci/playwright
target/release/devcoordinator2-tooling skills validate run \
  --root "$PWD" \
  --temp-root /absolute/external/temp/devcoordinator2-skill-validation
```

The skill gate verifies the exact six-skill inventory, shared policy,
runtime-neutral contracts, issue ledgers, ownership boundary, Rust harness
ownership, link-manager rollback, public artifacts, real browser fixtures, and
all six skill packages. It rejects executable Python and permits only the seven
explicit inert cross-language audit fixtures.

## Imported source provenance

The reusable agent assets were imported as a current-tree snapshot from the
retired Holy Skills repository. See `docs/holy-skills-snapshot.md`. Its Git
history remains available in the archived source repository; it is not merged
into this repository's ancestry.
