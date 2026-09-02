---
name: ui-implementation-audit
description: Run an explicit exhaustive audit of an existing substantive product UI against product journeys, mockups, source wiring, rendered evidence, and tests. Use only through the active runtime's explicit skill-invocation mechanism after manually confirming at least one repo-owned executable screen/component/view. Do not use for ordinary implementation or review, pre-implementation plans, mockups, screenshots, stories/tests, styles/assets alone, scaffolding, or backend-only repositories.
---

# UI Implementation Audit

## Purpose And Boundary

This is an explicit-only, read-only assurance workflow. Invocation authorizes
the audit, its isolated workers, and external audit artifacts—not product
implementation or unrelated repository changes.

The installed skill must be a direct link into the canonical DevCoordinator2
checkout. Its scripts resolve that link and import the one root
`full_repo_harness`; copied standalone skill packages are unsupported.

Use it to determine whether an implemented product UI is complete and faithful
to its journeys and design target:

- Are all intended screens, states, controls, messages, and journeys present?
- Do visible actions have source wiring references for handlers, navigation,
  APIs, permissions, persistence, and tests?
- Does the rendered product support intended decisions and match relevant
  mockups across its declared platforms?
- Are missing behavior, evidence, and test paths converted into a prioritized
  implementation plan?

`formal-web-ui-verification` is the deterministic browser engine, not a second
implementation audit. It owns declared web route/state/viewport execution,
geometry, journey hierarchy, continuation, WCAG contrast, theme metrics,
screenshot pairs, and changed-review selection. This skill consumes that
structured evidence for web surfaces and adds discovery, source review,
mockup/journey judgment, native UI review, subjective visual judgment, and
cross-surface synthesis.

## Applicability Gate

Before self-tests, queue creation, or workers, manually identify a substantive
repo-owned executable product UI source file and pass it through
`--implemented-ui-file`. Imports, empty shells, route/provider mounts,
untouched starters, styles, assets, prototypes, mockups, stories, fixtures,
tests, and screenshots are insufficient.

A partial implementation qualifies once one real screen/component/view exists;
missing planned surfaces remain findings. A backend-only or pre-implementation
repository is not applicable, and the eligibility preflight exits `3` without
creating artifacts.

For an unrecognized real UI toolkit, use
`--implemented-ui-override PATH UI-KIND SOURCE-ANCHOR` only after manual
inspection. The verifier rechecks that the named source defines the anchor and
uses the named UI kind.

## Required Inputs

For a full audit, declare `--ui-platform web`, `native`, or `hybrid`.

- `web`: desktop and narrow/mobile rendered evidence plus a manifest-bound
  formal Web UI config.
- `native`: at least one native screenshot/snapshot; formal Web UI evidence is
  not applicable.
- `hybrid`: both the web formal-evidence chain and native captures.

Pass the existing project-owned formal verifier config with `--formal-config`
for web/hybrid. The builder records its path, size, and SHA-256. When it is
missing, the audit may continue, but formal coverage stays `BLOCKED` with a
finding; a worker must not invent an unbound replacement.

Use `--mockup` and `--journey-file` to force known design/requirement evidence
when discovery would miss it.

## Worker Model

Use fresh isolated workers when available and pass the complete generated
prompt plus applicable project-ledger requirements. Do not prescribe or
validate a worker reasoning-effort level; use the runtime/user-selected
default and do not rely on an inherited lead transcript.

Workers write complete reports to their prompt-declared paths and return only a
bounded filename-bearing `REPORT_SAVED` receipt. If workers cannot be spawned,
the lead may execute the same bounded prompts through the documented manual
fallback and records that provenance.

Source workers own only their deterministic interface-source units. They:

- inventory visible elements, states, responsive rules, and requirement
  alignment;
- record handler/API/permission/persistence/test `path#symbol` references or
  `missing`/reasoned `not-applicable`;
- report source-backed gaps.

A source reference proves only that its anchor exists. It does not prove an
observable outcome, integration, persistence, or success. Completion claims for
those behaviors require real runtime or test evidence.

One visual worker owns the journey decision model, rendered usability,
mockup comparison, interaction checklist, and visual findings. Source workers
do not duplicate those rendered judgments.

## Workflow

1. **Confirm eligibility**
   - Inspect and name at least one substantive executable UI source.
   - Run the builder with `--eligibility-only`. Exit `3` means not applicable.

2. **Build the audit queue**
   - Run the skill self-test unless validation commands are forbidden.
   - Generate the queue with implemented UI evidence, declared platform, and
     formal config when applicable.
   - Inspect `manifest.json`, `audit_index.md`, and `excluded_files.json`;
     resolve every scope warning before claiming coverage.

3. **Dispatch source workers**
   - Process every generated `batch_###.md` in a fresh context or documented
     manual fallback.
   - Accept only its bounded receipt and confirm the report exists.

4. **Run visual and formal evidence**
   - Read journey requirements and mockups before visual judgment.
   - For web/hybrid, run `formal-web-ui-verification` only with the
     manifest-bound config. Finish it and other automatic tests before opening
     review images.
   - Review only entries in `review-queue.json`; never reopen carried unchanged
     screenshots. Finalize decisions with `formal_web_ui_review.py`.
   - Import the completed formal bundle into `visual_evidence.json` using
     `scripts/import_formal_web_evidence.py`; do not transcribe its screenshot,
     journey-evidence, queue, or review records manually.
   - For native/hybrid, register real `native-snapshot` evidence.
   - Compare rendered results against journeys and mockups. Missing mockups are
     labelled `mockup target missing`, not silently replaced by taste.

5. **Complete visual review**
   - Build one journey decision model per important surface.
   - Check web desktop/mobile and/or native surfaces according to the declared
     platform.
   - Verify decision-driving content, secondary-detail access, responsive fit,
     typography, imagery, palette quality, state coverage, and accessibility.
   - Complete these interaction labels as `pass`, `gap`, `blocked`, or
     `not applicable` with evidence: `badge-detail`, `row-hit-target`,
     `navigation-cursor`, `transient-disclosure`, `disclosure-scrollbar`,
     `icon-meaning`, `stable-expansion-width`, `hover-copy`, `status-summary`,
     `message-metadata`.

6. **Synthesize the final report**
   - Deduplicate source and visual findings.
   - Separate confirmed gaps from assumptions and external blockers.
   - Prioritize by journey impact, missing behavior, accessibility, visual
     mismatch, implementation risk, and dependency order.
   - Write `final-report.md` before running the result verifier.

7. **Verify completion**
   - Run `verify_ui_implementation_audit_results.py` against the manifest and
     reports directory.
   - The verifier checks source/unit coverage, current hashes, worker status,
     formal/native evidence, final-report structure, interaction labels, and
     evidence references.
   - `ok: true` means the audit artifact set is internally complete. It does not
     mean the product UI is ready; the final report may truthfully remain
     `GAP`/`BLOCKED` until its findings are implemented and retested.

## Commands

Eligibility only:

```bash
python3 "$UI_IMPLEMENTATION_AUDIT_SKILL_DIR/scripts/build_ui_implementation_audit_batches.py" \
  --repo "$REPO_ROOT" \
  --implemented-ui-file src/App.tsx \
  --eligibility-only
```

Web audit queue:

```bash
python3 "$UI_IMPLEMENTATION_AUDIT_SKILL_DIR/scripts/build_ui_implementation_audit_batches.py" \
  --repo "$REPO_ROOT" \
  --implemented-ui-file src/App.tsx \
  --ui-platform web \
  --formal-config formal-web-ui.json
```

Import completed formal evidence:

```bash
python3 "$UI_IMPLEMENTATION_AUDIT_SKILL_DIR/scripts/import_formal_web_evidence.py" \
  --audit-root <audit-output> \
  --run-id <audit-run-id> \
  --formal-report <audit-output>/artifacts/report.json \
  --journey-evidence <audit-output>/artifacts/journey-evidence.json \
  --review-queue <audit-output>/artifacts/review-queue.json \
  --manual-review <audit-output>/artifacts/manual-review.json
```

Verify the complete audit:

```bash
python3 "$UI_IMPLEMENTATION_AUDIT_SKILL_DIR/scripts/verify_ui_implementation_audit_results.py" \
  --manifest <audit-output>/manifest.json \
  --reports <audit-output>/reports
```

## Final Report

`final-report.md` contains exactly these top-level sections:

```markdown
## Coverage
## Mockup And Requirement Inputs
## Journey Decision Model
## Rendered Journey Usability Findings
## Visual Audit Findings
## Source Implementation Findings
## Journey And Responsive Findings
## Accessibility And Interaction Findings
## Implementation Plan
## Verification Plan
```

Every section is non-empty. `Coverage` names the run id and declared platform.
The interaction section contains all ten labels. Web/hybrid reports cite the
imported formal report, review queue, manual-review manifest, and relevant
screenshots; native/hybrid reports cite native snapshots. The verification plan
names runtime/test proof or a concrete blocker/non-applicability and never
presents source references as outcome proof.

## Completion Rules

- Keep the audited repository read-only; write generated audit artifacts
  outside it by default.
- Do not call a source-only review a visual audit.
- Do not call web/hybrid formal coverage complete without the manifest-bound
  config and imported formal evidence chain.
- Do not call native coverage complete with browser evidence substituted for a
  native snapshot.
- Do not report the product UI ready while any requested journey, enabled
  control, implementation gap, or request-related completion-ledger item
  remains unresolved. A blocked but fully evidenced audit is reported as a
  completed audit with an unready product result.
- Keep full reports and logs in cold artifacts. Return only outcome, blocking
  status, finding counts, coverage caveats, verifier status, and artifact paths.
