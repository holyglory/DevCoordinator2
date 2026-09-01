# UI Implementation Audit Skill

`ui-implementation-audit` is an agent skill for checking whether a repo's
implemented interface matches its mockups, visual assets, and user journey
requirements. It combines source-level UI batching with rendered desktop/mobile
visual checks and produces a prioritized implementation plan for missing UI,
interaction, implementation, and test gaps.

This exhaustive skill is explicit-only: use the active runtime's native
skill-invocation mechanism. Ordinary UI implementation, review, testing,
visual checking, or gap-finding requests must not activate it implicitly.

Rendered evidence is real and bound. Web audits import the formal verifier
report, initial/full-page screenshots, changed-review queue, and manual-review
manifest deterministically into `visual_evidence.json`; native audits bind
native snapshots. Source rows record handler/backend/permission/persistence/test
`path#symbol` wiring references or report them missing. These references prove
source anchors exist, not that the observable behavior succeeded.

This skill has a hard applicability gate. At least one substantive, repo-owned
product screen, component, or native view must already exist in executable UI
source and be named with `--implemented-ui-file`. A partial implementation is
enough. Plans, requirements, mockups, prototypes, screenshots, stories, tests,
styles/assets, backend code, and untouched framework scaffolds are not. An
inapplicable run exits `3` and creates no audit artifacts.

The normal gate also requires a recognized executable UI surface with
substantive visible content or controls, so an arbitrary path, import, empty
view shell, or route/provider/root mount cannot qualify. For a real but
unrecognized UI toolkit, the exceptional
`--implemented-ui-override PATH UI-KIND SOURCE-ANCHOR` form records two
distinct exact source facts: an imported, inherited/conformed, or constructed
UI framework/view type and a named screen/component definition whose body uses
that type. The verifier rechecks the relationship. This is not a bypass for two
unrelated backend symbols, route scaffolding, or planned UI.

## Target

Use this skill when you want to audit:

- UI code against generated mockups, screenshots, Figma/ImageGen exports, brand
  assets, and product journey requirements.
- Required screens, controls, messages, states, handlers, data paths,
  accessibility paths, and visual/test evidence.
- Desktop, native, and mobile screenshots against expected visual hierarchy,
  density, spacing, imagery, states, readability, overload risk, and responsive
  fit.
- Rendered journey usability: visible content must help the user make the
  current journey decision, not merely avoid overflow or resemble the mockup.
- Journey-aware formal verification: primary content owns the initial viewport,
  activated work continues visibly and with focus, text/theme colors are
  measurable, and palette risks remain explicit visual-review evidence.
- Changed-input-only image review after all automatic checks: queued screenshot
  pairs are opened, carried unchanged pairs are not, and prior gaps remain
  blocking.
- Explicit `web`, `native`, or `hybrid` platform scope, avoiding extension-only
  classification such as treating React Native TSX as browser UI.
- Interface source files that define pages, screens, components, templates,
  styles, visible copy, native UI markup, and UI message catalogs.
- Missing visual tooling or safe fixture paths that prevent real screenshot
  verification.

Do not use it to design or plan a UI that has not been implemented. Start UI
design or implementation first, then audit once a real target surface exists.

The audited repository is kept read-only. Generated audit artifacts should live
outside the audited repo by default.

## How To Use

Invoke the skill explicitly:

```text
After confirming src/App.tsx implements a real product surface, use $ui-implementation-audit to audit this repo against its mockups and user journeys.
```

Or name a specific repository:

```text
After confirming app/views/contacts.html implements a real product surface, use $ui-implementation-audit to audit /path/to/repo.
```

You can force known design or requirement inputs:

```bash
python3 skills/ui-implementation-audit/scripts/build_ui_implementation_audit_batches.py \
  --repo /path/to/repo \
  --implemented-ui-file src/App.tsx \
  --ui-platform web \
  --formal-config formal-web-ui.json \
  --mockup docs/mockups/dashboard.png \
  --journey-file docs/product-journeys.md
```

## What It Produces

The harness creates an audit output directory containing:

- `manifest.json`: UI source files, coverage units, visual assets, mockups,
  requirement sources, implementation-gate evidence, batches, hashes, and
  coverage invariants.
- `audit_index.md`: dispatch guide for the lead agent.
- `batch_###.md`: prompts for low-effort UI source workers.
- `mockup_asset_audit.md`: prompt for extracting expected UI from mockups,
  assets, and journey requirements.
- `visual_tooling_audit.md`: prompt for finding runnable screenshot paths.
- `visual_comparison_audit.md`: prompt for desktop/mobile screenshot
  comparison.
- `visual_evidence.json`: real screenshot/native/formal-verifier, review-queue,
  and manual-review artifact records referenced as `evidence:<id>`.
- `execution_ledger.json`: lead-recorded worker status/provenance/fallback ledger;
  it contains no required reasoning-effort setting.
- `excluded_files.json`: skipped files and scope-warning reasons.
- `reports/`: required returned worker reports.
- `logs/`: verbose command output kept outside routine model context.
- `final-report.md`: complete lead visual/source synthesis.
- `queue_complete.json`: queue-generation marker, not proof that verification
  is complete.

## Direct Harness Usage

Run harness self-tests:

```bash
python3 skills/ui-implementation-audit/scripts/self_test.py
```

Generate an audit queue:

```bash
python3 skills/ui-implementation-audit/scripts/build_ui_implementation_audit_batches.py \
  --repo /path/to/repo \
  --implemented-ui-file src/App.tsx \
  --ui-platform web \
  --formal-config formal-web-ui.json \
  --out /tmp/ui-implementation-audit-run
```

Check eligibility without creating artifacts:

```bash
python3 skills/ui-implementation-audit/scripts/build_ui_implementation_audit_batches.py \
  --repo /path/to/repo \
  --implemented-ui-file src/App.tsx \
  --eligibility-only
```

Exceptional preflight for an unrecognized toolkit, after manual inspection:

```bash
python3 skills/ui-implementation-audit/scripts/build_ui_implementation_audit_batches.py \
  --repo /path/to/repo \
  --implemented-ui-override src/contacts.rs 'canvas_kit::ContactSurface' build_contacts \
  --eligibility-only
```

Verify saved reports:

```bash
python3 skills/ui-implementation-audit/scripts/verify_ui_implementation_audit_results.py --manifest /tmp/ui-implementation-audit-run/manifest.json --reports /tmp/ui-implementation-audit-run/reports
```

Import a completed formal Web UI evidence bundle before verification:

```bash
python3 skills/ui-implementation-audit/scripts/import_formal_web_evidence.py \
  --audit-root /tmp/ui-implementation-audit-run \
  --run-id <audit-run-id> \
  --formal-report /tmp/ui-implementation-audit-run/artifacts/report.json \
  --review-queue /tmp/ui-implementation-audit-run/artifacts/review-queue.json \
  --manual-review /tmp/ui-implementation-audit-run/artifacts/manual-review.json
```

Useful builder options include `--out`, `--mockup`, `--journey-file`,
`--include-generated`, `--include-vendor`, `--include-env`, `--include-file`,
`--include-glob`, `--implemented-ui-file`, `--implemented-ui-override`,
`--ui-platform`, `--formal-config`, `--eligibility-only`, `--batch-size`, and
`--max-batch-bytes`.

## Coverage Rules

A run is complete only after:

1. The manifest contains a passed, hash-bound implemented-UI gate.
2. Every generated source batch has a saved report in `reports/batch_###.md`.
3. Mockup/assets, visual tooling, and visual comparison reports exist. Web and
   hybrid audits import the formal report, changed-review queue, screenshot
   pairs, and manual-review evidence; native and hybrid audits bind native
   snapshots.
4. `execution_ledger.json` records completed lead, worker, and fallback status.
5. `final-report.md` contains the required non-empty synthesis and interaction
   checklist.
6. `verify_ui_implementation_audit_results.py` returns `ok: true`.

The verifier checks structure, hashes, report coverage, current source drift,
scope warnings, visual comparison evidence shape, first-viewport journey
coverage, and ledger completion. It cannot prove the semantic truth of each
visual judgment; the lead agent remains responsible for reviewing screenshot
evidence before final synthesis. That review happens only after automatic tests
and only for queue entries selected by changed declared UI inputs/intent or new
coverage; screenshot pixels and hashes never select review work.

Workers use fresh isolated contexts and runtime/user-selected defaults; the
skill does not prescribe or validate reasoning effort. When workers cannot be
spawned, use the disclosed manual fallback. Workers write complete reports directly to their
declared artifact paths and return only compact filename-bearing receipts. The
final chat response is a short outcome, counts, caveats, verifier status, and
artifact index.
