---
name: dev-coordinator
description: Coordinate host-visible local development tests and governed check graphs, deployments, services, ports, containers, PostgreSQL components, health, and runtime cleanup through the installed DevCoordinator2 CLI or MCP server, and use its authoritative planning/completion ledger and decision history. Use for shared runtime observation or mutation and for all ledger/decision work; do not use for ordinary source inspection, editing, Git work, formatting, or static checks.
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

- Use `test start|retry|status|output|stop|event|list` for repository tests.
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

## Governed tests

Before a consequential complete run, read the current `test --help`, inspect
the named test declaration in `.devcoordinator.toml`, and query `test list`.
Do not start a duplicate for a worktree that already has the intended run.

Distinguish the execution model before describing or starting it:

- A graph test declares named checks. Every check whose `after` and `requires`
  dependencies are satisfied starts concurrently; `requires` also requires a
  successful predecessor. Process exit or the check's exact `test event`
  advances the graph. `timeout_seconds` is only the outer runaway watchdog.
- A legacy test is one opaque command, even when that command internally runs
  many builds, locales, browser sessions, or formal phases. DevCoordinator
  cannot parallelize, time, select, or retry those hidden phases. State this
  limitation when it materially affects a requested long run. Do not infer a
  defect from timeout length alone or attempt to parse shell source as a graph.
- When evidence or known structure shows that a legacy target serializes
  independent work, create or reuse one specifically scoped graph-migration
  task. Do not add worker budgets, fixed concurrency counts, or timeout
  heuristics; express real ordering and isolation needs as graph dependencies.

The process is owned by its systemd unit, not by the agent that started or
observes it. If an observer exits, query `test status` or `test list` and read a
bounded `test output` tail before deciding the workload is stale. A `running`
summary plus a live unit/output growth is active work; do not cancel, restart,
or submit a duplicate merely because the original agent disappeared.

Use status as the compact authority: graph runs expose per-check state,
durations, bounded failure index, proof kind, and output references without log
text. Use `test output --check <name>` only for the check under diagnosis. Let a
finite complete pass collect every safe failure and cleanup result before batch
repair; begin independent read-only diagnosis without modifying its source or
artifacts.

`test start --check <name>` and `test retry --run-id <run> --check <name>` are
diagnostic shortcuts. Retry only after the originating complete run finishes
and only while its source, configuration, prerequisites, and declared artifact
receipts still match. Neither selection nor retry is release proof; readiness
still requires one fresh complete passing graph.

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
- Treat every non-empty `elaboration_requests` list in a planning, task,
  release, or decision result as an owner request that must not be silently
  skipped. Read each named task with `task_history`, rewrite its title and/or
  outcome in short everyday language that explains the user-visible result,
  and set `elaboration_needed: false` in that same `task_update`. The daemon
  rejects clearing the request without changed owner-facing wording. Keep the
  request open when you cannot yet make the wording genuinely clearer, and do
  not claim the related work complete while its request remains outstanding.
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
