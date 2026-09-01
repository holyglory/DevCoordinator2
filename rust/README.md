# Rust execution plane

This workspace is the reusable governed-check execution foundation for the
current Python control plane and the planned Rust daemon. It requires Rust
1.85 or newer and accepts executor plan schema 2 only.

Validate a JSON or TOML plan without running it:

```text
cargo run -p devcoordinator2-executor -- validate PLAN.json
```

Repository commands remain argv arrays; the executor never invokes a shell.
