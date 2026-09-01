#!/usr/bin/env python3
"""Self-tests for user-journey-docs-audit resources."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path
from shutil import rmtree


ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "scripts" / "build_journey_docs_inventory.py"
REPORT_VERIFY = ROOT / "scripts" / "verify_journey_docs_audit_results.py"
REQUIRED_REFERENCES = {
    "references/ux_principles.md": ["# UX Principles", "Nielsen Norman Group", "WCAG"],
    "references/journey_doc_template.md": [
        "# User Journey Documentation Template",
        "## Journey Inventory",
        "## Journey Decision Model",
        "## Information Relevance Inventory",
        "## Interaction And Metadata Model",
        "## UI Handoff Constraints",
        "## Formal Web UI Verification Handoff",
        "## Screen Requirements",
    ],
}


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def run_inventory(repo: Path) -> dict:
    result = subprocess.run(
        [sys.executable, str(INVENTORY), "--repo", str(repo), "--json"],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return json.loads(result.stdout)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def run_report_verify(path: Path, *, expect: int) -> None:
    result = subprocess.run(
        [sys.executable, str(REPORT_VERIFY), str(path)],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != expect:
        raise AssertionError(
            f"Expected report verifier exit {expect}, got {result.returncode}: {result.stdout}\n{result.stderr}"
        )


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="user-journey-docs-audit-self-test-"))
    try:
        skill_contract = (ROOT / "SKILL.md").read_text(encoding="utf-8")
        frontmatter = skill_contract.split("---", 2)[1]
        for needle in [
            "Audit an existing repository or supplied product/journey documentation set",
            "Use only when the user explicitly asks to audit, assess, score, validate, or find",
            "greenfield product discovery",
            "database or system specification",
            "co-writing a new specification",
            "“roast” an idea",
            "“do not start coding” is not an audit trigger",
        ]:
            check(needle in frontmatter, f"selector-visible trigger contract should contain {needle!r}")

        for needle in [
            "## Applicability Gate",
            "An existing repository or supplied documentation set is the object being evaluated.",
            "stop before running inventory scripts",
            "design a database for part numbering, serial numbers, and document identification",
            "Help me brainstorm users and workflows for a new product.",
            "Review this numbering scheme and propose a better domain model.",
            "Formal Web UI verification handoff",
            "continuation anchor",
            "changed visual review",
        ]:
            check(needle in skill_contract, f"applicability contract should contain {needle!r}")

        readme = (ROOT / "README.md").read_text(encoding="utf-8")
        for needle in [
            "existing repository or supplied documentation set is the audit",
            "database/system",
            "co-writing a new specification",
            "“Do not start coding” does not make a request",
        ]:
            check(needle in readme, f"README trigger guidance should contain {needle!r}")

        agent_metadata = (ROOT / "agents" / "openai.yaml").read_text(encoding="utf-8")
        check(
            'short_description: "Audit existing docs for journey readiness"' in agent_metadata,
            "agent metadata should identify existing documentation as the audit target",
        )
        check(
            any(
                line.strip().startswith("default_prompt:")
                and "$user-journey-docs-audit" in line
                and "existing product and journey documentation" in line
                for line in agent_metadata.splitlines()
            ),
            "default prompt should explicitly invoke the skill against existing documentation",
        )

        for rel_path, required_text in REQUIRED_REFERENCES.items():
            reference = ROOT / rel_path
            check(reference.is_file(), f"required reference is missing: {rel_path}")
            text = reference.read_text(encoding="utf-8")
            check(text.strip(), f"required reference is empty: {rel_path}")
            for needle in required_text:
                check(needle in text, f"required reference {rel_path} should contain {needle!r}")

        empty_repo = tmp / "empty"
        empty_repo.mkdir()
        write(empty_repo / "README.md", "# Tool\n\nA small app.\n")
        empty = run_inventory(empty_repo)
        check(empty["doc_count"] == 1, "README should be inventoried")
        check(empty["journey_doc_count"] == 0, "weak README should not count as journey docs")
        check(empty["missing_signals"], "weak docs should produce missing signals")

        keyword_noise_repo = tmp / "keyword-noise"
        write(keyword_noise_repo / "README.md", "# App\n\nA user-facing app with a mobile screen and desktop screen.\n")
        keyword_noise = run_inventory(keyword_noise_repo)
        check(keyword_noise["journey_doc_count"] == 0, "generic UI keywords should not count as journey docs")

        weak_mobile_repo = tmp / "weak-mobile"
        write(
            weak_mobile_repo / "docs" / "product.md",
            """# Product Overview

This app has a dashboard, mobile screen, desktop screen, settings, filters, and charts.

## Mobile

The mobile screen should be compact and avoid horizontal overflow.
""",
        )
        weak_mobile = run_inventory(weak_mobile_repo)
        check(weak_mobile["decision_model_doc_count"] == 0, "weak mobile docs should not provide a decision model")
        check(weak_mobile["information_relevance_doc_count"] == 0, "weak mobile docs should not provide information relevance requirements")
        check(not weak_mobile["ui_audit_handoff_ready"], "weak mobile docs should not be UI-audit handoff ready")
        check(
            any("decision information" in item or "journey decision model" in item for item in weak_mobile["ui_implementation_risk_signals"]),
            "weak mobile docs should warn about missing decision/relevance documentation",
        )

        density_gap_repo = tmp / "density-gap"
        write(
            density_gap_repo / "docs" / "metrics-dashboard.md",
            """# Metrics Dashboard Page

Users can view a dense command center dashboard and adjust display settings.

## Mobile Screen

The page has a Settings panel and a primary metrics list.

## Acceptance

The mobile screen should be compact and have no horizontal overflow.
""",
        )
        priority_gap = run_inventory(density_gap_repo)
        check(priority_gap["decision_model_doc_count"] == 0, "ambiguous docs should not accidentally count as decision-ready")
        check(priority_gap["information_relevance_doc_count"] == 0, "ambiguous docs should not accidentally count as relevance-ready")
        check(
            any("UI intent terms" in item for item in priority_gap["ui_implementation_risk_signals"]),
            "ambiguous docs should warn when dense/dashboard intent lacks decision and relevance definitions",
        )

        interaction_gap_repo = tmp / "interaction-gap"
        write(
            interaction_gap_repo / "docs" / "case-review.md",
            """# Case Review Journey

Operators review a queue dashboard with status badges, warning flags, expandable rows, messages, and result blocks.

## Journey Decision Model

| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unresolved assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Case Review | choose the next case | decide whether to inspect a flagged row | status badge, warning flag, latest message | red and yellow flags | inspect row | raw tool result block | none |

## Information Relevance Inventory

| Journey | Surface | Item | Relevance | Why it matters | Condition/frequency | Evidence source |
| --- | --- | --- | --- | --- | --- | --- |
| Case review | Case Review | status badge | critical-always | shows urgency | always | product requirement |
| Case review | Case Review | raw tool result block | rare-under-5-percent | supports debugging | rare | product requirement |

## UI Handoff Constraints

Use screenshots and rendered-state evidence to verify the case review surface.

## Feature Inventory
Features: status badges, warning flags, expandable rows, messages, result blocks.

## UI Element Inventory
UI elements: badge, flag, row, message, result block.

## Implementation Expectations
Handlers, state, persistence, validation, and permissions must exist.

## Test Expectations
Acceptance criteria, fixture, visual test, and accessibility checks must exist.
""",
        )
        interaction_gap = run_inventory(interaction_gap_repo)
        check(interaction_gap["decision_model_doc_count"] >= 1, "interaction-gap docs should have a decision model")
        check(interaction_gap["information_relevance_doc_count"] >= 1, "interaction-gap docs should have relevance docs")
        check(not interaction_gap["has_interaction_access_model"], "interaction-gap docs should miss the access model")
        check(not interaction_gap["ui_audit_handoff_ready"], "interaction-gap docs should not be UI-audit ready")
        check(
            any("click/hover/focus targets" in item for item in interaction_gap["ui_implementation_risk_signals"]),
            "interaction-gap docs should warn about missing interaction targets and popover/detail access",
        )

        complete_repo = tmp / "complete"
        write(
            complete_repo / "docs" / "journey-decision.md",
            """# Metrics Dashboard Journey Documentation

## App Idea

Product promise: help operators understand live metrics quickly.
Primary users: operations analysts.
Primary value: high-priority metrics are available before configuration.

## Journey Inventory

| Journey | User | Frequency | Importance | Risk if broken | Entry point | Success state |
| --- | --- | ---: | ---: | ---: | --- | --- |
| Review live metrics | operations analyst | high | high | high | /metrics | user can decide current metric status |

## Journey Decision Model

| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unresolved assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Metrics Dashboard | understand current live metrics quickly | decide current metric status | high-priority metric list, latest chart, latest update time | threshold warning and stale-data warning | inspect metric details | display filter and advanced settings configuration | none |

## Information Relevance Inventory

| Journey | Surface | Item | Relevance | Why it matters | Condition/frequency | Evidence source |
| --- | --- | --- | --- | --- | --- | --- |
| Review live metrics | Metrics Dashboard | high-priority metric list | critical-always | lets the analyst decide current status | always | product requirement |
| Review live metrics | Metrics Dashboard | display filter | secondary-occasional | helps refine the view | occasional | product requirement |
| Review live metrics | Metrics Dashboard | advanced settings configuration | rare-under-5-percent | supports admin tuning | rare | product requirement |

## UI Handoff Constraints

| Surface | Decisions the UI must support | Required evidence for UI audit | States to verify | Mockups/screenshots/assets | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- |
| Metrics Dashboard | decide current metric status using live metrics, chart, and warning state | screenshot, DOM viewport measurement, and visual audit evidence | loading, empty, error, stale-data warning, threshold warning | metrics-dashboard mockup | none |

## Feature Inventory

Features and capabilities: metrics summary, live chart, inspect metric details, display filter, advanced settings configuration, threshold warning, empty state, loading state, and error recovery.

## UI Element Inventory

Required UI elements: metric cards, chart, inspect button, display filter control, settings menu, threshold banner, loading state, empty state, error toast, and retry button.

## Implementation Expectations

Handlers, API data path, persistence, validation, permission check, and state change behavior must be defined for the metric detail action, display filter, settings menu, and retry path.

## Test Expectations

Acceptance criteria, unit test, component test, e2e, visual test, fixture, and test mode expectations must cover metrics loading, empty data, threshold warning, filter persistence, permission denial, error retry, and constrained-screen journey usefulness.

## UI Handoff Constraints

Use the UI implementation audit with a mobile screenshot, mockup, and DOM/native viewport measurement to confirm the rendered surface supports the live-metric decision and keeps conditional settings from overwhelming that decision path.

## Interaction And Metadata Model

| Surface | Element or metadata | User intent | Interaction target | Feedback/detail access | Stability/accessibility expectation |
| --- | --- | --- | --- | --- | --- |
| Metrics Dashboard | threshold warning badge | decide whether status needs action | badge and keyboard focus | hover/focus hint and click popover detail | stable position, accessible name, no scrollbar collision, popover closes on outside click or focus loss after idle timeout |
| Metrics Dashboard | latest update timestamp | know data freshness | not interactive | passive visible metadata | timestamp is not selectable message content |

## Formal Web UI Verification Handoff

- Primary journey: Review live metrics; usage percentage: 95%; risk if broken: high. A lower-frequency journey needs a priority override reason.
- Initial viewport: the metrics list is the `primary-content`; compact filters are a supporting region; settings are a secondary `workflow-surface`; a connectivity message may be a justified `blocking-alert`.
- Continuation anchor: after Inspect metric, its detail heading is visible without scrolling, focus enters the detail surface, and document scroll does not jump. A dedicated flow declares its expected destination route, predictable pointer cursor, and keyboard focus affordance.
- Theme intent: light, dark, or mixed is declared for each state; this surface is light.
- Implementation-input ownership: the verifier maps review inputs for UI code, styles, tokens, fonts, and assets without putting CSS selectors in product intent.
- Evidence: retain an initial-viewport screenshot and full-page screenshot pair; changed visual review opens only cells whose mapped inputs or intent changed.
""",
        )
        complete = run_inventory(complete_repo)
        check(complete["journey_doc_count"] >= 1, "complete docs should count as journey docs")
        check(complete["decision_model_doc_count"] >= 1, "complete docs should count decision model docs")
        check(complete["information_relevance_doc_count"] >= 1, "complete docs should count information relevance docs")
        check(complete["ui_handoff_constraint_doc_count"] >= 1, "complete docs should count UI handoff constraint docs")
        check(complete["has_feature_inventory"], "complete docs should include feature inventory")
        check(complete["has_ui_element_inventory"], "complete docs should include UI element inventory")
        check(complete["has_implementation_expectations"], "complete docs should include implementation expectations")
        check(complete["has_test_expectations"], "complete docs should include test expectations")
        check(complete["has_formal_verification_handoff"], "complete docs should include the formal verifier handoff")
        check(complete["ui_audit_handoff_ready"], "complete docs should be UI-audit handoff ready")
        check(not complete["ui_implementation_risk_signals"], "complete docs should clear UI implementation risk signals")

        overprescribed_repo = tmp / "overprescribed"
        write(
            overprescribed_repo / "docs" / "canonical-journeys.md",
            """# Operations Console Journey

## App Idea

Product promise: help operators supervise review queues.
Primary users: operators.
Primary value: the next review decision is clear.

## Journey Inventory

| Journey | User | Frequency | Importance | Risk if broken | Entry point | Success state |
| --- | --- | ---: | ---: | ---: | --- | --- |
| Review operations console | operator | high | high | high | /console | user knows which case needs attention |

## Journey Decision Model

| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unresolved assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Operations Console | decide which case needs attention | act now or continue monitoring | urgent case count and blocking state | critical and warning states | inspect case | raw evidence and owner metadata | none |

## Information Relevance Inventory

| Journey | Surface | Item | Relevance | Why it matters | Condition/frequency | Evidence source |
| --- | --- | --- | --- | --- | --- | --- |
| Review operations console | Operations Console | urgent case indicator | critical-always | tells user whether to act | always | product requirement |
| Review operations console | Operations Console | raw evidence and owner | rare-under-5-percent | supports deep debugging | rare | product requirement |

## UI Handoff Constraints

Required visible decision evidence: status chip, stage chip, severity summary, raw evidence, owner, next action, and raw status summary must show in the operations console.

## Feature Inventory
Features: urgent case indicator, warning count, raw signal evidence, owner metadata, retry.

## UI Element Inventory
UI elements: badge, status chip, details expander, retry button, error state.

## Implementation Expectations
Handlers, persistence, validation, and state changes must be implemented.

## Test Expectations
Acceptance criteria, fixture, unit test, visual test, and recovery test must exist.
""",
        )
        overprescribed = run_inventory(overprescribed_repo)
        check(overprescribed["decision_model_doc_count"] >= 1, "overprescribed docs still have decision docs")
        check(overprescribed["information_relevance_doc_count"] >= 1, "overprescribed docs still have relevance docs")
        check(overprescribed["ui_handoff_constraint_doc_count"] >= 1, "overprescribed docs still have handoff docs")
        check(overprescribed["prescriptive_ui_risk_doc_count"] >= 1, "overprescribed docs should be flagged")
        check(not overprescribed["ui_audit_handoff_ready"], "overprescribed docs should not be UI-audit handoff ready")
        check(
            any("always-visible layout requirements" in item for item in overprescribed["ui_implementation_risk_signals"]),
            "overprescribed docs should report a prescriptive UI risk",
        )

        nested_repo = tmp / "nested-repo"
        write(nested_repo / "README.md", "# Review App\n\nProduct overview and app purpose for operators.\n")
        write(
            nested_repo / ".gitmodules",
            """[submodule "external/product-docs"]
	path = external/product-docs
	url = https://example.invalid/product-docs.git
""",
        )
        write(
            nested_repo / "external" / "product-docs" / "docs" / "journey.md",
            """# External Journey

## Journey Decision Model
Primary user goal: external.
Primary decision: external.
Required facts: external.

## Information Relevance Inventory
critical-always external detail.
""",
        )
        write(
            nested_repo / "packages" / "vendored-app" / ".git" / "HEAD",
            "ref: refs/heads/main\n",
        )
        write(
            nested_repo / "packages" / "vendored-app" / "docs" / "journey.md",
            """# Vendored Journey

## Journey Decision Model
Primary user goal: vendored.
Primary decision: vendored.
Required facts: vendored.

## Information Relevance Inventory
critical-always vendored detail.
""",
        )
        nested_inventory = run_inventory(nested_repo)
        nested_doc_paths = {doc["path"] for doc in nested_inventory["docs"]}
        check("external/product-docs/docs/journey.md" not in nested_doc_paths, "git submodule docs should be excluded by default")
        check("packages/vendored-app/docs/journey.md" not in nested_doc_paths, "nested repo docs should be excluded by default")

        journey_repo = tmp / "journey"
        write(
            journey_repo / "docs" / "user-journeys.md",
            """# User Journeys

## Admin reviews upload

User: admin
Goal: review uploaded evidence
Entry point: dashboard
Route: /admin/uploads
Primary decision: approve or reject
Mobile: critical summary and warnings fit first
Error state: permission denied
Acceptance: admin can recover from failed upload
""",
        )
        write(journey_repo / "README.md", "# Evidence App\n\nProduct overview and purpose for users.\n")
        rich = run_inventory(journey_repo)
        check(rich["journey_doc_count"] >= 1, "journey docs should be detected")
        check(rich["product_doc_count"] >= 1, "product docs should be detected")
        check(not any("No strong user journey" in item for item in rich["missing_signals"]), "journey warning should clear")

        source_repo = tmp / "source"
        write(source_repo / "README.md", "# App\n\nPurpose and app value for users.\n")
        write(source_repo / "src" / "App.tsx", '<a href="/settings">Settings</a><button aria-label="Save profile">Save</button>')
        source = run_inventory(source_repo)
        check(source["source_hint_count"] == 1, "source route/visible text hints should be detected")

        native_source_repo = tmp / "native-source"
        write(native_source_repo / "README.md", "# Native App\n\nA local operator tool.\n")
        write(
            native_source_repo / "Sources" / "Dashboard.swift",
            'struct Dashboard: View { var body: some View { VStack { Text("Service health"); Button("Restart") {} } } }\n',
        )
        write(
            native_source_repo / "Views" / "MainWindow.axaml",
            '<StackPanel><TextBlock Text="Database backups"/><Button Content="Verify backup"/></StackPanel>\n',
        )
        native_source = run_inventory(native_source_repo)
        check(native_source["source_hint_count"] == 2, "SwiftUI and XAML visible-text hints should be detected")

        policy_repo = tmp / "policy-only"
        write(
            policy_repo / "AGENTS.md",
            """# Global Agent Instructions

## Journey Decision Model
Primary user goal: audit every user workflow.
Primary decision: decide whether the app is complete.
Required facts: product routes and acceptance criteria.

## Information Relevance Inventory
critical-always user journey evidence.
""",
        )
        write(policy_repo / "CLAUDE.md", "# Claude Policy\n\nAgents must check product and user journeys.\n")
        write(
            policy_repo / "DecisionHistory.md",
            "# Decision History\n\nA compact governance index for product direction.\n",
        )
        write(
            policy_repo / "DecisionDetails" / "D-20260714-01.md",
            "# Historical product decision\n\nUser journey workflow route screen goal acceptance criteria.\n",
        )
        policy_only = run_inventory(policy_repo)
        check(policy_only["journey_doc_count"] == 0, "agent policy must not count as product journey truth")
        check(policy_only["product_doc_count"] == 0, "agent policy must not count as product documentation")

        skill_repo = tmp / "skill-repo"
        write(
            skill_repo / "skills" / "sample-skill" / "SKILL.md",
            """# Sample Skill

This operational skill guide mentions user journeys, workflow, product, overview, app, mobile, desktop, route, screen, goal, and acceptance so agents know what to ask.
""",
        )
        skill_inventory = run_inventory(skill_repo)
        check(skill_inventory["journey_doc_count"] == 0, "operational skill docs should not count as app journey docs")
        check(skill_inventory["product_doc_count"] == 0, "operational skill docs should not count as app product docs")

        skill_readme_repo = tmp / "skill-readme-repo"
        write(
            skill_readme_repo / "README.md",
            """# Holy Skills

This is a local curation repository for Codex skills.

- `skills/user-journey-docs-audit/`: checks whether docs describe app idea, users, journeys, UI priorities, edge cases, and acceptance criteria.
- `scripts/validate.py`: validates standalone skill-copy execution.
""",
        )
        skill_readme_inventory = run_inventory(skill_readme_repo)
        check(skill_readme_inventory["journey_doc_count"] == 0, "skill repo README should not count as app journey docs")
        check(skill_readme_inventory["product_doc_count"] == 0, "skill repo README should not count as app product docs")

        comment_journey_repo = tmp / "comment-journey"
        write(comment_journey_repo / "README.md", "# App\n\nA tool.\n")
        write(
            comment_journey_repo / "src" / "checkout.ts",
            "// User journey: shopper reviews cart, then confirms payment.\n"
            "// This onboarding workflow drives the primary purchase use case.\n"
            "export function checkout() { return true; }\n",
        )
        comment_journey = run_inventory(comment_journey_repo)
        check(comment_journey["source_comment_journey_hits"] >= 1, "journey comments in source should be detected")
        check(
            any("source comments" in signal for signal in comment_journey["missing_signals"]),
            "journeys documented only in source comments should raise a missing signal",
        )

        rst_repo = tmp / "rst-docs"
        write(rst_repo / "docs" / "guide.rst", "User Guide\n=========\n\nThe app helps users.\n")
        rst_inventory = run_inventory(rst_repo)
        check(rst_inventory["doc_count"] >= 1, "reStructuredText docs should be inventoried")

        headings = [
            "Coverage",
            "Confirmation Status",
            "Journey Findings",
            "Decision And Information Gaps",
            "Documentation Findings",
            "Interaction Affordance And Metadata Gaps",
            "Recommended Documentation Plan",
            "Readiness And Open Questions",
        ]
        good_report = tmp / "good-report.md"
        sections = []
        for heading in headings:
            body = "Confirmed from the user interview and docs with file evidence."
            if heading == "Coverage":
                body = "Journey status confirmed; documentation and source hints were inspected."
            elif heading == "Confirmation Status":
                body = "Confirmed from the user and repository documentation."
            elif heading == "Journey Findings":
                body = "| Journey | Status | Evidence |\n| --- | --- | --- |\n| Review health | confirmed | user interview |"
            elif heading == "Decision And Information Gaps":
                body = (
                    "Confirmed handoff checks: primary journey, usage percentage, risk if broken, initial viewport region roles, "
                    "continuation anchor, theme intent, review inputs, initial-viewport screenshot, full-page screenshot, and changed visual review."
                )
            elif heading == "Interaction Affordance And Metadata Gaps":
                body = "No gaps: activation targets, focus feedback, destination, disclosure lifecycle, detail access, scrollbar separation, stable dimensions, hover-copy, concise status, icon meaning, and passive metadata are explicitly documented."
            elif heading == "Readiness And Open Questions":
                body = "Confirmed readiness: 3/3 for journey definition with user evidence; no open questions."
            sections.append(f"## {heading}\n\n{body}\n")
        write(good_report, "\n".join(sections))
        run_report_verify(good_report, expect=0)

        no_interaction = tmp / "no-interaction-report.md"
        write(
            no_interaction,
            good_report.read_text(encoding="utf-8").replace(
                "No gaps: activation targets, focus feedback, destination, disclosure lifecycle, detail access, scrollbar separation, stable dimensions, hover-copy, concise status, icon meaning, and passive metadata are explicitly documented.",
                "No relevant interaction or message-metadata surfaces documented.",
            ),
        )
        run_report_verify(no_interaction, expect=0)

        missing_formal_handoff = tmp / "missing-formal-handoff.md"
        write(
            missing_formal_handoff,
            good_report.read_text(encoding="utf-8").replace("changed visual review", "ordinary visual inspection"),
        )
        run_report_verify(missing_formal_handoff, expect=1)

        missing_interaction = tmp / "missing-interaction.md"
        write(missing_interaction, good_report.read_text(encoding="utf-8").replace("## Interaction Affordance And Metadata Gaps", "## Generic UI Gaps"))
        run_report_verify(missing_interaction, expect=1)

        unconfirmed = tmp / "unconfirmed-report.md"
        write(
            unconfirmed,
            good_report.read_text(encoding="utf-8")
            .replace("Journey status confirmed; documentation and source hints were inspected.", "journey assumptions unconfirmed")
            .replace("Confirmed readiness: 3/3 for journey definition with user evidence; no open questions.", "Readiness cannot be confirmed; no questions recorded."),
        )
        run_report_verify(unconfirmed, expect=1)

        print("self-test ok")
        return 0
    finally:
        rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
