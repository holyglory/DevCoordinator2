# Rust execution plane

This workspace is the reusable governed-check execution foundation for the
current Python control plane and the planned Rust daemon. It requires Rust
1.85 or newer and accepts executor plan schema 2 only.

Build the release binary from the canonical checkout:

```text
cargo build --release --locked -p devcoordinator2-executor
```

Run or validate a JSON/TOML plan:

```text
cargo run -p devcoordinator2-executor -- run PLAN.json
cargo run -p devcoordinator2-executor -- run-local PLAN.json
cargo run -p devcoordinator2-executor -- validate PLAN.json
```

`run` requires the host broker in `DEVCOORDINATOR_CAPACITY_SOCKET` and fails
closed when it is absent. Explicit `run-local` is the direct self-validation
mode and admits all dependency-ready leaves locally.
The control plane pre-creates disposable `current_dir` and stable `log_dir`;
the executor resolves all plan paths before writing and refuses a missing or
escaping run directory. This
accepts platform path aliases such as macOS `/var` → `/private/var` without
weakening containment.
Repository commands remain argv arrays; the executor never invokes a shell.

The Python control plane can request bounded content-free evidence without
retaining a second hashing implementation:

```text
devcoordinator2-executor source-digest --worktree /absolute/worktree
devcoordinator2-executor receipts-match --worktree /absolute/worktree --receipts receipts.json
```

Normal commands print one bounded JSON receipt. Every direct check, discovery
step, and expanded case writes byte-complete private streams and indexes below
the exact stable `log_dir` named by the plan. There is no aggregate output copy
or per-stream storage cap. `log-query` and `log-prune` are JSON-stdin internal
surfaces used by the authenticated control plane; they expose bounded logical
references rather than caller-supplied paths.
