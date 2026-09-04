#!/usr/bin/env python3
"""Inventory documentation relevant to user journey completeness."""

from __future__ import annotations

import argparse
import json
import os
import re
from dataclasses import asdict, dataclass
from pathlib import Path, PurePosixPath


DOC_EXTENSIONS = {".md", ".mdx", ".markdown", ".rst", ".adoc", ".asciidoc"}
SOURCE_HINT_EXTENSIONS = {
    ".ts", ".tsx", ".js", ".jsx", ".vue", ".svelte", ".html", ".cshtml", ".razor",
    ".swift", ".xaml", ".axaml", ".kt", ".kts", ".dart", ".m", ".mm", ".qml",
}
COMMENT_LINE_RE = re.compile(r"^\s*(?://+|#+|\*+|/\*+|<!--)")
COMMENT_JOURNEY_TERMS = ("journey", "user flow", "workflow", "onboarding", "use case", "user story")


def count_comment_journey_hits(text: str) -> int:
    """Count code-comment lines that describe a user journey.

    Journeys are sometimes documented in JSDoc/inline comments rather than
    Markdown; without this an inventory would report 'no journey docs' even
    though the intent exists in source.
    """
    hits = 0
    for line in text.splitlines():
        if COMMENT_LINE_RE.match(line):
            lowered = line.lower()
            if any(term in lowered for term in COMMENT_JOURNEY_TERMS):
                hits += 1
    return hits
EXCLUDED_DIRS = {
    ".git",
    ".hg",
    ".svn",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
    "vendor",
}

JOURNEY_TERMS = {
    "journey",
    "workflow",
    "flow",
    "onboarding",
    "persona",
    "user",
    "route",
    "screen",
    "mobile",
    "desktop",
    "empty state",
    "error",
    "permission",
    "acceptance",
}
JOURNEY_STRUCTURE_TERMS = {
    "acceptance",
    "decision",
    "entry point",
    "failure",
    "goal",
    "primary action",
    "route",
    "screen sequence",
    "success",
    "trigger",
}
EXPLICIT_JOURNEY_TERMS = {"journey", "workflow", "user flow", "persona", "scenario"}
APP_IDEA_TERMS = {"purpose", "overview", "goal", "mission", "product", "app", "value", "problem"}
PRODUCT_STRUCTURE_TERMS = {
    "app idea",
    "goal",
    "mission",
    "problem",
    "purpose",
    "requirements",
    "target user",
    "users",
    "value",
}
UX_TERMS = {
    "navigation",
    "priority",
    "hierarchy",
    "accessibility",
    "responsive",
    "mobile",
    "empty",
    "loading",
    "error",
    "undo",
}
DECISION_MODEL_TERMS = {
    "action frequency",
    "decision data",
    "decision-making information",
    "journey decision model",
    "primary decision",
    "primary user goal",
    "required facts",
    "required information",
    "unconfirmed assumptions",
    "unresolved assumptions",
    "warning conditions",
    "warning/flag conditions",
}
INFORMATION_RELEVANCE_TERMS = {
    "action frequency",
    "conditional",
    "critical always",
    "critical-always",
    "debug",
    "decision importance",
    "expert only",
    "expert-only",
    "frequent action",
    "frequent actions",
    "importance",
    "information relevance",
    "occasional controls",
    "primary frequent",
    "primary-frequent",
    "rare detail",
    "rare controls",
    "rare-under-5-percent",
    "rare/admin/configuration controls",
    "relative importance",
    "secondary occasional",
    "secondary-occasional",
}
UI_HANDOFF_CONSTRAINT_TERMS = {
    "dom measurement",
    "evidence expectations",
    "mockup",
    "rendered state",
    "screenshot",
    "states to verify",
    "ui audit constraint",
    "ui audit constraints",
    "test mode",
    "ui audit handoff",
    "ui handoff constraints",
    "ui implementation audit",
    "viewport measurement",
    "visual audit",
}
FORMAL_PRIORITY_TERMS = {
    "primary journey",
    "usage percentage",
    "frequency percent",
    "priority override reason",
    "risk if broken",
}
FORMAL_HIERARCHY_TERMS = {
    "initial viewport",
    "primary-content",
    "workflow-surface",
    "blocking-alert",
    "supporting region",
}
FORMAL_CONTINUATION_TERMS = {
    "continuation anchor",
    "focus enters",
    "document scroll",
    "expected destination route",
    "without scrolling",
}
FORMAL_THEME_TERMS = {
    "theme intent",
    "light, dark, or mixed",
    "declared theme",
}
FORMAL_REVIEW_INPUT_TERMS = {
    "review inputs",
    "ui code, styles, tokens, fonts, and assets",
    "implementation-input ownership",
    "changed visual review",
}
FORMAL_SCREENSHOT_TERMS = {
    "initial-viewport screenshot",
    "full-page screenshot",
    "screenshot pair",
}
FEATURE_TERMS = {
    "capability",
    "capabilities",
    "feature",
    "features",
    "feature inventory",
    "requirement",
    "requirements",
}
UI_ELEMENT_TERMS = {
    "badge",
    "banner",
    "button",
    "control",
    "controls",
    "field",
    "form",
    "menu",
    "screen",
    "state",
    "toast",
    "ui element",
    "ui elements",
}
IMPLEMENTATION_EXPECTATION_TERMS = {
    "api",
    "data path",
    "handler",
    "implementation expectation",
    "implementation expectations",
    "permission",
    "persistence",
    "state change",
    "validation",
}
TEST_EXPECTATION_TERMS = {
    "acceptance criteria",
    "component test",
    "e2e",
    "fixture",
    "qa",
    "test",
    "test expectation",
    "test expectations",
    "test mode",
    "unit test",
    "visual test",
}
LOW_FREQUENCY_CONTROL_TERMS = {
    "admin",
    "configuration",
    "filter",
    "filters",
    "settings",
}
PRIMARY_CONTENT_TERMS = {
    "chart",
    "dashboard",
    "decision",
    "metric",
    "metrics",
    "primary content",
    "summary",
}
MOBILE_SCREEN_TERMS = {
    "mobile",
    "narrow",
    "phone",
    "responsive",
    "screen",
    "viewport",
}
UI_INTENT_TERMS = {
    "command center",
    "compact",
    "dashboard",
    "dense",
    "expert ui",
    "overview",
}
VISIBLE_UI_PRESCRIPTION_TERMS = {
    "always visible",
    "default visible",
    "must display",
    "must list",
    "must show",
    "required visible decision evidence",
    "required visible evidence",
    "show by default",
    "shown by default",
    "visible by default",
    "visible decision evidence",
}
DETAIL_HEAVY_TERMS = {
    "debug",
    "detail",
    "details",
    "evidence",
    "gap breakdown",
    "metadata",
    "next action",
    "owner",
    "raw",
    "raw log",
    "raw status",
    "secondary detail",
    "severity summary",
    "stage",
    "status summary",
}
INTERACTION_SURFACE_TERMS = {
    "badge",
    "badges",
    "flag",
    "flags",
    "message",
    "messages",
    "panel",
    "panels",
    "popover",
    "popovers",
    "flyout",
    "flyouts",
    "row",
    "rows",
    "tool block",
    "tool blocks",
    "result block",
    "result blocks",
    "expand",
    "collapse",
    "disclosure",
}
DISCLOSURE_MODEL_TERMS = {
    "click",
    "click target",
    "clickable row",
    "detail path",
    "detail state",
    "dialog",
    "disclosure",
    "drill-down",
    "drill down",
    "drawer",
    "expand",
    "expansion",
    "focus",
    "hover",
    "interactive badge",
    "interactive badges",
    "on demand",
    "popover",
    "row selection",
    "whole row",
    "whole-row",
    "tooltip",
}
TRANSIENT_DISCLOSURE_TERMS = {
    "auto collapse",
    "auto-collapse",
    "close on",
    "dismiss",
    "dismissal",
    "focus loss",
    "idle",
    "leave timer",
    "lifecycle",
    "outside click",
    "persistent while hovered",
    "timeout",
}
NAVIGATION_SURFACE_TERMS = {
    "destination",
    "go to",
    "jump to",
    "navigate",
    "navigation",
    "open screen",
    "scroll to",
}
NAVIGATION_AFFORDANCE_TERMS = {
    "cursor",
    "focus affordance",
    "pointer",
    "predictable destination",
    "target surface",
}
COPY_CONTROL_TERMS = {
    "copy",
    "copy button",
    "copy control",
    "clipboard",
}
HOVER_COPY_TERMS = {
    "hover copy",
    "hover-revealed",
    "hover revealed",
    "reachable",
    "stable position",
    "stay visible",
}
STATUS_SUMMARY_TERMS = {
    "concise status",
    "duplicate status",
    "duration",
    "error count",
    "result summary",
    "status summary",
    "tool status",
}
STATUS_SUMMARY_MODEL_TERMS = {
    "default summary",
    "detail only",
    "hidden by default",
    "moved to detail",
    "only when",
    "success indicator",
}
MESSAGE_METADATA_TERMS = {
    "author",
    "authorship",
    "message metadata",
    "metadata",
    "routing label",
    "sender",
    "sender label",
    "timestamp",
    "unselectable",
}
LOW_IMPORTANCE_CLASSIFICATION_TERMS = {
    "conditional",
    "debug",
    "expert-only",
    "expert only",
    "low-frequency",
    "occasional",
    "rare",
    "rare-under-5-percent",
    "secondary",
    "secondary-occasional",
}
ROUTE_HINT_RE = re.compile(r"(?:route|path|href|to)\s*[:=]\s*[\"']([^\"']+)[\"']", re.IGNORECASE)
VISIBLE_TEXT_PATTERNS = (
    re.compile(r">[^<]{3,}<|aria-label\s*=|placeholder\s*=|title\s*=", re.IGNORECASE),
    re.compile(
        r"\b(?:Text|Label|Button|Toggle|TextField|SecureField|navigationTitle|accessibilityLabel)"
        r"\s*\(\s*[\"'][^\"']{2,}[\"']",
    ),
    re.compile(
        r"\b(?:Text|Content|Header|Title|Placeholder|AutomationProperties\.Name)"
        r"\s*=\s*[\"'][^\"']{2,}[\"']",
        re.IGNORECASE,
    ),
)
HEADING_RE = re.compile(r"^(#{1,6})\s+(.+?)\s*$")
GITMODULES_PATH_RE = re.compile(r"^\s*path\s*=\s*(.+?)\s*$", re.MULTILINE)


@dataclass
class DocRecord:
    path: str
    kind: str
    headings: list[str]
    app_idea_hits: int
    journey_hits: int
    ux_hits: int
    decision_model_hits: int
    information_relevance_hits: int
    ui_handoff_constraint_hits: int
    prescriptive_ui_risk: bool
    likely_journey_doc: bool
    likely_product_doc: bool
    likely_decision_model_doc: bool
    likely_information_relevance_doc: bool
    likely_ui_handoff_constraint_doc: bool


@dataclass
class SourceHint:
    path: str
    routes: list[str]
    visible_text_hits: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Inventory docs for user journey documentation audits.")
    parser.add_argument("--repo", default=".", help="Repository root. Defaults to cwd.")
    parser.add_argument("--json", action="store_true", help="Emit JSON instead of Markdown.")
    return parser.parse_args()


def skip_dir(path: Path) -> bool:
    return any(part in EXCLUDED_DIRS for part in path.parts)


def discover_submodule_paths(repo: Path) -> set[str]:
    gitmodules = repo / ".gitmodules"
    if not gitmodules.is_file():
        return set()
    try:
        text = gitmodules.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return set()
    return {PurePosixPath(match.group(1).strip()).as_posix().strip("/") for match in GITMODULES_PATH_RE.finditer(text)}


def is_nested_repo_dir(path: Path, repo: Path) -> bool:
    if path == repo:
        return False
    return (path / ".git").exists()


def is_under_path(rel_path: str, parent: str) -> bool:
    return rel_path == parent or rel_path.startswith(parent.rstrip("/") + "/")


def is_excluded_submodule_path(rel_path: str, submodules: set[str]) -> bool:
    return any(is_under_path(rel_path, item) for item in submodules if item)


def iter_files(repo: Path):
    submodules = discover_submodule_paths(repo)
    for root, dirs, files in os.walk(repo):
        root_path = Path(root)
        kept_dirs: list[str] = []
        for item in dirs:
            candidate = root_path / item
            rel_candidate = candidate.relative_to(repo).as_posix()
            if item in EXCLUDED_DIRS:
                continue
            if is_excluded_submodule_path(rel_candidate, submodules):
                continue
            if is_nested_repo_dir(candidate, repo):
                continue
            kept_dirs.append(item)
        dirs[:] = kept_dirs
        if skip_dir(root_path.relative_to(repo)):
            continue
        for filename in files:
            path = root_path / filename
            rel = path.relative_to(repo)
            rel_text = rel.as_posix()
            if skip_dir(rel) or is_excluded_submodule_path(rel_text, submodules):
                continue
            yield path


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return ""


def count_terms(text: str, terms: set[str]) -> int:
    lowered = text.lower()
    total = 0
    for term in terms:
        pattern = r"(?<![a-z0-9])" + re.escape(term.lower()).replace(r"\ ", r"\s+") + r"(?![a-z0-9])"
        total += len(re.findall(pattern, lowered))
    return total


def has_any_term(text: str, terms: set[str]) -> bool:
    return count_terms(text, terms) > 0


def journey_structure_score(text: str) -> int:
    return sum(1 for term in JOURNEY_STRUCTURE_TERMS if count_terms(text, {term}))


def is_operational_skill_doc(rel_path: str) -> bool:
    parts = PurePosixPath(rel_path).parts
    return len(parts) >= 2 and parts[0] == "skills"


def is_policy_or_governance_doc(rel_path: str) -> bool:
    path = PurePosixPath(rel_path)
    name = path.name.lower()
    decision_detail = (
        len(path.parts) == 2
        and path.parts[0].lower() == "decisiondetails"
        and re.fullmatch(r"d-\d{8}-\d{2}\.md", name) is not None
    )
    return name in {"agents.md", "claude.md", "decisionhistory.md"} or decision_detail


def is_operational_docs_repo_text(text: str) -> bool:
    lowered = text.lower()
    markers = (
        "agent skill",
        "agent skills",
        "skill directory",
        "skill directories",
        "skills/",
        "full_repo_harness",
        "scripts/validate.py",
    )
    return any(marker in lowered for marker in markers)


def is_operational_doc(rel_path: str, text: str) -> bool:
    return (
        is_operational_skill_doc(rel_path)
        or is_policy_or_governance_doc(rel_path)
        or is_operational_docs_repo_text(text)
    )


def is_likely_journey_doc(rel_path: str, text: str, journey_hits: int) -> bool:
    if is_operational_doc(rel_path, text):
        return False
    lowered = text.lower()
    structural_score = journey_structure_score(lowered)
    explicit = has_any_term(lowered, EXPLICIT_JOURNEY_TERMS)
    return (explicit and structural_score >= 1) or (journey_hits >= 3 and structural_score >= 2)


def is_likely_product_doc(rel_path: str, text: str, app_hits: int) -> bool:
    if is_operational_doc(rel_path, text):
        return False
    lowered = text.lower()
    structural_score = sum(1 for term in PRODUCT_STRUCTURE_TERMS if count_terms(lowered, {term}))
    has_product_language = has_any_term(lowered, {"product", "requirements", "app idea", "overview"})
    return (has_product_language and structural_score >= 2) or app_hits >= 4


def is_likely_decision_model_doc(rel_path: str, text: str) -> bool:
    if is_operational_doc(rel_path, text):
        return False
    lowered = text.lower()
    explicit_model = has_any_term(lowered, {"journey decision model"})
    has_goal = has_any_term(lowered, {"primary user goal", "primary goal", "goal"})
    has_decision = has_any_term(lowered, {"primary decision", "decision data", "decision-making information"})
    has_required_facts = has_any_term(lowered, {"required facts", "required information", "warning conditions", "warning/flag conditions"})
    has_action_frequency = has_any_term(
        lowered,
        {"action frequency", "frequent action", "frequent actions", "occasional controls", "rare controls", "rare/admin/configuration controls"},
    )
    return (explicit_model and has_goal and has_decision and has_required_facts) or (
        has_goal and has_decision and has_required_facts and has_action_frequency
    )


def is_likely_information_relevance_doc(rel_path: str, text: str) -> bool:
    if is_operational_doc(rel_path, text):
        return False
    lowered = text.lower()
    explicit_relevance = has_any_term(lowered, {"information relevance", "critical-always", "primary-frequent", "secondary-occasional", "rare-under-5-percent"})
    has_decision = has_any_term(lowered, {"primary decision", "decision data", "required facts", "required information"})
    has_frequency = has_any_term(lowered, {"action frequency", "frequent", "occasional", "rare", "conditional"})
    has_importance_classes = has_any_term(lowered, {"critical-always", "primary-frequent", "secondary-occasional", "rare-under-5-percent", "expert-only", "debug"})
    return (explicit_relevance and has_importance_classes) or (has_decision and has_frequency and has_importance_classes)


def is_likely_ui_handoff_constraint_doc(rel_path: str, text: str) -> bool:
    if is_operational_doc(rel_path, text):
        return False
    lowered = text.lower()
    return has_any_term(lowered, {"ui handoff constraints", "ui implementation audit", "ui audit handoff"}) or (
        has_any_term(lowered, {"mockup", "screenshot", "viewport measurement", "rendered state", "states to verify"})
        and is_likely_decision_model_doc(rel_path, text)
        and is_likely_information_relevance_doc(rel_path, text)
    )


def has_prescriptive_ui_risk(rel_path: str, text: str) -> bool:
    if is_operational_doc(rel_path, text):
        return False
    lowered = text.lower()
    has_visible_prescription = has_any_term(lowered, VISIBLE_UI_PRESCRIPTION_TERMS)
    has_detail_load = has_any_term(lowered, DETAIL_HEAVY_TERMS)
    has_surface_intent = has_any_term(lowered, UI_INTENT_TERMS) or "required visible" in lowered
    if not (has_visible_prescription and has_detail_load and has_surface_intent):
        return False

    segments = [segment.strip() for segment in re.split(r"(?:\n\s*\n|[.!?]\s+)", lowered) if segment.strip()]
    for segment in segments:
        if re.search(r"\b(?:not|never|does not|do not|without)\b[^.\n]{0,80}\balways visible\b", segment):
            continue
        if has_any_term(segment, VISIBLE_UI_PRESCRIPTION_TERMS) and has_any_term(segment, DETAIL_HEAVY_TERMS):
            if not has_any_term(segment, DISCLOSURE_MODEL_TERMS):
                return True
            if has_any_term(lowered, LOW_IMPORTANCE_CLASSIFICATION_TERMS) and not has_any_term(segment, {"critical-always", "primary-frequent"}):
                return True
    return False


def classify_doc(rel_path: str, text: str) -> str:
    lowered = rel_path.lower()
    if is_policy_or_governance_doc(rel_path):
        return "policy-or-decision-history"
    if "readme" in lowered:
        return "readme"
    if any(term in lowered for term in ("journey", "workflow", "flow", "persona", "product", "spec", "feature", "requirement")):
        return "product-doc"
    if any(term in lowered for term in ("architecture", "design", "adr")):
        return "architecture-doc"
    if "test" in lowered or "qa" in lowered:
        return "test-doc"
    if is_likely_journey_doc(rel_path, text, count_terms(text, JOURNEY_TERMS)):
        return "journey-candidate"
    return "doc"


def doc_record(repo: Path, path: Path) -> DocRecord:
    text = read_text(path)
    rel = path.relative_to(repo).as_posix()
    headings = [match.group(2).strip() for line in text.splitlines() if (match := HEADING_RE.match(line))]
    app_hits = count_terms(text, APP_IDEA_TERMS)
    journey_hits = count_terms(text, JOURNEY_TERMS)
    ux_hits = count_terms(text, UX_TERMS)
    decision_model_hits = count_terms(text, DECISION_MODEL_TERMS)
    relevance_hits = count_terms(text, INFORMATION_RELEVANCE_TERMS)
    handoff_hits = count_terms(text, UI_HANDOFF_CONSTRAINT_TERMS)
    prescriptive_ui_risk = has_prescriptive_ui_risk(rel, text)
    likely_journey = is_likely_journey_doc(rel, text, journey_hits)
    likely_product = is_likely_product_doc(rel, text, app_hits)
    likely_decision_model = is_likely_decision_model_doc(rel, text)
    likely_relevance = is_likely_information_relevance_doc(rel, text)
    likely_handoff = is_likely_ui_handoff_constraint_doc(rel, text)
    return DocRecord(
        path=rel,
        kind=classify_doc(rel, text),
        headings=headings[:20],
        app_idea_hits=app_hits,
        journey_hits=journey_hits,
        ux_hits=ux_hits,
        decision_model_hits=decision_model_hits,
        information_relevance_hits=relevance_hits,
        ui_handoff_constraint_hits=handoff_hits,
        prescriptive_ui_risk=prescriptive_ui_risk,
        likely_journey_doc=likely_journey,
        likely_product_doc=likely_product,
        likely_decision_model_doc=likely_decision_model,
        likely_information_relevance_doc=likely_relevance,
        likely_ui_handoff_constraint_doc=likely_handoff,
    )


def source_hint(repo: Path, path: Path) -> SourceHint | None:
    text = read_text(path)
    routes = sorted(set(match.group(1) for match in ROUTE_HINT_RE.finditer(text)))[:30]
    visible_text_hits = sum(len(pattern.findall(text)) for pattern in VISIBLE_TEXT_PATTERNS)
    if not routes and not visible_text_hits:
        return None
    return SourceHint(path=path.relative_to(repo).as_posix(), routes=routes, visible_text_hits=visible_text_hits)


def build_inventory(repo: Path) -> dict:
    docs: list[DocRecord] = []
    hints: list[SourceHint] = []
    source_comment_journey_hits = 0
    for path in iter_files(repo):
        if path.suffix.lower() in DOC_EXTENSIONS:
            docs.append(doc_record(repo, path))
        elif path.suffix.lower() in SOURCE_HINT_EXTENSIONS:
            hint = source_hint(repo, path)
            if hint:
                hints.append(hint)
            source_comment_journey_hits += count_comment_journey_hits(read_text(path))
    missing_signals: list[str] = []
    ui_implementation_risk_signals: list[str] = []
    if not any(doc.likely_product_doc for doc in docs):
        missing_signals.append("No strong app idea/product overview documentation detected.")
    if not any(doc.likely_journey_doc for doc in docs):
        missing_signals.append("No strong user journey/workflow/persona documentation detected.")
        if source_comment_journey_hits > 0:
            missing_signals.append(
                f"Journey descriptions appear only in source comments ({source_comment_journey_hits} hit(s)); "
                "move them into product/journey docs so they can be reviewed and kept current."
            )
    if sum(doc.ux_hits for doc in docs) < 3:
        missing_signals.append("Very little UI/UX hierarchy, navigation, responsive, or accessibility documentation detected.")
    decision_model_doc_count = sum(1 for doc in docs if doc.likely_decision_model_doc)
    information_relevance_doc_count = sum(1 for doc in docs if doc.likely_information_relevance_doc)
    ui_handoff_constraint_doc_count = sum(1 for doc in docs if doc.likely_ui_handoff_constraint_doc)
    prescriptive_ui_risk_doc_count = sum(1 for doc in docs if doc.prescriptive_ui_risk)
    doc_texts = []
    for doc in docs:
        text = read_text(repo / doc.path).lower()
        if not is_operational_doc(doc.path, text):
            doc_texts.append(text)
    has_mobile_screen_docs = any(has_any_term(text, MOBILE_SCREEN_TERMS) for text in doc_texts)
    has_low_frequency_controls = any(has_any_term(text, LOW_FREQUENCY_CONTROL_TERMS) for text in doc_texts)
    has_primary_content = any(has_any_term(text, PRIMARY_CONTENT_TERMS) for text in doc_texts)
    has_ui_intent_terms = any(has_any_term(text, UI_INTENT_TERMS) for text in doc_texts)
    has_interaction_surface_terms = any(has_any_term(text, INTERACTION_SURFACE_TERMS) for text in doc_texts)
    has_interaction_access_terms = any(has_any_term(text, DISCLOSURE_MODEL_TERMS) for text in doc_texts)
    has_transient_disclosure_terms = any(has_any_term(text, TRANSIENT_DISCLOSURE_TERMS) for text in doc_texts)
    has_navigation_surface_terms = any(has_any_term(text, NAVIGATION_SURFACE_TERMS) for text in doc_texts)
    has_navigation_affordance_terms = any(has_any_term(text, NAVIGATION_AFFORDANCE_TERMS) for text in doc_texts)
    has_copy_control_terms = any(has_any_term(text, COPY_CONTROL_TERMS) for text in doc_texts)
    has_hover_copy_terms = any(has_any_term(text, HOVER_COPY_TERMS) for text in doc_texts)
    has_status_summary_terms = any(has_any_term(text, STATUS_SUMMARY_TERMS) for text in doc_texts)
    has_status_summary_model_terms = any(has_any_term(text, STATUS_SUMMARY_MODEL_TERMS) for text in doc_texts)
    has_message_metadata_terms = any(has_any_term(text, MESSAGE_METADATA_TERMS) for text in doc_texts)
    has_feature_inventory = any(has_any_term(text, FEATURE_TERMS) for text in doc_texts)
    has_ui_element_inventory = any(has_any_term(text, UI_ELEMENT_TERMS) for text in doc_texts)
    has_implementation_expectations = any(has_any_term(text, IMPLEMENTATION_EXPECTATION_TERMS) for text in doc_texts)
    has_test_expectations = any(has_any_term(text, TEST_EXPECTATION_TERMS) for text in doc_texts)
    formal_handoff_parts = {
        "priority": any(has_any_term(text, FORMAL_PRIORITY_TERMS) for text in doc_texts),
        "hierarchy": any(has_any_term(text, FORMAL_HIERARCHY_TERMS) for text in doc_texts),
        "continuation": any(has_any_term(text, FORMAL_CONTINUATION_TERMS) for text in doc_texts),
        "theme": any(has_any_term(text, FORMAL_THEME_TERMS) for text in doc_texts),
        "review_inputs": any(has_any_term(text, FORMAL_REVIEW_INPUT_TERMS) for text in doc_texts),
        "screenshots": any(has_any_term(text, FORMAL_SCREENSHOT_TERMS) for text in doc_texts),
    }
    has_formal_verification_handoff = all(formal_handoff_parts.values())
    if docs and not has_feature_inventory:
        missing_signals.append("No complete feature inventory documentation detected.")
    if docs and not has_ui_element_inventory:
        missing_signals.append("No required UI element inventory documentation detected.")
    if docs and not has_implementation_expectations:
        missing_signals.append("No implementation expectation documentation detected.")
    if docs and not has_test_expectations:
        missing_signals.append("No test expectation documentation detected.")
    if docs and not (
        has_feature_inventory and has_ui_element_inventory and has_implementation_expectations and has_test_expectations
    ):
        ui_implementation_risk_signals.append(
            "Docs do not fully define required features, UI elements, implementation expectations, and test expectations."
        )
    if docs and decision_model_doc_count == 0:
        missing_signals.append("No journey decision model documentation detected.")
        ui_implementation_risk_signals.append(
            "Docs do not define primary goal, primary decision, required facts, warning/flag conditions, action frequency, rare details, and unresolved assumptions for UI implementation."
        )
    if docs and information_relevance_doc_count == 0:
        missing_signals.append("No information relevance inventory documentation detected.")
        ui_implementation_risk_signals.append(
            "Docs do not classify decision information, warnings, actions, and details as critical-always, primary-frequent, secondary-occasional, rare, conditional, debug, or expert-only."
        )
    if has_mobile_screen_docs and has_low_frequency_controls and has_primary_content and information_relevance_doc_count == 0:
        ui_implementation_risk_signals.append(
            "Docs mention constrained screens plus settings/filters/configuration and primary content, but do not define the decision/relevance model; the UI audit must judge rendered placement rather than inheriting a layout guess."
        )
    if has_ui_intent_terms and (decision_model_doc_count == 0 or information_relevance_doc_count == 0):
        ui_implementation_risk_signals.append(
            "Docs use UI intent terms such as dense, dashboard, command center, overview, compact, or expert UI without defining the decisions, relevance, action frequency, and assumptions behind that intent."
        )
    if docs and has_interaction_surface_terms and not has_interaction_access_terms:
        missing_signals.append("No interaction access model detected for badges, flags, rows, messages, or disclosure surfaces.")
        ui_implementation_risk_signals.append(
            "Docs mention badges, flags, messages, rows, tool/result blocks, or disclosure surfaces without defining click/hover/focus targets, popover/detail access, or stable expanded/collapsed behavior."
        )
    if docs and has_interaction_surface_terms and not has_transient_disclosure_terms:
        ui_implementation_risk_signals.append(
            "Docs mention interactive or disclosure surfaces without defining a transient panel lifecycle such as explicit close, outside click, focus loss, idle/leave timeout, or documented persistence."
        )
    if docs and has_navigation_surface_terms and not has_navigation_affordance_terms:
        ui_implementation_risk_signals.append(
            "Docs mention navigation or cross-surface jumps without defining predictable destinations and pointer/focus affordance for navigational elements."
        )
    if docs and has_copy_control_terms and not has_hover_copy_terms:
        ui_implementation_risk_signals.append(
            "Docs mention copy or clipboard controls without defining whether copy affordances are hidden until hover/focus, where they appear, and how they remain reachable."
        )
    if docs and has_status_summary_terms and not has_status_summary_model_terms:
        ui_implementation_risk_signals.append(
            "Docs mention tool/result/status summaries without defining which status, error count, duration, success, or severity signals belong in the concise default state versus detail."
        )
    if docs and has_message_metadata_terms and information_relevance_doc_count == 0:
        ui_implementation_risk_signals.append(
            "Docs mention message metadata such as sender labels, routing labels, timestamps, or authorship without classifying whether it is decision content or passive metadata."
        )
    if prescriptive_ui_risk_doc_count:
        ui_implementation_risk_signals.append(
            "Docs appear to turn UI handoff evidence into always-visible layout requirements for lower-importance information; classify decision importance and define inline, hint, selection, expansion, drawer, modal, or detail-view access before treating the docs as UI-audit ready."
        )
    if hints and decision_model_doc_count == 0:
        ui_implementation_risk_signals.append(
            "Source hints expose UI surfaces, but docs do not provide a journey decision model; source hints must not substitute for product truth."
        )
    if docs and not has_formal_verification_handoff:
        missing_parts = sorted(name for name, present in formal_handoff_parts.items() if not present)
        missing_signals.append("No complete formal Web UI verification handoff documentation detected.")
        ui_implementation_risk_signals.append(
            "Docs do not completely define primary journey frequency/risk, initial-viewport region roles, visible/focused continuation, light/dark/mixed theme intent, implementation review-input ownership, and initial/full-page changed-review evidence. "
            f"Missing handoff parts: {', '.join(missing_parts)}."
        )
    ui_audit_handoff_ready = bool(
        decision_model_doc_count
        and information_relevance_doc_count
        and ui_handoff_constraint_doc_count
        and has_formal_verification_handoff
        and not prescriptive_ui_risk_doc_count
        and (not has_interaction_surface_terms or (has_interaction_access_terms and has_transient_disclosure_terms))
        and (not has_navigation_surface_terms or has_navigation_affordance_terms)
        and (not has_copy_control_terms or has_hover_copy_terms)
        and (not has_status_summary_terms or has_status_summary_model_terms)
    )
    return {
        "repo_root": str(repo),
        "doc_count": len(docs),
        "journey_doc_count": sum(1 for doc in docs if doc.likely_journey_doc),
        "product_doc_count": sum(1 for doc in docs if doc.likely_product_doc),
        "decision_model_doc_count": decision_model_doc_count,
        "information_relevance_doc_count": information_relevance_doc_count,
        "ui_handoff_constraint_doc_count": ui_handoff_constraint_doc_count,
        "prescriptive_ui_risk_doc_count": prescriptive_ui_risk_doc_count,
        "ui_audit_handoff_ready": ui_audit_handoff_ready,
        "has_feature_inventory": has_feature_inventory,
        "has_ui_element_inventory": has_ui_element_inventory,
        "has_implementation_expectations": has_implementation_expectations,
        "has_test_expectations": has_test_expectations,
        "has_formal_verification_handoff": has_formal_verification_handoff,
        "formal_verification_handoff_parts": formal_handoff_parts,
        "has_interaction_access_model": bool(
            (not has_interaction_surface_terms or (has_interaction_access_terms and has_transient_disclosure_terms))
            and (not has_navigation_surface_terms or has_navigation_affordance_terms)
            and (not has_copy_control_terms or has_hover_copy_terms)
            and (not has_status_summary_terms or has_status_summary_model_terms)
        ),
        "source_hint_count": len(hints),
        "source_comment_journey_hits": source_comment_journey_hits,
        "docs": [asdict(doc) for doc in sorted(docs, key=lambda item: item.path)],
        "source_hints": [asdict(hint) for hint in sorted(hints, key=lambda item: item.path)[:100]],
        "missing_signals": missing_signals,
        "ui_implementation_risk_signals": ui_implementation_risk_signals,
    }


def print_markdown(inventory: dict) -> None:
    print(f"# Journey Documentation Inventory\n")
    print(f"Repo root: `{inventory['repo_root']}`")
    print(f"Docs: **{inventory['doc_count']}**")
    print(f"Likely journey docs: **{inventory['journey_doc_count']}**")
    print(f"Likely product docs: **{inventory['product_doc_count']}**")
    print(f"Journey decision model docs: **{inventory['decision_model_doc_count']}**")
    print(f"Information relevance docs: **{inventory['information_relevance_doc_count']}**")
    print(f"UI handoff constraint docs: **{inventory['ui_handoff_constraint_doc_count']}**")
    print(f"Prescriptive UI risk docs: **{inventory['prescriptive_ui_risk_doc_count']}**")
    print(f"Feature inventory present: **{str(inventory['has_feature_inventory']).lower()}**")
    print(f"UI element inventory present: **{str(inventory['has_ui_element_inventory']).lower()}**")
    print(f"Implementation expectations present: **{str(inventory['has_implementation_expectations']).lower()}**")
    print(f"Test expectations present: **{str(inventory['has_test_expectations']).lower()}**")
    print(f"UI audit handoff ready: **{str(inventory['ui_audit_handoff_ready']).lower()}**")
    print(f"Source hint files: **{inventory['source_hint_count']}**\n")
    if inventory["missing_signals"]:
        print("## Missing Signals")
        for item in inventory["missing_signals"]:
            print(f"- {item}")
        print()
    if inventory["ui_implementation_risk_signals"]:
        print("## UI Implementation Risk Signals")
        for item in inventory["ui_implementation_risk_signals"]:
            print(f"- {item}")
        print()
    print("## Docs")
    print("| File | Kind | Headings | Journey hits | UX hits | Decision model hits | Relevance hits | Handoff hits |")
    print("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |")
    for doc in inventory["docs"]:
        headings = "; ".join(doc["headings"][:5]) or "None"
        print(
            f"| `{doc['path']}` | {doc['kind']} | {headings.replace('|', '/')} | "
            f"{doc['journey_hits']} | {doc['ux_hits']} | {doc['decision_model_hits']} | {doc['information_relevance_hits']} | {doc['ui_handoff_constraint_hits']} |"
        )


def main() -> int:
    args = parse_args()
    repo = Path(args.repo).expanduser().resolve()
    if not repo.is_dir():
        raise SystemExit(f"Repo path is not a directory: {repo}")
    inventory = build_inventory(repo)
    if args.json:
        print(json.dumps(inventory, indent=2, sort_keys=True))
    else:
        print_markdown(inventory)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
