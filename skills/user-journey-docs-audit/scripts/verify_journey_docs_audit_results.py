#!/usr/bin/env python3
"""Verify the final user-journey documentation audit report contract."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


REQUIRED_HEADINGS = [
    "Coverage",
    "Confirmation Status",
    "Journey Findings",
    "Decision And Information Gaps",
    "Documentation Findings",
    "Interaction Affordance And Metadata Gaps",
    "Recommended Documentation Plan",
    "Readiness And Open Questions",
]
PLACEHOLDER_RE = re.compile(r"\b(?:todo|tbd|placeholder|fill\s+this|lorem\s+ipsum)\b|<[^>]+>", re.IGNORECASE)
INTERACTION_TERMS = (
    "activation target",
    "focus",
    "destination",
    "disclosure lifecycle",
    "detail access",
    "scrollbar",
    "stable dimension",
    "hover-copy",
    "concise status",
    "icon meaning",
    "passive metadata",
)
FORMAL_WEB_HANDOFF_TERMS = (
    "primary journey",
    "usage percentage",
    "risk if broken",
    "initial viewport",
    "continuation anchor",
    "theme intent",
    "review inputs",
    "initial-viewport screenshot",
    "full-page screenshot",
    "changed visual review",
)
FORMAL_WEB_NOT_APPLICABLE = "formal web ui verification handoff not applicable: no web ui surfaces documented."


def sections(text: str) -> tuple[list[str], dict[str, str]]:
    matches = list(re.finditer(r"^##\s+(.+?)\s*$", text, re.MULTILINE))
    order: list[str] = []
    bodies: dict[str, str] = {}
    for index, match in enumerate(matches):
        heading = match.group(1).strip()
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        order.append(heading)
        bodies[heading] = text[match.end():end].strip()
    return order, bodies


def verify(text: str) -> list[str]:
    issues: list[str] = []
    order, bodies = sections(text)
    if order != REQUIRED_HEADINGS:
        missing = [item for item in REQUIRED_HEADINGS if item not in order]
        unexpected = [item for item in order if item not in REQUIRED_HEADINGS]
        issues.append(f"top-level headings must exactly match the required order; missing={missing}; unexpected={unexpected}")
    for heading in REQUIRED_HEADINGS:
        body = bodies.get(heading, "")
        if not body:
            issues.append(f"section is missing or empty: {heading}")
        elif PLACEHOLDER_RE.search(body):
            issues.append(f"section contains placeholder text: {heading}")

    coverage = bodies.get("Coverage", "")
    confirmation = bodies.get("Confirmation Status", "")
    inventory = bodies.get("Journey Findings", "")
    readiness = bodies.get("Readiness And Open Questions", "")
    interaction = bodies.get("Interaction Affordance And Metadata Gaps", "").lower()

    confirmation_status = re.search(r"\b(confirmed|unconfirmed|unavailable|declined)\b", confirmation, re.IGNORECASE)
    confirmation_source = re.search(r"\b(user|docs?|documentation)\b", confirmation, re.IGNORECASE)
    if not confirmation_status or not confirmation_source:
        issues.append("Confirmation Status must state confirmed/unconfirmed status and identify user or documentation as the source")
    if inventory and not re.search(r"\b(confirmed|draft-needs-user-confirmation|draft|rejected|blocked)\b", inventory, re.IGNORECASE):
        issues.append("Journey Findings must label each journey confirmed, draft-needs-user-confirmation, rejected, or blocked")

    unconfirmed = "journey assumptions unconfirmed" in coverage.lower()
    if unconfirmed:
        if "journey assumptions unconfirmed" not in readiness.lower():
            issues.append("Readiness And Open Questions must repeat 'journey assumptions unconfirmed' when coverage is unconfirmed")
    elif not re.search(r"\bconfirmed\b", coverage, re.IGNORECASE):
        issues.append("Coverage must label journey assumptions confirmed or use the exact unconfirmed phrase")

    no_surface = "no relevant interaction or message-metadata surfaces documented." in interaction
    if not no_surface:
        missing_interaction = [term for term in INTERACTION_TERMS if term not in interaction]
        if missing_interaction:
            issues.append(f"Interaction Affordance And Metadata Gaps omits required checks: {missing_interaction}")
    normalized_report = text.lower()
    if FORMAL_WEB_NOT_APPLICABLE not in normalized_report:
        missing_handoff = [term for term in FORMAL_WEB_HANDOFF_TERMS if term not in normalized_report]
        if missing_handoff:
            issues.append(f"report omits formal Web UI verification handoff checks: {missing_handoff}")
    return issues


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    if not args.report.is_file():
        print(f"ERROR: report not found: {args.report}")
        return 1
    issues = verify(args.report.read_text(encoding="utf-8", errors="replace"))
    if issues:
        for issue in issues:
            print(f"ERROR: {issue}")
        return 1
    print("journey-docs audit report verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
