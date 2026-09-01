# User Journey Docs Audit

`user-journey-docs-audit` performs a lexical and structural check of whether a repo has enough product and
development documentation to build an app that is genuinely easy to use and
complete: journeys, features, UI elements, implementation expectations, and
tests are all spelled out.

Policy files and source hints are classified separately and never establish
confirmed product truth. Native SwiftUI, XAML, AppKit/UIKit, Compose, Flutter,
and QML source can expose undocumented UI surfaces, but user confirmation or
product documentation remains required.

Use it when an existing repository or supplied documentation set is the audit
target and the user explicitly asks for documentation-completeness,
journey-readiness, or UI-handoff-readiness findings. Do not use it for
greenfield discovery, brainstorming, domain or data modeling, database/system
specification, ordinary requirements discussion, general critique, or
co-writing a new specification. “Do not start coding” does not make a request
a documentation audit.

## What It Audits

- App idea, user groups, expertise, context, and mistake cost.
- Complete feature inventory, required UI element inventory, implementation
  expectations, and test expectations.
- Complete journey inventory, including frequent, rare, failure, permission, onboarding, recovery, and power-user paths.
- Per-journey decision model: primary decision, required facts, warning/flag conditions, frequent actions, secondary/rare actions, and unresolved assumptions.
- Information relevance inventory for UI implementation: critical-always, primary-frequent, secondary-occasional, rare-under-5-percent, conditional, destructive, and expert-only information/actions.
- UI handoff constraints for the implementation audit: screens/routes/states to verify, evidence expected from screenshots or rendered surfaces, and assumptions that remain unconfirmed.
- Formal web verification handoff per route/state: primary journey and
  frequency/risk, initial-viewport region roles, visible/focused continuation,
  light/dark/mixed theme intent, implementation-input ownership, and
  initial/full-page screenshots reviewed only after mapped UI inputs or intent
  changes.
- Mobile and desktop fit for critical information and actions.
- Accessibility, states, acceptance criteria, QA hooks, analytics/support clues, and implementation readiness.

## Required Behavior

The skill must actively interview the user when journey information is missing or ambiguous. It should not silently infer journeys and call the docs complete. If the user cannot answer during the run, the report must label those journeys as `journey assumptions unconfirmed`.

For UI handoff, the skill should flag a P1 when docs use terms such as `dense`, `dashboard`, `command center`, `overview`, or `compact` without defining the decisions, information relevance, action frequency, and assumptions behind those terms.

Product docs should remain semantic: they identify primary content, secondary
workflows, compact support, justified blocking alerts, and implementation-input
ownership. The implemented formal verifier configuration supplies exact CSS
selectors and repository-relative input paths.

## Helper Command

```bash
python3 skills/user-journey-docs-audit/scripts/build_journey_docs_inventory.py --repo <repo>
```

Use `--json` when another script or agent needs machine-readable inventory output.

Validate a completed audit report with:

```bash
python3 skills/user-journey-docs-audit/scripts/verify_journey_docs_audit_results.py report.md
```

## References

- `references/ux_principles.md`: compact rubric of UX principles used by the audit.
- `references/journey_doc_template.md`: reusable documentation template for app idea, users, journeys, screen requirements, and QA acceptance criteria.
