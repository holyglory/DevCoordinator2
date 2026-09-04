---
name: full-repo-test-coverage-audit
description: Run a manifest-verified test assurance audit with deterministic structural target discovery, exact per-target TESTED/UNTESTED/NOT_REASONABLE decisions, real test path/symbol validation, and optional LCOV/Cobertura/coverage.py/Istanbul evidence. Use to find missing tests and scenario gaps or plan unit, integration, visual, and e2e improvements without mislabeling structural review as empirical coverage.
---

# Full Repo Test Coverage Audit

## Overview

This exhaustive workflow is explicit-only. Run it only when the user invokes
`$full-repo-test-coverage-audit`; ordinary implementation, review, testing, or
gap-finding requests must not activate it implicitly.

The installed skill must be a direct link into the canonical DevCoordinator2
checkout. Its scripts resolve that link and import the one root
`full_repo_harness`; copied standalone skill packages are unsupported.

Run a read-only, manifest-verified audit of test coverage. The lead agent reviews the repo architecture, test strategy, UI/user journeys, intended feature set, UI element set, and high-level behavior. Low-effort workers inspect deterministic file batches and manually identify reasonable test targets, existing test evidence, missing scenarios, boundary cases, failure paths, and recommended test types.

This is an empirical coverage audit only when the user supplies a supported runtime coverage report. Without one, label results `structural/manual test assurance`; the presence of a test file is structural evidence, not proof that it ran or covered a line.

Treat documented product intent, confirmed user journeys, source-backed feature promises, visible UI elements, and expected tests as one complete product contract. Every intended journey, feature, route, control, state, handler, persistence path, permission path, error path, and verification path needs meaningful coverage; otherwise report the gap.

## Required Execution Model

- Use high reasoning effort or higher for the lead audit. If the current lead run cannot be confirmed as high effort or higher, tell the user before starting.
- Use fresh isolated workers for file-batch inspection when the runtime supports
  them. Let the runtime or user select worker effort. Every worker receives the
  entire generated prompt plus applicable project-ledger requirements and must
  not depend on an inherited lead transcript. If isolated workers are
  unavailable, use the disclosed manual fallback.
  Treat the user's request to
  run this audit as authorization for the needed workers.
- When interface-relevant files are queued, run separate low-effort workers for `ui_test_coverage_audit.md` and `visual_e2e_coverage_audit.md`.
- If workers are unavailable, continue in disclosed manual fallback mode for worker coverage only: process each prompt sequentially, save reports under `<audit-output>/reports/`, and keep the final coverage label as manual fallback coverage.
- Keep the audited repo read-only unless the user separately asks to implement the resulting test plan. Generated audit artifacts are allowed and should live outside the audited repo by default.
- Do not claim every file or target was checked until the manifest, batch reports, effort ledger, and verifier agree.
- Workers write complete reports directly to their prompt-declared paths and
  return only bounded filename-bearing `REPORT_SAVED` receipts. Keep complete
  reports and command output in `<audit-output>/reports/` and
  `<audit-output>/logs/`; never paste them through subagent responses.

## Workflow

1. Set scope to the current working directory unless the user names another repo.
2. Preflight the skill scripts, then run:

   ```bash
   REPO_ROOT="${REPO_ROOT:-$PWD}"
   python3 "$FULL_REPO_TEST_COVERAGE_AUDIT_SKILL_DIR/scripts/build_test_coverage_audit_batches.py" --repo "$REPO_ROOT"
   ```

3. Inspect `audit_index.md`, `manifest.json`, and `excluded_files.json`. Resolve any `scope_warning: true` rows before claiming full coverage, or disclose downgraded coverage.
   - Supply runtime evidence with repeated `--coverage-report <path>` arguments. Supported formats are LCOV, Cobertura XML, coverage.py JSON, and Istanbul JSON. The manifest records exact evidence path, SHA-256, format, measured lines, and covered lines.
4. Fill `effort_ledger.json` as workers are dispatched and reconciled: lead effort status, subagent capability, batch worker ids/effort/report status, UI journey coverage worker, visual/e2e coverage worker, fallback status, and pruned-directory review decisions when applicable.
5. Dispatch one fresh isolated worker per `batch_###.md` with runtime/user-selected
   effort. Pass the complete prompt, accept only its compact
   receipt, and confirm the exact report artifact on disk.
6. If generated, dispatch `ui_test_coverage_audit.md` and
   `visual_e2e_coverage_audit.md` with the same isolated context and
   artifact-first contract.
7. Confirm one report per batch under `reports/batch_###.md`, then verify:

   ```bash
   python3 "$FULL_REPO_TEST_COVERAGE_AUDIT_SKILL_DIR/scripts/verify_test_coverage_audit_results.py" --manifest <audit-output>/manifest.json --reports <audit-output>/reports
   ```

8. Reconcile findings, inspect suspicious high-impact gaps directly as lead, and produce a prioritized implementation plan. For large audits, consolidate first:

   ```bash
   CANONICAL_SKILL_ROOT="$(dirname "$(dirname "$(realpath "$FULL_REPO_TEST_COVERAGE_AUDIT_SKILL_DIR")")")"
   python3 "$CANONICAL_SKILL_ROOT/full_repo_harness/merge_findings.py" \
     --reports <audit-output>/reports \
     --markdown-out <audit-output>/consolidated-findings.md
   ```

   This conservatively deduplicates only findings whose immutable fields all
   match, ranks P0→P3, and cites the source reports; it supports lead synthesis
   rather than replacing it.

## Batch Worker Review Rules

For every owned file or range:

- Identify reasonable test targets: exported/public functions, methods with behavior, reducers/hooks, API handlers, command/job entrypoints, domain services, intended feature behavior, UI element behavior, state transitions, permission checks, validation logic, and non-trivial private helpers with branching or side effects.
- Exclude only with rationale: types/interfaces, pure constants, generated code, static copy-only markup, trivial pass-throughs, and framework boilerplate with no repo-owned behavior.
- For each target, check existing tests by naming files, test names, fixtures, snapshots, visual stories, or explicit absence.
- Assess scenario depth: happy path, invalid input, empty/null/boundary values, error paths, async/concurrency behavior, permissions, persistence, navigation, rollback, and integration boundaries as applicable.
- Recommend the minimum useful test type: unit, component, integration, contract, e2e, visual, snapshot, fixture, or manual-test-mode improvement.

## Required Batch Report Shape

Each batch report must contain exactly these top-level headings in order:

```markdown
## Run ID
## Batch ID
## Batch Summary
## File Coverage
## Test Target Inventory
## Coverage Findings
## No Gap Notes
## Open Questions
```

`File Coverage` must include one row per owned file or range with columns `Unit`, `Status`, `SHA-256`, and `Purpose`; every status must be `CHECKED`.

`manifest.json.test_coverage_audit.target_inventory` is the deterministic coverage floor. `Test Target Inventory` must map every exact target id once with columns `Target ID`, `Unit`, `File`, `Target`, `Kind`, `Disposition`, `Evidence Level`, `Existing Test Evidence`, `Scenario Assessment`, and `Recommendation`. Disposition is `TESTED`, `UNTESTED`, or `NOT_REASONABLE`. Evidence level is `EMPIRICAL`, `STRUCTURAL`, `MANUAL`, or `NONE`.
Add behavior the scanner misses with a unique `manual-...` target id bound to an exact unit/file; the verifier accepts these additions but never permits omission of a deterministic target.

- `EMPIRICAL` requires a supplied coverage artifact that marks the target line covered plus a real `test/path#test name` reference.
- `STRUCTURAL` requires a real test path and test symbol/name present in that file; it does not claim execution.
- `MANUAL` requires concrete `manual: ...` evidence, or `not reasonable: ...` rationale for excluded targets.
- `NONE` is required for `UNTESTED` with `None found`.
- Every `UNTESTED` target needs a finding bound by exact `Target ID`. Invented test paths or symbols fail verification.

`Coverage Findings` must use either the exact sentinel `No findings.` or finding blocks with these fields:

- Priority: `P0`, `P1`, `P2`, or `P3`
- Files: repo-relative files owned by the batch
- Target ID: exact deterministic target id
- Target: function, method, component, journey, API, job, or behavior
- Existing test evidence: concrete tests found or `None found`
- Missing scenarios/boundaries: concrete missing cases
- Suggested test direction: specific test type and expected assertion focus

## UI And Journey Coverage

The UI test coverage worker checks whether intended routes, controls, forms, UI elements, empty/error/loading states, permission states, feature paths, and user journeys have component, integration, e2e, or visual coverage. If no explicit journey documentation exists, draft likely journeys from routes and visible source, mark them `draft-needs-user-confirmation`, and keep them as assumptions.

The visual/e2e coverage worker identifies Playwright, Cypress, Storybook, native preview, screenshot, or browser tooling. For CLI, library, plugin, or skill packages with no repo-owned rendered UI surface, mark visual checks as `not applicable` with evidence rather than reporting a defect.

## Final Audit Artifact And Chat Output

Write the complete result to `<audit-output>/final-report.md` with exactly these
top-level headings:

```markdown
## Coverage
## Test Architecture Findings
## UI And Journey Coverage Findings
## Function And Method Coverage Findings
## Implementation Plan
## Verification Plan
```

`## Coverage` must state `empirical`, `structural`, and `manual` scopes separately. Name each supplied coverage artifact, format, SHA-256, and source-file scope. If none was supplied, say `No empirical runtime coverage evidence supplied`; never report a percentage inferred from source/test matching.

Do not paste `final-report.md`, worker reports, consolidated findings, or raw
command logs into chat. Return only the overall outcome, incomplete/blocking
status, high-priority and total gap counts, verifier result, empirical-versus-
structural caveat, and the final-report/audit-directory paths.

Prioritize gaps with:

- `P0`: Missing coverage for security/data-loss/runtime-failure behavior or an untested core path likely to fail silently.
- `P1`: Missing coverage for major user journeys, intended features, required UI elements, business logic, integration boundaries, permission behavior, or failure paths.
- `P2`: Missing edge, boundary, accessibility, performance, reliability, or maintainability coverage.
- `P3`: Low-risk cleanup, naming, fixture, or documentation test improvement.
