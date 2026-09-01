# Full Repo Audit Skill

`full-repo-audit` audits an entire repository against the complete intended
product: semantic implementation, user journeys, feature set, UI elements,
tests, architecture, and operational quality. Every verified artifact-backed
audit produces a verified file and implementation-trace record, a prioritized
plan, and an exact reviewed database completion-ledger projection, including
an empty projection for a clean audit.

## Target

Use this skill when you want a repository-wide audit that checks:

- Architecture and product-flow risks.
- Every queued source file, manually reviewed in deterministic batches.
- Every promised feature and public/operational entry point decomposed into
  distinct responsibilities with unique `Contract ID`s, then traced from
  registration and inputs through real calculation/domain logic, data and
  integrations, permissions, failure/recovery, observable output, lifecycle,
  and verification.
- Intended journeys, features, routes, controls, states, handlers, persistence
  paths, permissions, and tests that should exist.
- UI controls, messages, routes, forms, menu items, and visible copy that imply missing or incorrect behavior.
- User journeys through the UI, including multi-journey relevance, route priority, decision-making information, information hierarchy, compact desktop/native/mobile fit, UI assumption status, rare/detail/debug content risk, readability, and test-mode availability.
- Visual journey testability when the repo has a rendered UI or visual tooling,
  including primary-journey hierarchy, activation continuation, theme/palette
  evidence, and changed-input-only screenshot review after automatic checks.
- TODO, stub, placeholder, console-only, mocked-as-real, dead-ended, or partially implemented behavior.
- Semantic gaps without markers: hard-coded substitutes for calculations,
  ignored inputs/configuration, fake success, incomplete plumbing, memory-only
  persistence, unregistered routes/jobs, production fixtures/mocks, and tests
  that prove shape but not outcomes.
- Reviewable semantic judgments: every responsibility records an authoritative
  or source-inferred basis, parsed or manual discovery provenance, and a
  counterfactual or invariance with typed test/runtime/source-only evidence.
  Source-only evidence cannot close stateful or external PASS claims.

The audited repository is read-only by default. Audit artifacts and an exact
ledger projection live outside it. Only an explicit request or applicable
project instruction to record findings authorizes a transactional import into
the software-owned database completion ledger; it never authorizes
implementation, commits, pushes, or other source changes. The workflow never
creates or updates `CompletionLedger.md`.

## How To Use

This exhaustive skill is explicit-only: ordinary review or gap-finding requests
do not activate it. Invoke it through the active runtime's native skill mechanism:

```text
Use $full-repo-audit to audit this repo.
```

Or name a specific repository:

```text
Use $full-repo-audit to audit /path/to/repo.
```

The skill requests an extra-high-effort lead audit. It calls that effort runtime-attested only when the runtime supplies immutable provenance; configured or self-reported effort remains `ledger-recorded-unverified`. File batches and journey checks use fresh isolated workers with runtime/user-selected effort or disclosed manual fallback when workers are unavailable. Every worker receives the complete generated prompt. The lead independently reopens every batch PASS anchor; it may work incrementally, but sampling cannot support full semantic coverage.

## What It Produces

The harness creates an audit output directory containing:

- `manifest.json`: source files, coverage units, batches, hashes, interface files, and coverage invariants.
- `audit_index.md`: dispatch guide for the lead agent.
- `batch_###.md`: prompts for low-effort file-audit workers.
- `journey_audit.md`: source-level user-journey worker prompt when interface files exist.
- `visual_journey_audit.md`: visual journey worker prompt when interface files exist.
- `lead_reconciliation.md`: required lead prompt for cross-file contract traces,
  atomic findings, and open questions.
- `effort_ledger.json`: lead-recorded worker/effort/fallback ledger.
- `visual_evidence.json`: hashes and metadata for real screenshot pairs/native
  captures, formal-verifier JSON, changed-review queue, and manual-review
  manifest.
- `excluded_files.json`: skipped files and scope-warning reasons.
- `reports/`: required returned worker reports plus verified
  `lead_reconciliation.md`.
- `logs/`: complete command stdout/stderr kept out of routine model context.
- `final-report.md`: complete lead synthesis and trace tables.
- `queue_complete.json`: queue-generation marker, not proof that the audit is finished.
- `verification_receipt.json`: written only after a passing stable verifier run; binds the manifest, exact report root, and every authorized report hash used by consolidation.
- `consolidated-findings.json` / `.md`: mechanically merged candidates.
- `completion_ledger_projection.json`: every candidate's lead-reviewed
  disposition and proposed active database issue.
- `completion-ledger-database-import.json`: digest-bound transactional import
  produced only after the exporter reruns the verifier over guarded
  manifest-owned reports, source files, effort/queue/exclusion records,
  prompts, and bound evidence artifacts and matches that canonical pass result
  to the receipt.

Workers write full reports directly to their prompt-declared paths and return
only compact filename/hash/byte/count receipts. The complete lead result belongs
in `final-report.md`. The chat response is only a compact outcome, finding
counts, verifier status, material caveats, and artifact index; it must not
duplicate worker reports or the full trace table.
If no explicit user journeys exist, the audit should ask for confirmation or
label UI/journey coverage as assumption-based rather than claiming the interface
is user-friendly.

## Direct Harness Usage

The skill is normally invoked by an agent runtime, but the bundled scripts can be used directly.

Run harness self-tests:

```bash
cd /path/to/full-repo-audit
python3 scripts/self_test.py
```

Those deterministic tests prove queue/verifier contracts, not whether an agent
understands arbitrary domain behavior. The separate `evals/marker-free/` suite
can score six isolated fresh-agent runs against marker-free gaps and intentional
lookalike controls. Its own self-test uses synthesized oracle-derived responses
and proves only the evaluation infrastructure. Follow its `README.md`; do not
report either self-test as semantic-agent recall evidence.

Generate an audit queue:

```bash
cd /path/to/full-repo-audit
python3 scripts/build_audit_batches.py --repo /path/to/repo --out /tmp/full-repo-audit-run
```

Before verification, complete the generated `lead_reconciliation.md` prompt and
save it as `/tmp/full-repo-audit-run/reports/lead_reconciliation.md`. Its
`Worker` is exactly `lead_reconciliation`; its cross-file table cites the
responsibility-level `Contract ID`s and all nine implementation trace labels;
its findings are atomic. Batch rows use deterministic `batch_###:C###` IDs;
cross-file lead-reconciliation rows use `lead:C###`, map every batch ID exactly
once, derive their result from the nine statuses, and never hide a mapped
`GAP` or `BLOCKED`.

Verify saved reports:

```bash
cd /path/to/full-repo-audit
python3 scripts/verify_audit_results.py \
  --manifest /tmp/full-repo-audit-run/manifest.json \
  --reports /tmp/full-repo-audit-run/reports \
  --receipt-out /tmp/full-repo-audit-run/verification_receipt.json
```

After verifier success, create and review the exact ledger projection:

```bash
python3 scripts/_vendor/full_repo_harness/merge_findings.py \
  --reports /tmp/full-repo-audit-run/reports \
  --manifest /tmp/full-repo-audit-run/manifest.json \
  --json-out /tmp/full-repo-audit-run/consolidated-findings.json \
  --markdown-out /tmp/full-repo-audit-run/consolidated-findings.md \
  --ledger-projection-out /tmp/full-repo-audit-run/completion_ledger_projection.json
```

The manifest restricts consolidation to the exact verified report allowlist and
hashes. Findings merge only when all immutable fields match.

Dispose every candidate as `confirmed`, `duplicate`, `hypothesis`, `invalid`,
or `out_of_scope`, with a concrete reason. If one candidate contains multiple
independently completable obligations, reject and reissue its owning verified
report with atomic findings, refresh lead reconciliation, then rerun verification
and projection; do not add or split projection candidates manually. When every
candidate is disposed, set top-level `review_status` to `complete`, including
for a clean empty projection.

When ledger mutation is authorized by an explicit user request or applicable
project instruction, record each confirmed projection issue in
DevCoordinator2's completion ledger with `task_create` (plain-language
outcome and impact; paths and the projection's stable issue ID in
`technical_note`; kind `stub` or `improvement`; sized in estimated lines of
code), updating an existing task when its stable ID is already recorded.

Useful builder options:

- `--out <dir>`: write artifacts to a specific harness-owned directory.
- `--include-generated`: include generated/build directories.
- `--include-vendor`: include vendor/dependency directories.
- `--include-env`: include real `.env` files intentionally.
- `--include-assets`: include binary UI assets such as logos, screenshots, and fonts.
- `--include-file <repo-relative-path>`: force a specific file into coverage.
- `--include-glob <glob>`: force matching files into coverage.
- `--batch-size <n>`: limit files per batch.
- `--max-batch-bytes <n>`: split large files into line or byte range units.

## Coverage Rules

Do not treat queue generation as audit completion. A run is complete only after:

1. Every generated batch prompt has a saved report in `reports/batch_###.md`.
2. Every coverage unit and high-confidence named source definition has concrete
   source-backed `Implementation Inventory` coverage; every named definition
   has its own row; and every distinct responsibility has a globally unique
   `batch_###:C###` ID. Each row records an enumerated basis, parsed or manual
   discovery bound to an assigned-unit anchor, typed verification evidence, and
   one counterfactual or invariance. Stateful/external PASS rows use test or
   runtime evidence. Every batch ID maps exactly once into a unique `lead:C###`
   trace whose statuses agree with its Result, and the lead independently
   reopens every PASS anchor with the same evidence discipline. Symbol presence,
   type/shape checks, generic prose, and PASS sampling are not sufficient. Every
   semantic `GAP` or `BLOCKED` maps one-to-one to an independently closable
   finding that cites exactly that ID.
3. Required journey reports exist when interface-relevant files are queued.
4. `effort_ledger.json` records completed lead, worker, journey, and fallback status.
5. Every high-risk manifest file has a direct lead-review row.
6. Applicable visual reports bind real `evidence:<id>` artifacts; web UI binds
   the formal report, initial/full-page pairs, changed-review queue, and
   finalized decisions, with carried unchanged images explicitly not reopened.
7. `reports/lead_reconciliation.md` supplies the manifest-declared, verified
   cross-file trace and atomic lead findings.
8. A passing stable verifier run writes `verification_receipt.json`; manifest-mode consolidation consumes its exact report hashes.
9. `verify_audit_results.py` returns `ok: true`.
10. Before plan and again before apply, the ledger updater reruns the verifier
    over its guarded input closure and matches the canonical pass result to the
    receipt; a structurally valid or replayed receipt alone is insufficient.
11. The lead disposes every atomic consolidated candidate and sets projection
   `review_status` to `complete` before any ledger update.

If a run is interrupted, trust the manifest and verifier over stale
`effort_ledger.json` notes. Rerun missing batch, journey, visual, or lead
reconciliation prompts, save the exact report filenames, update
`effort_ledger.json`, and verify again.

## Notes

`SKILL.md` is the authoritative operational contract. This README is a shorter orientation guide for humans who want to understand the skill and its bundled harness quickly.
