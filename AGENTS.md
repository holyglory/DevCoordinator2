# DevCoordinator2 Repository Instructions

These instructions apply to every coding agent working in this repository.
The universal cross-repository policy is `reference/universal/AGENTS.md`.

## Canonical checkout and worktrees

- `/home/DevCoordinator2` is the one live source for the daemon, edge, command
  line, six agent skills, and universal policy.
- Keep that checkout on a clean `main` exactly fast-forwarded to
  `origin/main`. Fetch before consequential repository-wide work.
- Develop in a separate linked worktree and merge through Git. Never stash,
  reset, clean, rebase, or develop directly in the live checkout.
- A dirty, stale, non-main, or non-fast-forward live checkout blocks service
  restart and readiness.

## Authoritative project context

- Read `security-assumptions.md`, relevant requirements and decisions, and
  every applicable file under `UserIssueLedgers/` before consequential work.
- DevCoordinator2's database is the only completion ledger and decision
  history. Use `devcoordinator2 plan`, `task`, and `decision`; never create a
  Markdown completion ledger.
- Use the installed Coordinator only for other repositories' governed runtime
  work and for ledger/decision operations. Never use it to test, install,
  deploy, roll back, or validate DevCoordinator2 itself.

## Skill and policy ownership

- This repository is the only writable source for these six skills:
  `dev-coordinator`, `formal-web-ui-verification`, `full-repo-audit`,
  `full-repo-test-coverage-audit`, `ui-implementation-audit`, and
  `user-journey-docs-audit`.
- `reference/universal/AGENTS.md` is the only universal policy source. Root
  `AGENTS.md` is repository policy and must not be installed globally.
- Installed skill and policy entries are direct absolute symlinks into this
  checkout. Preserve unrelated runtime entries and use the reviewed link
  managers under `scripts/skills/`.
- Shared policies, skill contracts, generated prompts, and generic
  documentation remain runtime-neutral. Runtime names belong only in factual
  installation adapters, provider metadata, product integrations, or history.

## Skill development

- Reproduce a changed detector or workflow gap before editing it.
- Keep each `SKILL.md` authoritative and mirror enforceable behavior in its
  self-tests.
- Detector changes need realistic must-catch cases and false-positive guards.
- Keep `full_repo_harness/` synchronized with all three vendored copies using
  `scripts/skills/sync_vendored_harness.py`.
- Do not edit installed skill copies; change this repository and verify the
  direct links.

## Validation

- Run product lint/tests directly from this repository.
- Run `python3 scripts/skills/validate.py` for the six-skill, policy, ownership,
  standalone-package, and browser matrix.
- Keep complete verbose output in cold logs and report bounded failure indexes.
- Finish finite diagnostic passes after ordinary failures, batch fixes, then
  rerun the complete relevant pass.

## Security boundary

- The live checkout is writable only by mutually trusted accounts controlled
  by the same owner. Its Python source executes as root when the daemon starts.
- Review `security-assumptions.md` before changing this trust model, adding a
  writer, or making repository writers mutually distrusting.
- Secrets, credentials, private instance values, and live evidence never enter
  source, prompts, normal results, or logs.
