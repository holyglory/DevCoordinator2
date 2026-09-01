#!/usr/bin/env python3
"""Verify returned reports for the ui-implementation-audit skill."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
REPO_ROOT = Path(__file__).resolve().parents[3]
VENDOR_ROOT = SCRIPT_DIR / "_vendor"
DEV_SKILL_DIR = (REPO_ROOT / "skills" / "ui-implementation-audit").resolve()
running_in_dev_repo = DEV_SKILL_DIR == SKILL_DIR.resolve() and (REPO_ROOT / "full_repo_harness" / "verify_common.py").is_file()

path_roots = [REPO_ROOT, VENDOR_ROOT] if running_in_dev_repo else [VENDOR_ROOT]
for root in reversed([item for item in path_roots if item.is_dir()]):
    root_text = str(root)
    if root_text not in sys.path:
        sys.path.insert(0, root_text)

from full_repo_harness import evidence as audit_evidence
from full_repo_harness import verify_common as common
import ui_implementation_gate as ui_gate


BATCH_SECTIONS = [
    "run id",
    "batch id",
    "batch summary",
    "file coverage",
    "ui source inventory",
    "mockup and journey alignment",
    "implementation gap findings",
    "no gap notes",
    "open questions",
]
MOCKUP_SECTIONS = [
    "run id",
    "worker",
    "mockup/asset inputs",
    "journey requirement inputs",
    "expected screens and visual requirements",
    "findings",
    "open questions",
]
TOOLING_SECTIONS = [
    "run id",
    "worker",
    "tooling inventory",
    "safe run path",
    "desktop/mobile screenshot plan",
    "findings",
    "open questions",
]
VISUAL_SECTIONS = [
    "run id",
    "worker",
    "mockup and asset inventory",
    "visual tooling",
    "journey decision model",
    "rendered journey usability",
    "visual comparison checks",
    "formal evidence",
    "findings",
    "open questions",
]
REQUIRED_FINDING_FIELDS = [
    "Priority",
    "Files",
    "Mockup/requirement evidence",
    "Interface evidence",
    "Expected behavior/standard",
    "Gap",
    "Suggested implementation direction",
]
VISUAL_RESULT_VALUES = {"MATCHED", "GAP", "BLOCKED", "NOT_APPLICABLE"}
VISUAL_EVIDENCE_RE = re.compile(
    r"\b(screenshot|screen shot|png|jpg|jpeg|webp|trace|video|playwright|cypress|storybook|browser|native|preview|simulator|xcode|android|blocked|unavailable|not applicable|no runnable|no safe)\b",
    re.IGNORECASE,
)
FIRST_VIEWPORT_EVIDENCE_RE = re.compile(
    r"\b(screenshot|screen shot|png|jpg|jpeg|webp|trace|video|playwright|cypress|storybook|browser|preview|simulator|native|snapshot|dom|viewport|measurement|measured|px|%|fold|scroll|source|css|blocked|unavailable|not applicable|no safe)\b",
    re.IGNORECASE,
)
JOURNEY_DECISION_COLUMNS = {
    "surface",
    "primary user goal",
    "primary decision",
    "required facts",
    "warning/flag conditions",
    "frequent actions",
    "secondary/rare actions",
    "unconfirmed assumptions",
}
RENDERED_USABILITY_COLUMNS = {
    "platform",
    "viewport",
    "decision supported",
    "visible decision-driving content",
    "visible secondary/detail content",
    "detail access pattern",
    "readability/contrast evidence",
    "layout quality result",
    "evidence",
}
UI_SOURCE_INVENTORY_COLUMNS = {
    "unit",
    "file",
    "surface",
    "visible element",
    "source evidence",
    "expected behavior",
    "actual implementation",
    "handler reference",
    "backend/api reference",
    "permission reference",
    "persistence reference",
    "test reference",
    "responsive/state notes",
}
USABILITY_RESULT_VALUES = {"PASS", "GAP", "BLOCKED", "NOT_APPLICABLE"}
VISUAL_COMPARISON_COLUMNS = {
    "platform",
    "journey",
    "viewport",
    "route/screen",
    "mockup/requirement",
    "implementation screenshot/tool evidence",
    "differences",
    "result",
}
FINAL_SECTIONS = [
    "coverage",
    "mockup and requirement inputs",
    "journey decision model",
    "rendered journey usability findings",
    "visual audit findings",
    "source implementation findings",
    "journey and responsive findings",
    "accessibility and interaction findings",
    "implementation plan",
    "verification plan",
]
VISUAL_DANGER_RE = re.compile(
    r"\b(overloaded?|crowded|cramped|unreadable|invisible|low[- ]contrast|clipped|cropped|truncated|"
    r"overflow|hidden overflow|no scroll|without scroll|unscannable|ambiguous hierarchy|oversized|"
    r"excessive detail|debug detail|raw status|overexposed|over-prescribed|overprescribed|"
    r"duplicate summaries?|duplicative summaries?|duplicate severity|vague labels?|unclear labels?|"
    r"detail controls?|source-model leakage|data-model leakage|dominates|dominating|buried|below the fold|"
    r"nested (?:cards?|blocks?|containers?|frames?)|card[- ]in[- ]card|blocks? inside blocks?|"
    r"border stacks?|background stacks?|visual noise|noisy surfaces?|misaligned|misalignment|"
    r"random(?:ly)? placed|random placement|weak grid|poor grid|grid drift|inconsistent gutters?|"
    r"unstable (?:expander|expansion|disclosure)|jumps? horizontally|width changes?|different widths?|"
    r"collapsed and expanded [^|.\n]*(?:different widths?|mismatch)|expanded and collapsed [^|.\n]*(?:different widths?|mismatch)|"
	    r"meaningless icons?|unclear icons?|unintuitive icons?|decorative clutter|avatar clutter|unnecessary avatars?|"
	    r"tiny icon[- ]only target|icon[- ]only (?:target|activation)|row [^|.\n]*(?:not clickable|not activated|not interactive)|"
	    r"(?:navigate|navigation|destination|click) [^|.\n]*(?:no pointer|cursor missing|unpredictable|missing destination|wrong destination)|"
	    r"(?:popover|flyout|expanded panel|temporary panel) [^|.\n]*(?:no timeout|no lifecycle|never closes|stays open|does not close)|"
	    r"(?:badge|flag) [^|.\n]*(?:not interactive|no hover|no click|no popover|no detail)|"
	    r"(?:hover|focus|click) [^|.\n]*(?:missing|absent|no feedback)|popover [^|.\n]*(?:missing|absent|not available)|"
	    r"(?:expander|expand icon|disclosure icon|chevron) [^|.\n]*(?:interferes?|overlaps?|collides?|fights?) [^|.\n]*(?:scroll|scrollbar)|"
	    r"scrollbar [^|.\n]*(?:interferes?|overlaps?|collides?|fights?) [^|.\n]*(?:expander|expand icon|disclosure icon|chevron)|"
	    r"copy (?:button|icon|control)s? [^|.\n]*(?:always visible|permanent|not reachable|disappears|unstable)|"
	    r"duplicate (?:status|error|duration)|status [^|.\n]*twice|error [^|.\n]*twice|duration [^|.\n]*(?:unnecessary|steals attention)|"
	    r"sender labels?|authorship labels?|routing labels?|selectable timestamps?|timestamp [^|.\n]*selectable|"
    r"permanent (?:helper|instruction)|instruction noise|helper text|low[- ]importance|"
    r"decision[- ]critical [^|.\n]*(?:hidden|weak|buried)|hidden [^|.\n]*decision[- ]critical|"
    r"weakly placed [^|.\n]*decision)\b",
    re.IGNORECASE,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Verify ui-implementation-audit reports.")
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--reports", nargs="+", required=True)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--skip-current-hash-check", action="store_true")
    return parser.parse_args()


def load_manifest(path: Path) -> dict:
    manifest = common.load_json_object(path, "manifest")
    if manifest.get("audit_kind") != "ui-implementation":
        raise ValueError("manifest audit_kind must be 'ui-implementation'.")
    source_files = manifest.get("source_files")
    batches = manifest.get("batches")
    coverage_units = manifest.get("coverage_units")
    if not isinstance(source_files, list):
        raise ValueError("manifest source_files must be a list.")
    if not isinstance(batches, list):
        raise ValueError("manifest batches must be a list.")
    if not isinstance(coverage_units, list):
        raise ValueError("manifest coverage_units must be a list.")
    if not source_files or not batches or not coverage_units:
        raise ValueError("UI implementation audit manifests must contain implemented interface source, batches, and coverage units.")
    if manifest.get("source_file_count") != len(source_files) or manifest.get("interface_file_count") != len(source_files):
        raise ValueError("manifest source/interface file counts must match the non-empty source_files list.")

    source_paths: list[str] = []
    source_hashes: dict[str, str] = {}
    for index, item in enumerate(source_files):
        if not isinstance(item, dict) or not isinstance(item.get("rel_path"), str):
            raise ValueError(f"source_files[{index}] must contain rel_path.")
        rel_path = item["rel_path"]
        source_paths.append(rel_path)
        sha = item.get("sha256")
        if not isinstance(sha, str) or not common.SHA256_RE.fullmatch(sha):
            raise ValueError(f"source_files[{index}].sha256 must be a SHA-256 hex digest.")
        source_hashes[rel_path] = sha
        if item.get("interface_relevant") is not True:
            raise ValueError(f"source_files[{index}] must be interface_relevant.")
        if item.get("kind") == "source/ui-asset":
            raise ValueError(f"source_files[{index}] must not be a visual asset batch entry.")
    duplicates = common.duplicate_values(source_paths)
    if duplicates:
        raise ValueError(f"source_files rel_path values must be unique: {duplicates}")

    source_set = set(source_paths)
    unit_to_file: dict[str, str] = {}
    unit_hashes: dict[str, str] = {}
    for index, unit in enumerate(coverage_units):
        if not isinstance(unit, dict) or not isinstance(unit.get("unit_id"), str) or not isinstance(unit.get("rel_path"), str):
            raise ValueError(f"coverage_units[{index}] must contain unit_id and rel_path.")
        if unit["rel_path"] not in source_set:
            raise ValueError(f"coverage_units[{index}].rel_path is absent from source_files: {unit['rel_path']}")
        unit_to_file[unit["unit_id"]] = unit["rel_path"]
        sha = unit.get("sha256") or source_hashes[unit["rel_path"]]
        if not isinstance(sha, str) or not common.SHA256_RE.fullmatch(sha):
            raise ValueError(f"coverage_units[{index}].sha256 must be a SHA-256 hex digest.")
        unit_hashes[unit["unit_id"]] = sha
    duplicate_units = common.duplicate_values(list(unit_to_file))
    if duplicate_units:
        raise ValueError(f"coverage_units unit_id values must be unique: {duplicate_units}")

    expected_by_batch: dict[str, set[str]] = {}
    files_by_batch: dict[str, set[str]] = {}
    assigned_units: list[str] = []
    for index, batch in enumerate(batches):
        if not isinstance(batch, dict) or not isinstance(batch.get("id"), str):
            raise ValueError(f"batches[{index}] must contain id.")
        units = batch.get("coverage_units")
        files = batch.get("files")
        if not isinstance(units, list) or not all(isinstance(item, str) for item in units):
            raise ValueError(f"batches[{index}].coverage_units must be a list of strings.")
        if not isinstance(files, list) or not all(isinstance(item, str) for item in files):
            raise ValueError(f"batches[{index}].files must be a list of strings.")
        unknown_units = sorted(set(units) - set(unit_to_file))
        if unknown_units:
            raise ValueError(f"batch {batch['id']} references unknown coverage units: {unknown_units}")
        expected_by_batch[batch["id"]] = set(units)
        files_by_batch[batch["id"]] = set(files)
        assigned_units.extend(units)
    missing_units = sorted(set(unit_to_file) - set(assigned_units))
    extra_units = sorted(set(assigned_units) - set(unit_to_file))
    duplicate_assignments = common.duplicate_values(assigned_units)
    if missing_units or extra_units or duplicate_assignments:
        raise ValueError(
            f"coverage unit assignment mismatch; missing={missing_units} extra={extra_units} duplicates={duplicate_assignments}"
        )

    audit = manifest.get("ui_implementation_audit")
    if not isinstance(audit, dict):
        raise ValueError("manifest ui_implementation_audit must be an object.")
    if audit.get("visual_required") is not True:
        raise ValueError("implemented UI audits must require visual review.")
    ui_platform = audit.get("ui_platform")
    if ui_platform not in {"web", "native", "hybrid"}:
        raise ValueError("manifest ui_implementation_audit.ui_platform must be web, native, or hybrid.")
    formal_config = audit.get("formal_config")
    if ui_platform == "native":
        if formal_config is not None:
            raise ValueError("native UI audit manifest must not declare formal web config.")
    elif formal_config is not None:
        if not isinstance(formal_config, dict) or not isinstance(formal_config.get("rel_path"), str):
            raise ValueError("formal_config must be null or a hash-bound repository file record.")
        rel_path = formal_config["rel_path"]
        config_path = Path(manifest["repo_root"]) / rel_path
        if config_path.is_symlink() or not config_path.is_file():
            raise ValueError("formal_config must resolve to a regular repository file.")
        if formal_config.get("sha256") != common.sha256_file(config_path):
            raise ValueError("formal_config hash does not match the current repository file.")
        if formal_config.get("size_bytes") != config_path.stat().st_size:
            raise ValueError("formal_config size does not match the current repository file.")
    gate = audit.get("implementation_gate")
    gate_schema = gate.get("schema_version") if isinstance(gate, dict) else None
    if not isinstance(gate, dict) or gate_schema not in {1, 2} or gate.get("status") != "passed":
        raise ValueError("manifest implementation_gate must be a passed schema-version-1-or-2 gate.")
    evidence_files = gate.get("evidence_files")
    if not isinstance(evidence_files, list) or not evidence_files:
        raise ValueError("manifest implementation_gate must name at least one evidence file.")
    if gate.get("rejected_files") not in ([], None):
        raise ValueError("manifest implementation_gate must not retain rejected evidence files in a passed gate.")
    evidence_paths: set[str] = set()
    for index, item in enumerate(evidence_files):
        if not isinstance(item, dict) or not isinstance(item.get("rel_path"), str):
            raise ValueError(f"implementation_gate.evidence_files[{index}] must contain rel_path.")
        rel_path = item["rel_path"]
        sha = item.get("sha256")
        if rel_path not in source_hashes:
            raise ValueError(f"implementation gate evidence is absent from source_files: {rel_path}")
        if sha != source_hashes[rel_path]:
            raise ValueError(f"implementation gate evidence hash does not match source_files: {rel_path}")
        qualification = item.get("qualification")
        if gate_schema == 1:
            override = None
        elif not isinstance(qualification, dict):
            raise ValueError(f"implementation gate evidence lacks qualification metadata: {rel_path}")
        elif qualification.get("method") == "recognized-ui-signal":
            override = None
        elif qualification.get("method") == "explicit-source-anchor":
            override = {
                "ui_kind": qualification.get("ui_kind"),
                "source_anchor": qualification.get("source_anchor"),
            }
        else:
            raise ValueError(f"implementation gate evidence uses an unknown qualification method: {rel_path}")
        issue, current_qualification = ui_gate.qualify_implementation_source(
            Path(manifest["repo_root"]),
            rel_path,
            override,
        )
        if issue:
            raise ValueError(f"implementation gate evidence is not a qualifying product UI source: {rel_path}: {issue}")
        if gate_schema == 2 and current_qualification != qualification:
            raise ValueError(f"implementation gate qualification no longer matches executable source: {rel_path}")
        evidence_paths.add(rel_path)
    if len(evidence_paths) != len(evidence_files):
        raise ValueError("manifest implementation_gate evidence paths must be unique.")
    asset_hashes: dict[str, str] = {}
    for index, item in enumerate(audit.get("visual_assets", [])):
        if not isinstance(item, dict) or not isinstance(item.get("rel_path"), str):
            raise ValueError(f"visual_assets[{index}] must contain rel_path.")
        sha = item.get("sha256")
        if not isinstance(sha, str) or not common.SHA256_RE.fullmatch(sha):
            raise ValueError(f"visual_assets[{index}].sha256 must be a SHA-256 hex digest.")
        asset_hashes[item["rel_path"]] = sha
    requirement_hashes: dict[str, str] = {}
    for index, item in enumerate(audit.get("requirement_sources", [])):
        if not isinstance(item, dict) or not isinstance(item.get("rel_path"), str):
            raise ValueError(f"requirement_sources[{index}] must contain rel_path.")
        sha = item.get("sha256")
        if not isinstance(sha, str) or not common.SHA256_RE.fullmatch(sha):
            raise ValueError(f"requirement_sources[{index}].sha256 must be a SHA-256 hex digest.")
        requirement_hashes[item["rel_path"]] = sha

    manifest["_source_hashes"] = source_hashes
    manifest["_asset_hashes"] = asset_hashes
    manifest["_requirement_hashes"] = requirement_hashes
    manifest["_ui_platform"] = ui_platform
    manifest["_formal_config"] = formal_config
    manifest["_unit_hashes"] = unit_hashes
    manifest["_unit_to_file"] = unit_to_file
    manifest["_expected_by_batch"] = expected_by_batch
    manifest["_files_by_batch"] = files_by_batch
    return manifest


def first_declared_value(body: str) -> str:
    for line in body.splitlines():
        stripped = line.strip().strip("`")
        if stripped:
            return stripped
    return ""


def split_files(value: str) -> list[str]:
    cleaned = value.replace("`", "")
    parts = re.split(r"[,;]", cleaned)
    return [part.strip() for part in parts if part.strip() and part.strip().lower() not in {"none"}]


def finding_blocks(text: str) -> list[dict[str, str]]:
    if text.strip() == "No findings.":
        return []
    blocks: list[dict[str, str]] = []
    current: dict[str, str] = {}
    for raw in text.splitlines():
        match = re.match(r"^-\s+([^:]+):\s*(.*)$", raw.strip())
        if not match:
            continue
        key = match.group(1).strip()
        value = match.group(2).strip()
        if key == "Priority" and current:
            blocks.append(current)
            current = {}
        current[key] = value
    if current:
        blocks.append(current)
    return blocks


def validate_findings(text: str, allowed_files: set[str], path: Path, section: str, *, allow_not_applicable: bool = False) -> list[dict]:
    issues: list[dict] = []
    stripped = text.strip()
    if not stripped:
        return [{"path": str(path), "section": section, "reason": "findings section is empty"}]
    if stripped == "No findings.":
        return []
    blocks = finding_blocks(text)
    if not blocks:
        return [{"path": str(path), "section": section, "reason": "findings must use required field blocks or exact sentinel"}]
    for index, block in enumerate(blocks, start=1):
        missing = [field for field in REQUIRED_FINDING_FIELDS if not block.get(field)]
        if missing:
            issues.append({"path": str(path), "section": section, "finding": index, "missing_fields": missing})
        priority = block.get("Priority", "")
        if priority and priority not in {"P0", "P1", "P2", "P3"}:
            issues.append({"path": str(path), "section": section, "finding": index, "field": "Priority", "actual": priority})
        files = split_files(block.get("Files", ""))
        if not files:
            issues.append({"path": str(path), "section": section, "finding": index, "field": "Files", "reason": "no files listed"})
            continue
        normalized = {item for item in files if item.lower() not in {"not-applicable", "not applicable", "n/a"}}
        if not normalized and not allow_not_applicable:
            issues.append({"path": str(path), "section": section, "finding": index, "field": "Files", "reason": "not-applicable not allowed here"})
        unknown = sorted(normalized - allowed_files)
        if unknown:
            issues.append({"path": str(path), "section": section, "finding": index, "field": "Files", "out_of_scope": unknown})
    return issues


def parse_percentage(value: str) -> float | None:
    match = re.search(r"(\d+(?:\.\d+)?)\s*%", value)
    if not match:
        return None
    return float(match.group(1))


def is_mobile_viewport(value: str) -> bool:
    lowered = value.lower()
    return any(token in lowered for token in ("mobile", "narrow", "phone", "390", "375", "360", "320"))


def web_evidence_required(manifest: dict) -> bool:
    return manifest.get("_ui_platform") in {"web", "hybrid"}


def native_evidence_required(manifest: dict) -> bool:
    return manifest.get("_ui_platform") in {"native", "hybrid"}


def is_negative(value: str) -> bool:
    lowered = value.lower()
    return bool(re.search(r"\b(no|not visible|below fold|below the fold|after scroll|hidden|cropped)\b", lowered))


def is_affirmative(value: str) -> bool:
    lowered = value.lower()
    return bool(re.search(r"\b(yes|true|above|dominates|dominant|settings|filters|configuration|config|controls)\b", lowered))


def has_visual_danger_finding(findings: str) -> bool:
    for block in finding_blocks(findings):
        combined = " ".join(block.values()).lower()
        if VISUAL_DANGER_RE.search(combined) or any(
            token in combined
            for token in ("journey usability", "decision path", "rendered journey", "readability", "contrast", "scannable")
        ):
            return True
    return False


def verify_wiring_reference_value(repo_root: Path, value: str, *, field: str) -> tuple[str, dict | None]:
    normalized = value.strip().strip("`")
    lowered = normalized.lower()
    if lowered == "missing":
        return "missing", None
    if lowered.startswith("not-applicable:") or lowered.startswith("not applicable:"):
        rationale = normalized.split(":", 1)[1].strip()
        if len(rationale) < 12:
            return "invalid", {"field": field, "reason": "not-applicable wiring references require a concrete rationale", "actual": value}
        return "not-applicable", None
    if "#" not in normalized:
        return "invalid", {"field": field, "reason": "wiring reference must be path#symbol, missing, or not-applicable: rationale", "actual": value}
    raw_path, symbol = normalized.split("#", 1)
    rel_path = Path(raw_path)
    if rel_path.is_absolute() or ".." in rel_path.parts or not symbol.strip():
        return "invalid", {"field": field, "reason": "wiring reference path#symbol is invalid", "actual": value}
    path = repo_root / rel_path
    try:
        resolved = path.resolve(strict=True)
        resolved.relative_to(repo_root.resolve())
    except (FileNotFoundError, ValueError):
        return "invalid", {"field": field, "reason": "wiring reference path does not resolve inside the audited repo", "actual": value}
    if path.is_symlink() or not path.is_file():
        return "invalid", {"field": field, "reason": "wiring reference must reference a regular repository file", "actual": value}
    try:
        source = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return "invalid", {"field": field, "reason": "wiring reference source is not UTF-8 text", "actual": value}
    if symbol.strip() not in source:
        return "invalid", {"field": field, "reason": "wiring reference symbol/text is absent from the referenced file", "actual": value}
    if field == "test reference" and not re.search(r"(?:^|/)(?:tests?|__tests__)(?:/|$)|\.(?:test|spec)\.", raw_path, re.IGNORECASE):
        return "invalid", {"field": field, "reason": "test reference must use a test/spec path", "actual": value}
    return "bound", None


def verify_journey_decision_model(path: Path, body: str) -> list[dict]:
    issues: list[dict] = []
    rows = common.parse_markdown_table_dicts(body)
    if not rows:
        return [{"path": str(path), "section": "Journey Decision Model", "reason": "missing journey decision table"}]
    for index, row in enumerate(rows, start=1):
        missing_columns = sorted(JOURNEY_DECISION_COLUMNS - set(row))
        if missing_columns:
            issues.append({"path": str(path), "section": "Journey Decision Model", "row": index, "missing_columns": missing_columns})
            continue
        for field in JOURNEY_DECISION_COLUMNS:
            if not row.get(field, "").strip():
                issues.append({"path": str(path), "section": "Journey Decision Model", "row": index, "field": field, "reason": "empty"})
    return issues


def rendered_usability_failure(row: dict[str, str]) -> bool:
    result = row.get("layout quality result", "").strip()
    if result in {"GAP", "BLOCKED"}:
        return True
    combined = " ".join(row.values())
    return bool(VISUAL_DANGER_RE.search(combined)) and result == "PASS"


def verify_rendered_usability_table(
    path: Path,
    body: str,
    findings: str,
    *,
    require_visual_evidence: bool = False,
    ui_platform: str,
) -> list[dict]:
    issues: list[dict] = []
    rows = common.parse_markdown_table_dicts(body)
    if not rows:
        return [{"path": str(path), "section": "Rendered Journey Usability", "reason": "missing rendered journey usability table"}]
    seen_web_desktop = False
    seen_web_mobile = False
    seen_native = False
    gap_or_blocked = False
    danger_failure = False
    for index, row in enumerate(rows, start=1):
        missing_columns = sorted(RENDERED_USABILITY_COLUMNS - set(row))
        if missing_columns:
            issues.append({"path": str(path), "section": "Rendered Journey Usability", "row": index, "missing_columns": missing_columns})
            continue
        for field in RENDERED_USABILITY_COLUMNS:
            if not row.get(field, "").strip():
                issues.append({"path": str(path), "section": "Rendered Journey Usability", "row": index, "field": field, "reason": "empty"})
        result = row.get("layout quality result", "").strip()
        if result not in USABILITY_RESULT_VALUES:
            issues.append({"path": str(path), "section": "Rendered Journey Usability", "row": index, "field": "Layout quality result", "expected": sorted(USABILITY_RESULT_VALUES), "actual": result})
        if result in {"GAP", "BLOCKED"}:
            gap_or_blocked = True
        platform = row.get("platform", "").strip().lower()
        if platform not in {"web", "native"}:
            issues.append({"path": str(path), "section": "Rendered Journey Usability", "row": index, "field": "Platform", "expected": ["native", "web"], "actual": platform})
        viewport = row.get("viewport", "").lower()
        if platform == "web" and "desktop" in viewport:
            seen_web_desktop = True
        if platform == "web" and is_mobile_viewport(viewport):
            seen_web_mobile = True
        if platform == "native":
            seen_native = True
        evidence = row.get("evidence", "")
        if evidence.strip() and not FIRST_VIEWPORT_EVIDENCE_RE.search(evidence):
            issues.append({"path": str(path), "section": "Rendered Journey Usability", "row": index, "field": "Evidence", "reason": "must name screenshot, DOM/viewport measurement, source evidence, or a concrete blocker"})
        if require_visual_evidence and evidence.strip() and not (VISUAL_EVIDENCE_RE.search(evidence) or re.search(r"\b(dom|viewport|measurement|measured|px|%|fold)\b", evidence, re.IGNORECASE)):
            issues.append({"path": str(path), "section": "Rendered Journey Usability", "row": index, "field": "Evidence", "reason": "visual journey checks require screenshot/tool evidence, DOM/native measurement, viewport measurement, or a concrete blocker"})
        if rendered_usability_failure(row):
            danger_failure = True
            if result == "PASS" and VISUAL_DANGER_RE.search(" ".join(row.values())):
                issues.append({"path": str(path), "section": "Rendered Journey Usability", "row": index, "field": "Layout quality result", "reason": "danger terms such as overload, unreadable text, clipping, overflow, or low contrast cannot be marked PASS without a finding"})
    if ui_platform in {"web", "hybrid"} and (not seen_web_desktop or not seen_web_mobile):
        issues.append({"path": str(path), "section": "Rendered Journey Usability", "reason": "web platform requires desktop and mobile/narrow rendered rows"})
    if ui_platform in {"native", "hybrid"} and not seen_native:
        issues.append({"path": str(path), "section": "Rendered Journey Usability", "reason": "native platform requires at least one native rendered row"})
    if gap_or_blocked and findings.strip() == "No findings.":
        issues.append({"path": str(path), "section": "Findings", "reason": "GAP or BLOCKED rendered journey usability rows require a finding"})
    if danger_failure and not has_visual_danger_finding(findings):
        issues.append({"path": str(path), "section": "Findings", "reason": "rendered journey usability danger terms require a visual/usability finding"})
    return issues


def verify_batch_report(path: Path, manifest: dict, batch_id: str) -> list[dict]:
    text = path.read_text(encoding="utf-8")
    order = common.section_order(text)
    bodies = common.section_bodies(text)
    issues: list[dict] = []
    if order != BATCH_SECTIONS:
        issues.append({"path": str(path), "reason": "batch report sections must match required order", "expected": BATCH_SECTIONS, "actual": order})
    if first_declared_value(bodies.get("run id", "")) != manifest["run_id"]:
        issues.append({"path": str(path), "field": "Run ID", "expected": manifest["run_id"], "actual": first_declared_value(bodies.get("run id", ""))})
    if first_declared_value(bodies.get("batch id", "")) != batch_id:
        issues.append({"path": str(path), "field": "Batch ID", "expected": batch_id, "actual": first_declared_value(bodies.get("batch id", ""))})

    expected_units = manifest["_expected_by_batch"][batch_id]
    expected_files = manifest["_files_by_batch"][batch_id]
    unit_hashes = manifest["_unit_hashes"]
    unit_to_file = manifest["_unit_to_file"]

    coverage_rows = common.parse_markdown_table_dicts(bodies.get("file coverage", ""))
    if not coverage_rows:
        issues.append({"path": str(path), "section": "File Coverage", "reason": "missing coverage table"})
    covered_units = {row.get("unit", "") for row in coverage_rows}
    missing = sorted(expected_units - covered_units)
    extra = sorted(covered_units - expected_units)
    if missing or extra:
        issues.append({"path": str(path), "section": "File Coverage", "missing_units": missing, "extra_units": extra})
    for row in coverage_rows:
        unit = row.get("unit", "")
        if row.get("status") != "CHECKED":
            issues.append({"path": str(path), "section": "File Coverage", "unit": unit, "field": "Status", "actual": row.get("status")})
        if unit in unit_hashes and row.get("sha-256") != unit_hashes[unit]:
            issues.append({"path": str(path), "section": "File Coverage", "unit": unit, "field": "SHA-256", "expected": unit_hashes[unit], "actual": row.get("sha-256")})
        if not row.get("purpose", "").strip():
            issues.append({"path": str(path), "section": "File Coverage", "unit": unit, "field": "Purpose", "reason": "empty"})

    inventory_rows = common.parse_markdown_table_dicts(bodies.get("ui source inventory", ""))
    if not inventory_rows:
        issues.append({"path": str(path), "section": "UI Source Inventory", "reason": "missing UI source inventory table"})
    elif set(inventory_rows[0]) != UI_SOURCE_INVENTORY_COLUMNS:
        issues.append(
            {
                "path": str(path),
                "section": "UI Source Inventory",
                "reason": "UI source inventory headers must include exact handler/backend/permission/persistence/test reference columns",
                "expected": sorted(UI_SOURCE_INVENTORY_COLUMNS),
                "actual": sorted(inventory_rows[0]),
            }
        )
    missing_trace_rows: list[dict] = []
    repo_root = Path(manifest["repo_root"])
    for row in inventory_rows:
        unit = row.get("unit", "")
        rel_file = row.get("file", "")
        if unit not in expected_units:
            issues.append({"path": str(path), "section": "UI Source Inventory", "unit": unit, "reason": "unit is outside this batch"})
        expected_file = unit_to_file.get(unit)
        if rel_file not in expected_files or (expected_file and rel_file != expected_file):
            issues.append({"path": str(path), "section": "UI Source Inventory", "file": rel_file, "reason": "file is outside this batch or mismatched to unit"})
        for field in ("surface", "visible element", "source evidence", "expected behavior", "actual implementation", "responsive/state notes"):
            if not row.get(field, "").strip():
                issues.append({"path": str(path), "section": "UI Source Inventory", "unit": unit, "field": field, "reason": "empty"})
        for field in ("handler reference", "backend/api reference", "permission reference", "persistence reference", "test reference"):
            status, reference_issue = verify_wiring_reference_value(repo_root, row.get(field, ""), field=field)
            if reference_issue:
                issues.append({"path": str(path), "section": "UI Source Inventory", "unit": unit, **reference_issue})
            if status == "missing":
                missing_trace_rows.append({"unit": unit, "visible_element": row.get("visible element", ""), "field": field})

    findings = bodies.get("implementation gap findings", "")
    if missing_trace_rows and findings.strip() == "No findings.":
        issues.append(
            {
                "path": str(path),
                "section": "Implementation Gap Findings",
                "reason": "missing handler/backend/permission/persistence/test wiring references require a finding",
                "missing_traces": missing_trace_rows,
            }
        )
    issues.extend(validate_findings(findings, expected_files, path, "Implementation Gap Findings"))
    for section in ("batch summary", "mockup and journey alignment", "no gap notes", "open questions"):
        if not bodies.get(section, "").strip():
            issues.append({"path": str(path), "section": section, "reason": "empty"})
    return issues


def verify_aux_report(path: Path, manifest: dict, expected_sections: list[str], worker: str, allowed_files: set[str]) -> list[dict]:
    text = path.read_text(encoding="utf-8")
    order = common.section_order(text)
    bodies = common.section_bodies(text)
    issues: list[dict] = []
    if order != expected_sections:
        issues.append({"path": str(path), "reason": "worker report sections must match required order", "expected": expected_sections, "actual": order})
    if first_declared_value(bodies.get("run id", "")) != manifest["run_id"]:
        issues.append({"path": str(path), "field": "Run ID", "expected": manifest["run_id"], "actual": first_declared_value(bodies.get("run id", ""))})
    if first_declared_value(bodies.get("worker", "")) != worker:
        issues.append({"path": str(path), "field": "Worker", "expected": worker, "actual": first_declared_value(bodies.get("worker", ""))})
    issues.extend(validate_findings(bodies.get("findings", ""), allowed_files, path, "Findings", allow_not_applicable=True))
    for section in expected_sections:
        if not bodies.get(section, "").strip():
            issues.append({"path": str(path), "section": section, "reason": "empty"})
    if worker == "visual_comparison_audit":
        ui_platform = manifest["_ui_platform"]
        issues.extend(verify_journey_decision_model(path, bodies.get("journey decision model", "")))
        issues.extend(
            verify_rendered_usability_table(
                path,
                bodies.get("rendered journey usability", ""),
                bodies.get("findings", ""),
                require_visual_evidence=True,
                ui_platform=ui_platform,
            )
        )
        issues.extend(verify_visual_comparison_table(path, bodies.get("visual comparison checks", ""), bodies.get("findings", ""), ui_platform=ui_platform))
        checklist_text = "\n".join(
            [
                bodies.get("rendered journey usability", ""),
                bodies.get("visual comparison checks", ""),
                bodies.get("findings", ""),
            ]
        )
        missing_labels = common.interaction_checklist_missing(checklist_text)
        if missing_labels:
            issues.append(
                {
                    "path": str(path),
                    "section": "Interaction Checklist",
                    "reason": "visual comparison report must mark every interaction checklist label pass/gap/blocked/not-applicable",
                    "missing": missing_labels,
                }
            )
        visual_rows = common.parse_markdown_table_dicts(bodies.get("visual comparison checks", ""))
        rendered_rows = [row for row in visual_rows if row.get("result", "").strip() in {"MATCHED", "GAP"}]
        evidence_records, evidence_issues = audit_evidence.validate_visual_evidence_manifest(
            path.parent.parent,
            manifest["run_id"],
            required=bool(rendered_rows),
        )
        for issue in evidence_issues:
            issues.append({"path": str(path), "section": "Visual Evidence", **issue})
        formal_body = bodies.get("formal evidence", "")
        formal_references = audit_evidence.evidence_references(formal_body)
        formal_kinds = {evidence_records[item].get("kind") for item in formal_references if item in evidence_records}
        if web_evidence_required(manifest):
            if manifest.get("_formal_config") is None:
                if "blocked" not in formal_body.lower() or bodies.get("findings", "").strip() == "No findings.":
                    issues.append({"path": str(path), "section": "Formal Evidence", "reason": "missing manifest-bound formal config must be BLOCKED with a finding"})
            else:
                missing_formal_kinds = sorted({"formal-web-verifier", "review-queue", "manual-review"} - formal_kinds)
                if missing_formal_kinds:
                    issues.append({"path": str(path), "section": "Formal Evidence", "reason": "web/hybrid audit must cite imported formal evidence", "missing_kinds": missing_formal_kinds})
        elif formal_body.strip() != "Formal Web UI verification not applicable to declared native platform.":
            issues.append({"path": str(path), "section": "Formal Evidence", "reason": "native audit must use the exact formal-web not-applicable statement"})
        if rendered_rows:
            required_kinds = set()
            if web_evidence_required(manifest):
                required_kinds.add("screenshot")
            if native_evidence_required(manifest):
                required_kinds.add("native-snapshot")
            if web_evidence_required(manifest) and manifest.get("_formal_config") is not None:
                required_kinds.update({"formal-web-verifier", "review-queue", "manual-review"})
            for issue in audit_evidence.validate_references(text, evidence_records, required_kinds=required_kinds):
                issues.append({"path": str(path), "section": "Visual Evidence", **issue})
            for index, row in enumerate(rendered_rows, start=1):
                references = audit_evidence.evidence_references(row.get("implementation screenshot/tool evidence", ""))
                screenshots = [
                    evidence_records[item]
                    for item in references
                    if item in evidence_records and evidence_records[item].get("kind") in {"screenshot", "native-snapshot"}
                ]
                row_platform = row.get("platform", "").strip().lower()
                if row_platform == "native":
                    screenshots = [item for item in screenshots if item.get("kind") == "native-snapshot"]
                elif row_platform == "web":
                    screenshots = [item for item in screenshots if item.get("kind") == "screenshot"]
                if not screenshots:
                    issues.append(
                        {
                            "path": str(path),
                            "section": "Visual Evidence",
                            "row": index,
                            "reason": "each rendered comparison row must bind a real screenshot/native snapshot with evidence:<id>",
                        }
                    )
                    continue
                route = row.get("route/screen", "").strip().lower()
                viewport = row.get("viewport", "").strip().lower()
                if not any(str(item.get("route", "")).strip().lower() == route for item in screenshots):
                    issues.append({"path": str(path), "section": "Visual Evidence", "row": index, "reason": "screenshot route metadata does not match the comparison row"})
                if not any(
                    viewport in str(item.get("viewport", {}).get("label", "")).strip().lower()
                    or str(item.get("viewport", {}).get("label", "")).strip().lower() in viewport
                    for item in screenshots
                ):
                    issues.append({"path": str(path), "section": "Visual Evidence", "row": index, "reason": "screenshot viewport metadata does not match the comparison row"})
    return issues


def verify_visual_comparison_table(path: Path, body: str, findings: str, *, ui_platform: str) -> list[dict]:
    issues: list[dict] = []
    rows = common.parse_markdown_table_dicts(body)
    if not rows:
        return [{"path": str(path), "section": "Visual Comparison Checks", "reason": "missing visual comparison table"}]
    required = VISUAL_COMPARISON_COLUMNS
    web_viewports: set[str] = set()
    seen_native = False
    non_not_applicable = False
    blocked_or_gap = False
    danger_failure = False
    for index, row in enumerate(rows, start=1):
        missing_columns = sorted(required - set(row))
        if missing_columns:
            issues.append({"path": str(path), "section": "Visual Comparison Checks", "row": index, "missing_columns": missing_columns})
            continue
        for field in required:
            if not row.get(field, "").strip():
                issues.append({"path": str(path), "section": "Visual Comparison Checks", "row": index, "field": field, "reason": "empty"})
        result = row.get("result", "").strip()
        if result not in VISUAL_RESULT_VALUES:
            issues.append({"path": str(path), "section": "Visual Comparison Checks", "row": index, "field": "Result", "expected": sorted(VISUAL_RESULT_VALUES), "actual": result})
        if result != "NOT_APPLICABLE":
            non_not_applicable = True
        if result in {"GAP", "BLOCKED"}:
            blocked_or_gap = True
        if result == "MATCHED" and VISUAL_DANGER_RE.search(" ".join(row.values())):
            danger_failure = True
            issues.append(
                {
                    "path": str(path),
                    "section": "Visual Comparison Checks",
                    "row": index,
                    "field": "Result",
                    "reason": "danger terms such as overload, nested frames, unstable disclosure, meaningless icons, instruction noise, unreadable text, clipping, overflow, or low contrast cannot be marked MATCHED without a finding",
                }
            )
        platform = row.get("platform", "").strip().lower()
        if platform not in {"web", "native"}:
            issues.append({"path": str(path), "section": "Visual Comparison Checks", "row": index, "field": "Platform", "expected": ["native", "web"], "actual": platform})
        viewport = row.get("viewport", "").lower()
        if platform == "web" and "desktop" in viewport:
            web_viewports.add("desktop")
        if platform == "web" and ("mobile" in viewport or "narrow" in viewport or "phone" in viewport):
            web_viewports.add("mobile")
        if platform == "native":
            seen_native = True
        evidence = row.get("implementation screenshot/tool evidence", "")
        if evidence.strip() and not VISUAL_EVIDENCE_RE.search(evidence):
            issues.append(
                {
                    "path": str(path),
                    "section": "Visual Comparison Checks",
                    "row": index,
                    "field": "Implementation Screenshot/Tool Evidence",
                    "reason": "must name screenshot/trace/tool evidence or a concrete blocker",
                }
            )
    if non_not_applicable and ui_platform in {"web", "hybrid"} and {"desktop", "mobile"} - web_viewports:
        issues.append({"path": str(path), "section": "Visual Comparison Checks", "reason": "web platform requires desktop and mobile/narrow comparison rows", "viewports_found": sorted(web_viewports)})
    if non_not_applicable and ui_platform in {"native", "hybrid"} and not seen_native:
        issues.append({"path": str(path), "section": "Visual Comparison Checks", "reason": "native platform requires at least one native comparison row"})
    if blocked_or_gap and findings.strip() == "No findings.":
        issues.append({"path": str(path), "section": "Findings", "reason": "GAP or BLOCKED visual rows require a finding"})
    if danger_failure and not has_visual_danger_finding(findings):
        issues.append({"path": str(path), "section": "Findings", "reason": "visual danger terms require a visual/usability finding"})
    return issues


def verify_final_report(manifest_path: Path, manifest: dict) -> list[dict]:
    path = manifest_path.parent / "final-report.md"
    if not path.is_file() or path.is_symlink():
        return [{"path": str(path), "reason": "final-report.md is missing or not a regular file"}]
    text = path.read_text(encoding="utf-8")
    order = common.section_order(text)
    bodies = common.section_bodies(text)
    issues: list[dict] = []
    if order != FINAL_SECTIONS:
        issues.append({"path": str(path), "reason": "final report sections must match required order", "expected": FINAL_SECTIONS, "actual": order})
    for section in FINAL_SECTIONS:
        if not bodies.get(section, "").strip():
            issues.append({"path": str(path), "section": section, "reason": "empty"})
    coverage = bodies.get("coverage", "")
    if manifest["run_id"] not in coverage:
        issues.append({"path": str(path), "section": "Coverage", "reason": "must name the audit run id"})
    if manifest["_ui_platform"] not in coverage.lower():
        issues.append({"path": str(path), "section": "Coverage", "reason": "must state the declared UI platform"})
    missing_labels = common.interaction_checklist_missing(bodies.get("accessibility and interaction findings", ""))
    if missing_labels:
        issues.append({"path": str(path), "section": "Accessibility And Interaction Findings", "reason": "final report must mark every interaction checklist label", "missing": missing_labels})

    evidence_required = native_evidence_required(manifest) or manifest.get("_formal_config") is not None
    evidence_records, evidence_issues = audit_evidence.validate_visual_evidence_manifest(
        manifest_path.parent,
        manifest["run_id"],
        required=evidence_required,
    )
    for issue in evidence_issues:
        issues.append({"path": str(path), "section": "Visual Evidence", **issue})
    required_visual_kinds = set()
    if web_evidence_required(manifest):
        required_visual_kinds.add("screenshot")
    if native_evidence_required(manifest):
        required_visual_kinds.add("native-snapshot")
    if evidence_required:
        for issue in audit_evidence.validate_references(text, evidence_records, required_kinds=required_visual_kinds):
            issues.append({"path": str(path), "section": "Visual Evidence", **issue})
    referenced = audit_evidence.evidence_references(text)
    referenced_kinds = {evidence_records[item].get("kind") for item in referenced if item in evidence_records}
    if web_evidence_required(manifest) and manifest.get("_formal_config") is not None:
        missing = sorted({"formal-web-verifier", "review-queue", "manual-review"} - referenced_kinds)
        if missing:
            issues.append({"path": str(path), "section": "Visual Audit Findings", "reason": "final report must cite the imported formal evidence chain", "missing_kinds": missing})
    if web_evidence_required(manifest) and manifest.get("_formal_config") is None:
        formal_text = f"{coverage}\n{bodies.get('visual audit findings', '')}".lower()
        if "formal" not in formal_text or "blocked" not in formal_text:
            issues.append({"path": str(path), "section": "Visual Audit Findings", "reason": "missing formal config must remain an explicit blocker"})
    verification = bodies.get("verification plan", "").lower()
    if not any(token in verification for token in ("runtime", "test", "blocked", "not applicable")):
        issues.append({"path": str(path), "section": "Verification Plan", "reason": "must name runtime/test proof or a concrete blocker/non-applicability"})
    if "source-only proves" in text.lower() or "path#symbol proves" in text.lower():
        issues.append({"path": str(path), "reason": "source wiring references must not be represented as observable outcome proof"})
    return issues


def verify_marker(manifest_path: Path, manifest: dict) -> list[dict]:
    marker_path = manifest_path.parent / "queue_complete.json"
    if not marker_path.is_file():
        return [{"path": str(marker_path), "reason": "queue_complete.json is missing"}]
    marker = common.load_json_object(marker_path, "queue_complete.json")
    expected = {
        "run_id": manifest["run_id"],
        "phase": "queue_generated",
        "audit_verified": False,
        "audit_kind": "ui-implementation",
        "manifest": "manifest.json",
        "audit_index": "audit_index.md",
        "execution_ledger": "execution_ledger.json",
        "excluded_files": "excluded_files.json",
        "reports_dir": "reports",
        "ownership_marker": ".ui-implementation-audit-artifacts.json",
        "batch_count": manifest["batch_count"],
        "source_file_count": manifest["source_file_count"],
    }
    return [
        {"path": str(marker_path), "field": key, "expected": value, "actual": marker.get(key)}
        for key, value in expected.items()
        if marker.get(key) != value
    ]


def verify_excluded_files(manifest_path: Path, manifest: dict) -> tuple[list[dict], list[dict]]:
    excluded_path = manifest_path.parent / "excluded_files.json"
    excluded = common.load_json_list(excluded_path, "excluded_files.json")
    issues: list[dict] = []
    if manifest.get("excluded_file_count") != len(excluded):
        issues.append({"path": str(excluded_path), "field": "excluded_file_count", "expected": manifest.get("excluded_file_count"), "actual": len(excluded)})
    digest = common.canonical_json_sha256(excluded)
    if manifest.get("excluded_files_sha256") != digest:
        issues.append({"path": str(excluded_path), "field": "excluded_files_sha256", "expected": manifest.get("excluded_files_sha256"), "actual": digest})
    warnings = [item for item in excluded if isinstance(item, dict) and item.get("scope_warning")]
    if warnings:
        issues.append({"path": str(excluded_path), "reason": "unresolved scope warnings", "scope_warnings": warnings})
    return warnings, issues


def verify_execution_ledger(manifest_path: Path, manifest: dict) -> list[dict]:
    ledger_path = manifest_path.parent / "execution_ledger.json"
    if not ledger_path.is_file():
        return [{"path": str(ledger_path), "reason": "execution_ledger.json is missing"}]
    ledger = common.load_json_object(ledger_path, "execution_ledger.json")
    issues: list[dict] = []
    if ledger.get("run_id") != manifest["run_id"]:
        issues.append({"path": str(ledger_path), "field": "run_id", "expected": manifest["run_id"], "actual": ledger.get("run_id")})
    forbidden_effort_keys = []
    def find_forbidden(value: object, prefix: str = "") -> None:
        if isinstance(value, dict):
            for key, child in value.items():
                child_prefix = f"{prefix}.{key}" if prefix else key
                if key in {"required_reasoning_effort", "actual_reasoning_effort", "can_set_reasoning_effort"}:
                    forbidden_effort_keys.append(child_prefix)
                find_forbidden(child, child_prefix)
        elif isinstance(value, list):
            for index, child in enumerate(value):
                find_forbidden(child, f"{prefix}[{index}]")
    find_forbidden(ledger)
    if forbidden_effort_keys:
        issues.append({"path": str(ledger_path), "reason": "UI audit execution ledger must not prescribe or validate worker reasoning effort", "fields": forbidden_effort_keys})

    capability = ledger.get("worker_capability_check", {})
    if not isinstance(capability, dict):
        issues.append({"path": str(ledger_path), "field": "worker_capability_check", "reason": "must be an object"})
    else:
        if capability.get("status") != "completed":
            issues.append({"path": str(ledger_path), "field": "worker_capability_check.status", "expected": "completed", "actual": capability.get("status")})
        if not capability.get("spawn_tool"):
            issues.append({"path": str(ledger_path), "field": "worker_capability_check.spawn_tool", "expected": "non-empty string", "actual": capability.get("spawn_tool")})
    lead = ledger.get("lead", {})
    if not isinstance(lead, dict) or lead.get("status") not in {"completed", "confirmed", "manual-fallback-completed"}:
        issues.append({"path": str(ledger_path), "field": "lead.status", "expected": "completed/confirmed", "actual": lead.get("status") if isinstance(lead, dict) else None})
    if isinstance(lead, dict) and not lead.get("runtime_provenance"):
        issues.append({"path": str(ledger_path), "field": "lead.runtime_provenance", "expected": "non-empty string", "actual": lead.get("runtime_provenance")})
    workers = ledger.get("batch_workers")
    if not isinstance(workers, list):
        return issues + [{"path": str(ledger_path), "field": "batch_workers", "reason": "must be a list"}]
    by_id = {item.get("batch_id"): item for item in workers if isinstance(item, dict)}
    for batch in manifest["batches"]:
        row = by_id.get(batch["id"])
        if not row:
            issues.append({"path": str(ledger_path), "field": "batch_workers", "missing": batch["id"]})
        elif row.get("status") not in {"completed", "manual-fallback-completed"}:
            issues.append({"path": str(ledger_path), "batch_id": batch["id"], "field": "status", "actual": row.get("status")})
        else:
            if row.get("status") != "manual-fallback-completed" and not row.get("agent_id"):
                issues.append({"path": str(ledger_path), "batch_id": batch["id"], "field": "agent_id", "expected": "non-empty string", "actual": row.get("agent_id")})
            if not row.get("runtime_provenance"):
                issues.append({"path": str(ledger_path), "batch_id": batch["id"], "field": "runtime_provenance", "expected": "non-empty string", "actual": row.get("runtime_provenance")})
    if manifest.get("ui_implementation_audit", {}).get("visual_required"):
        audit = manifest["ui_implementation_audit"]
        expected_worker_keys = ["visual_comparison_worker"]
        if audit.get("mockup_asset_prompt"):
            expected_worker_keys.append("mockup_asset_worker")
        if audit.get("visual_tooling_prompt"):
            expected_worker_keys.append("visual_tooling_worker")
        for key in expected_worker_keys:
            row = ledger.get(key, {})
            if not isinstance(row, dict) or row.get("status") not in {"completed", "manual-fallback-completed"}:
                issues.append({"path": str(ledger_path), "field": f"{key}.status", "actual": row.get("status") if isinstance(row, dict) else None})
            else:
                if row.get("status") != "manual-fallback-completed" and not row.get("agent_id"):
                    issues.append({"path": str(ledger_path), "field": f"{key}.agent_id", "expected": "non-empty string", "actual": row.get("agent_id")})
                if not row.get("runtime_provenance"):
                    issues.append({"path": str(ledger_path), "field": f"{key}.runtime_provenance", "expected": "non-empty string", "actual": row.get("runtime_provenance")})
    return issues


def verify_current_hashes(manifest: dict) -> list[dict]:
    repo_root = Path(manifest["repo_root"])
    issues: list[dict] = []
    expected_hashes = {}
    expected_hashes.update(manifest["_source_hashes"])
    expected_hashes.update(manifest["_asset_hashes"])
    expected_hashes.update(manifest["_requirement_hashes"])
    for rel_path, expected in expected_hashes.items():
        path = repo_root / rel_path
        if not path.is_file():
            issues.append({"path": str(path), "reason": "manifest input file is missing"})
            continue
        actual = common.sha256_file(path)
        if actual != expected:
            issues.append({"path": rel_path, "expected": expected, "actual": actual, "reason": "current input hash differs from manifest"})
    return issues


def verify(manifest_path: Path, reports: list[Path], *, skip_current_hash_check: bool = False) -> dict:
    manifest = load_manifest(manifest_path)
    issues: dict[str, list] = {
        "completion_marker_mismatches": verify_marker(manifest_path, manifest),
        "excluded_file_issues": [],
        "execution_ledger_issues": verify_execution_ledger(manifest_path, manifest),
        "missing_reports": [],
        "report_issues": [],
        "final_report_issues": [],
        "current_hash_mismatches": [],
    }
    _, excluded_issues = verify_excluded_files(manifest_path, manifest)
    issues["excluded_file_issues"] = excluded_issues
    if not skip_current_hash_check:
        issues["current_hash_mismatches"] = verify_current_hashes(manifest)

    reports_by_name = {path.name: path for path in reports}
    for batch in manifest["batches"]:
        expected_name = f"{batch['id']}.md"
        report = reports_by_name.get(expected_name)
        if not report:
            issues["missing_reports"].append({"report": expected_name})
            continue
        issues["report_issues"].extend(verify_batch_report(report, manifest, batch["id"]))

    all_source_files = set(manifest["_source_hashes"])
    if manifest.get("ui_implementation_audit", {}).get("visual_required"):
        audit = manifest["ui_implementation_audit"]
        expected_aux = [("visual_comparison_audit.md", VISUAL_SECTIONS, "visual_comparison_audit")]
        if audit.get("mockup_asset_prompt"):
            expected_aux.append(("mockup_asset_audit.md", MOCKUP_SECTIONS, "mockup_asset_audit"))
        if audit.get("visual_tooling_prompt"):
            expected_aux.append(("visual_tooling_audit.md", TOOLING_SECTIONS, "visual_tooling_audit"))
        for filename, sections, worker in expected_aux:
            report = reports_by_name.get(filename)
            if not report:
                issues["missing_reports"].append({"report": filename})
            else:
                issues["report_issues"].extend(verify_aux_report(report, manifest, sections, worker, all_source_files))

    issues["final_report_issues"] = verify_final_report(manifest_path, manifest)

    ok = not any(issues.values())
    return {"ok": ok, "manifest": str(manifest_path), "run_id": manifest["run_id"], "issues": issues}


def print_human(result: dict) -> None:
    print(f"ok: {str(result['ok']).lower()}")
    print(f"run_id: {result['run_id']}")
    for key, values in result["issues"].items():
        if values:
            print(f"{key}:")
            for value in values:
                print(f"- {json.dumps(value, sort_keys=True)}")


def main() -> int:
    args = parse_args()
    try:
        reports = common.iter_report_files(args.reports)
        result = verify(Path(args.manifest), reports, skip_current_hash_check=args.skip_current_hash_check)
    except Exception as exc:
        if args.json:
            print(json.dumps({"ok": False, "error": {"type": type(exc).__name__, "message": str(exc)}}, indent=2))
        else:
            print(f"{type(exc).__name__}: {exc}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print_human(result)
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
