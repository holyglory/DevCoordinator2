---
name: ui-implementation-audit
description: Run an explicit exhaustive audit of an existing substantive product UI against user journeys, avoidable user effort, applicable UI guidelines, mockups, source wiring, rendered evidence, and tests. Use only through the active runtime's explicit skill-invocation mechanism after manually confirming at least one repo-owned executable screen/component/view. Do not use for ordinary implementation or review, pre-implementation plans, mockups, screenshots, stories/tests, styles/assets alone, scaffolding, or backend-only repositories.
---

# UI Implementation Audit

## Purpose And Boundary

This is an explicit-only, read-only assurance workflow. Invocation authorizes
the audit, its isolated workers, and external audit artifacts—not product
implementation or unrelated repository changes.

The installed skill must be a direct link into the canonical DevCoordinator2
checkout. Invoke the installed `devcoordinator2-tooling` binary backed by the
shared Rust tooling source; copied standalone skill packages are unsupported.

Use it to determine whether an implemented product UI is complete and faithful
to its journeys and design target:

- Are all intended screens, states, controls, messages, and journeys present?
- Do visible actions have source wiring references for handlers, navigation,
  APIs, permissions, persistence, and tests?
- Does the rendered product support intended decisions and match relevant
  mockups across its declared platforms?
- Can users reach the intended outcome without unnecessary navigation,
  decisions, repeated entry, explanations, or separate screens?
- Does each surface and piece of supporting text serve the user's current
  task rather than expose the implementation or its development history?
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

Read the effective universal and project UI guidelines, confirmed product
requirements, and applicable glossary through their established sources.
Record the revisions or immutable Coordinator refs used, including confirmed
corrections and current design selections. A matching mockup is evidence of
fidelity, not an exemption from usability requirements; report a harmful design
target as a finding rather than recommending faithful reproduction of its
problems.

Do not audit whether mockups existed before implementation. Do not require
creation dates, reconstruct historical mockups, or turn missing historical
material into a process finding. Review available design targets and current
decisions without reopening approved designs or routine fixes.

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

## Journey And UX Review Contract

Derive the journey inventory from intended user outcomes before mapping the
existing screens. Include requested but missing journeys. Give every journey a
stable `Journey ID` and requirement evidence; record unconfirmed assumptions
rather than treating the existing implementation as the specification.

Complete the generated `Journey Flow Review` table for every journey. Record
the starting situation, intended outcome, actual ordered path, every surface
visited, outcome evidence, unnecessary effort, a simpler valid alternative,
result, and rationale. Examine the necessity of each action, decision, wait,
handoff, repeated entry, and backtracking. Include applicable cancellation and
recovery. Do not substitute an imagined walkthrough or a screenshot for proof
of saving, processing, or completion. Do not invent measured efficiency gains.

Complete the generated `UX Guideline Review` table with evidence-backed
assessments of every criterion for each journey:

- `journey-first`: the experience serves the user's outcome, including forms,
  rather than the application's entities or component boundaries.
- `step-necessity`: each step earns its place; compare a simpler coherent path.
- `surface-purpose`: each separate page, tab, mode, or dialog has a distinct
  user benefit that cannot be served better in the existing context.
- `copy-purpose`: each status, helper, description, or explanation helps the
  user act, decide, understand a result, or avoid a concrete error at that point.
  Consider a clearer label, default, placement, or interaction before more copy.
- `product-language`: users need no development history or internal knowledge;
  implementation notes stay outside the product unless reviewing them is the
  actual user task. Preserve useful domain terminology and necessary guidance.
- `project-guidelines`: assess the applicable project expectations and glossary
  against the rendered experience and identify the actual source used.
- `context-inheritance`: known project/parent values are inherited and can be
  changed without unnecessary entry; infrequently changed context uses clickable
  text rather than permanent full-size selectors. Cancellation preserves context.
- `contextual-actions`: actions stay beside their object, including useful
  next actions in empty states, without unrelated controls.
- `compact-choices`: small option sets use direct choices with recognizable
  icons and labels, clear selection, and working keyboard/touch behavior.
- `progressive-disclosure`: optional fields and infrequently changed context
  stay compact; revealing them preserves values, focus, and task context.
- `overlay-behavior`: dropdowns overlay rather than stretch forms, remain
  positioned and reachable, and dismiss correctly on supported inputs.
- `generated-results`: live generated output follows input changes, agrees
  with the saved result, and handles invalid, pending, and failed calculation.
- `contextual-help`: detailed help is available beside its action without
  default clutter, hover-only access, or hiding essential guidance.

Use the generated columns without replacing them with a blanket checklist.
List exact surface names separated by semicolons in each flow. Review each
surface separately and each supporting-text or contextual-control item
separately, including conditional items. Reconcile these inventories with the
source workers' reports. The seven contextual criteria cannot use `Surface=all`;
passing controls need a concrete item and runtime observation, not a screenshot.
A surface without supporting text gets an evidenced, reasoned
`NOT_APPLICABLE` copy row; it must not disappear from coverage. Other criteria
may use `Surface=all`. No automatic verifier can prove that a subjective
judgment is correct: the reviewer owns the completeness and quality of the
observations; software enforces declared coverage, evidence, and consistency.

Use `PASS`, `GAP`, or `BLOCKED` for journeys. Guideline rows may also use a
reasoned `NOT_APPLICABLE`, except for journey-first, step-necessity, and
surface-purpose. Every non-blocked assessment cites registered observation
evidence, not only a prior review decision. Every `GAP`/`BLOCKED` links to a
complete finding with a unique `Finding ID`; use `Finding=none` for clean rows.
Preserve all journeys, item assessments, and unresolved results in the final
report. Do not clear gaps merely during synthesis.

These are reviewer-owned checks, not extra user approval gates. Minimize user
effort rather than imposing click quotas, hiding essential actions, or merging
distinct tasks into an overloaded screen. Report evidence limitations honestly
and keep unavailable outcome proof blocked rather than guessing.

## Configuration And Design Coverage

Declare the `UI Configuration Contract` from requirements and supported
configurations before choosing captures. Each unique platform/theme/viewport/
input combination has a stable `Config ID`, applicable semicolon-separated
`Journey IDs`, and a revision-bound requirement source. Do not infer support
from available screenshots. Cover every supported theme and relevant wide/narrow
layout and input method (`pointer`, `keyboard`, `touch`). Use `web`,
`desktop:<target>` for installed desktop apps, or `native:<target>` for other
native targets; unknown support is explicitly `unknown`, with blocked
interaction/build evidence, never guessed completeness.
The reviewer establishes the truthful inventory; software checks its declared
coverage and preserves it through synthesis.

For each declared journey/configuration pair, complete `Interaction Coverage`
for `completion`, `cancellation`, `validation-error`, `recovery`, `persistence`,
`loading`, `empty`, and `long-content`. Record a precise target, expected result,
observed behavior, evidence segment/cell, verdict, and finding. Completion cannot
be waived. Other scenarios may be reasoned `NOT_APPLICABLE` when genuinely absent.
Passing interactions require a registered trace, video, or ordered journey
bundle; screenshots and aggregate formal reports alone are insufficient. Check
actual focus, activation, dismissal, touch behavior, and saved state after
reload/reopening in that configuration. Reuse evidence that actually covers
multiple cells; do not relabel one theme or input as proof of another.
Native/desktop execution requires a trace or video record whose `platform`
metadata exactly matches the declared target, such as `desktop:linux-x64`.
Imported browser journey evidence cannot substitute for native execution.

Each desktop configuration names its `Update journey` among the declared
journeys; other platforms use `none`. In an isolated test installation, add
`update-startup`, `update-periodic`, `update-download`, `update-ready`,
`update-restart`, and `update-recovery` cells. Demonstrate background checks and
downloads, a ready-only Update control, user-triggered installation/restart,
unsaved-work preservation, and failure recovery. Update cells cannot be waived;
an unimplemented updater is a gap, and unavailable safe execution is blocked.
The Update button stays small in the caption area and appears only after the
download; checking and downloading must not interrupt ongoing work.
Do not update the user's working installation as an audit side effect.

Complete `Rendered Build Review` once per configuration. Record its accessible
URL or native package/launch target, expected snapshot, observed snapshot, and
runtime evidence. Matching source/build identities are necessary for `PASS`;
stale or inaccessible targets remain gaps or blockers. Use existing deployment
or build evidence, not extra development metadata on ordinary product screens.

Complete `Design Decision Review` once per supplied design target and link its
journeys. Applicability is `alternatives`, `approved-design`, `routine-fix`, or
`unavailable`. When alternatives are supplied, assess three named options, real
layout/hierarchy/interaction differences, the selected option, and user-selection
or explicit autonomous authority. Distinctions are reviewer judgments, not a
color/name-count detector. Approved designs and routine fixes do not need new
options. With no usable target, record an evidenced, reasoned `NOT_APPLICABLE`
design review and continue the requirement-based UX audit. Do not manufacture
fidelity evidence or penalize the absence of historical mockups.

The four tables persist in final synthesis with exact configuration, target,
snapshot, and evidence bindings; gaps cannot become passes in a summary. A
journey cannot pass while its interaction, build, or applicable design review
has an unresolved gap/blocker. Share design/component evidence across journeys
when valid rather than repeating the same review.

Keep delivery watchdogs, implementation stop rules, and daily resource accounting
outside this UI audit. Inspect the version and user-accessible surface, not the
project's delivery schedule. A complete audit is not a prerequisite for every
preliminary deployment.

## Workflow

1. **Confirm eligibility**
   - Inspect and name at least one substantive executable UI source.
   - Run the builder with `--eligibility-only`. Exit `3` means not applicable.

2. **Build the audit queue**
   - Resolve `UI_IMPLEMENTATION_AUDIT_SKILL_DIR` from the loaded skill path
     and `CANONICAL_SKILL_ROOT` as its repository root.
   - Run `devcoordinator2-tooling skills self-test audit-tooling --source-root "$CANONICAL_SKILL_ROOT"` unless validation commands are forbidden.
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
     screenshots. Finalize decisions with
     `devcoordinator2-tooling formal-ui review`.
   - Import the completed formal bundle into `visual_evidence.json` using
     `devcoordinator2-tooling audit ui-implementation import-formal`; do not transcribe its screenshot,
     journey-evidence, queue, or review records manually.
   - For native/hybrid, register real `native-snapshot` evidence.
   - Compare rendered results against journeys and supplied design targets.
     Missing mockups are an explicit comparison limitation, not historical
     noncompliance or permission to substitute personal taste.

5. **Complete visual review**
   - Complete the journey decision model, flow review, and guideline review
     using the contract above, including requested journeys not yet implemented.
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
   - Explain each improvement through the user effort it removes or the
     decision it supports. Preserve necessary steps and guidance with reasons.
   - Write `final-report.md` before running the result verifier.

7. **Verify completion**
   - Run `devcoordinator2-tooling audit ui-implementation verify` against the
     manifest and reports directory.
   - The verifier checks source/unit coverage, current hashes, worker status,
     formal/native evidence, final-report structure, interaction labels, and
     evidence references. It also checks complete journey/criterion/surface
     coverage, supported results, finding links, and faithful final synthesis.
   - `ok: true` means the audit artifact set is internally complete. It does not
     mean the product UI is ready; the final report may truthfully remain
     `GAP`/`BLOCKED` until its findings are implemented and retested.

## Commands

Eligibility only:

```bash
devcoordinator2-tooling audit ui-implementation build \
  --repo "$REPO_ROOT" \
  --implemented-ui-file src/App.tsx \
  --eligibility-only
```

Web audit queue:

```bash
devcoordinator2-tooling audit ui-implementation build \
  --repo "$REPO_ROOT" \
  --implemented-ui-file src/App.tsx \
  --ui-platform web \
  --formal-config formal-web-ui.json
```

Import completed formal evidence:

```bash
devcoordinator2-tooling audit ui-implementation import-formal \
  --audit-root <audit-output> \
  --run-id <audit-run-id> \
  --formal-report <audit-output>/artifacts/report.json \
  --journey-evidence <audit-output>/artifacts/journey-evidence.json \
  --review-queue <audit-output>/artifacts/review-queue.json \
  --manual-review <audit-output>/artifacts/manual-review.json
```

Verify the complete audit:

```bash
devcoordinator2-tooling audit ui-implementation verify \
  --manifest <audit-output>/manifest.json \
  --reports <audit-output>/reports
```

## Final Report

`final-report.md` contains exactly these top-level sections:

```markdown
## Coverage
## Mockup And Requirement Inputs
## Journey Decision Model
## Journey Flow Review
## UX Guideline Review
## UI Configuration Contract
## Interaction Coverage
## Rendered Build Review
## Design Decision Review
## Rendered Journey Usability Findings
## Visual Audit Findings
## Source Implementation Findings
## Journey And Responsive Findings
## Accessibility And Interaction Findings
## Implementation Plan
## Verification Plan
```

Every section is non-empty. `Coverage` names the run id and declared platform.
All journey, guideline, configuration, interaction, build, and design sections
retain the generated tables and stable IDs, not prose substitutes. The
implementation plan includes complete finding blocks
for the referenced `Finding ID`s, deduplicating without losing their links.
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
