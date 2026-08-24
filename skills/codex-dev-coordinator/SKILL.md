---
name: codex-dev-coordinator
description: Coordinate host-visible local development tests, deployments, services, ports, containers, PostgreSQL components, health, and runtime cleanup through the installed DevCoordinator2 CLI or MCP server. Use for shared runtime observation or mutation; do not use for ordinary source inspection, editing, Git work, formatting, or static checks.
---

# DevCoordinator2

Use the installed `devcoordinator2` client from the intended Git worktree.
The client and MCP server share one typed JSON contract; prefer their current
`--help` output over remembered command shapes.

```bash
devcoordinator2 --help
devcoordinator2 test --help
devcoordinator2 deployment --help
devcoordinator2 health --help
```

## Choose the product-owned surface

- Use `test start|status|output|stop` for repository tests.
- Use `deployment list|apply|status|start|stop|restart|rollback|logs|remove`
  for declared permanent or preview deployments.
- Use `health summary|repositories|containers` for host and ownership
  observation. Treat `unmanaged` as unknown; never infer ownership from a
  name, image, port, or path.
- Use `bug report|list|close` for the independent open-bug registry.
- Use `devcoordinator2 mcp` only as the configured STDIO MCP server.

Read the exact subcommand help before destructive or uncommon administration.
Require the typed result to prove the requested state; a submitted command is
not success. On a typed failure, follow its stated recovery and exact identity.
Do not bypass it with direct Docker, database, process, port, or systemd
mutation.

Keep secrets out of argv, ordinary environment metadata, results, and logs.
Use only the installed instance configuration and private credential files.

## Preserve the self-hosting boundary

Never use an installed DevCoordinator or DevCoordinator2 runtime to package,
install, deploy, roll back, or validate DevCoordinator2 itself. Follow the
DevCoordinator2 repository's own non-self-hosting installation and acceptance
workflow.
