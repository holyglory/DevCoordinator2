---
name: codex-dev-coordinator
description: Coordinate host-visible local development tests, deployments, services, ports, containers, PostgreSQL components, health, and runtime cleanup through the installed DevCoordinator2 CLI or MCP server, and use its authoritative planning/completion ledger and decision history (tasks, releases, previews, decisions). Use for shared runtime observation or mutation and for all ledger/decision work; do not use for ordinary source inspection, editing, Git work, formatting, or static checks.
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
- Use `plan overview`, `task create|update|history`,
  `release create|deliver`, and `decision record|tail|search|summarize` for
  the authoritative planning ledger and decision history (below).
- Use `bug report|list|close` for the independent open-bug registry
  (coordinator defects only; product work items are ledger tasks).
- Use `devcoordinator2 mcp` only as the configured STDIO MCP server.

Read the exact subcommand help before destructive or uncommon administration.
Require the typed result to prove the requested state; a submitted command is
not success. On a typed failure, follow its stated recovery and exact identity.
Do not bypass it with direct Docker, database, process, port, or systemd
mutation.

Keep secrets out of argv, ordinary environment metadata, results, and logs.
Use only the installed instance configuration and private credential files.

## Plan, ledger, and decisions

The coordinator's database is the only completion ledger and decision
history — never a Markdown list, checklist, or chat memory. A daemon or
database error from these tools blocks the affected completion claim; there
is no file fallback.

- The moment you stub, fake, or skip anything, or notice something that can
  and should be improved, record it: `task_create` (kind `stub` or
  `improvement`), sized in estimated lines of code. Split large work into
  subtask trees. Keep statuses current as you work.
- Write titles, outcomes, and decision bodies for a non-technical manager
  ("Painting the button red"): what it means for the user of the product,
  never hashes, identifiers, file paths, or jargon — those belong only in
  `technical_note`. Report progress in chat the same way: plain outcomes and
  decision-relevant tradeoffs first.
- Check `plan_overview` before starting a task. When it — or any task
  result — shows a requested preview, honor it promptly: apply the
  deployment from the current work (dirty is expected), then
  `release_deliver` so the owner gets the URL or port. The owner's comments
  arrive as `user_feedback` tasks.
- Record consequential product choices with `decision_record`
  (aspect-tagged, management-facing body; `supersedes` when replacing one).
  Load context with `decision_tail`; `decision_search` before retrying an
  approach that may already have been tried and rejected. When any decision
  read reports `summary_due`, write and store the rolling summary via
  `decision_summarize` before continuing.

## Preserve the self-hosting boundary

Never use an installed DevCoordinator or DevCoordinator2 runtime to package,
install, deploy, roll back, or validate DevCoordinator2 itself. Follow the
DevCoordinator2 repository's own non-self-hosting installation and acceptance
workflow.
