#!/usr/bin/env python3
"""Verify returned reports for the full-repo-test-coverage-audit skill."""

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
DEV_SKILL_DIR = (REPO_ROOT / "skills" / "full-repo-test-coverage-audit").resolve()
running_in_dev_repo = DEV_SKILL_DIR == SKILL_DIR.resolve() and (REPO_ROOT / "full_repo_harness" / "verify_common.py").is_file()

path_roots = [REPO_ROOT, VENDOR_ROOT] if running_in_dev_repo else [VENDOR_ROOT]
for root in reversed([item for item in path_roots if item.is_dir()]):
    root_text = str(root)
    if root_text not in sys.path:
        sys.path.insert(0, root_text)

from full_repo_harness import verify_common as common


BATCH_SECTIONS = [
    "run id",
    "batch id",
    "batch summary",
    "file coverage",
    "test target inventory",
    "coverage findings",
    "no gap notes",
    "open questions",
]
UI_SECTIONS = ["run id", "worker", "journey/test sources", "ui coverage checks", "findings", "open questions"]
VISUAL_SECTIONS = ["run id", "worker", "visual/e2e tooling", "visual/e2e coverage checks", "findings", "open questions"]
REQUIRED_FINDING_FIELDS = [
    "Priority",
    "Files",
    "Target",
    "Existing test evidence",
    "Missing scenarios/boundaries",
    "Suggested test direction",
]
TARGET_INVENTORY_COLUMNS = {
    "target id",
    "unit",
    "file",
    "target",
    "kind",
    "disposition",
    "evidence level",
    "existing test evidence",
    "scenario assessment",
    "recommendation",
}
DISPOSITIONS = {"TESTED", "UNTESTED", "NOT_REASONABLE"}
EVIDENCE_LEVELS = {"EMPIRICAL", "STRUCTURAL", "MANUAL", "NONE"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Verify full-repo-test-coverage-audit reports.")
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--reports", nargs="+", required=True)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--skip-current-hash-check", action="store_true")
    return parser.parse_args()


def load_manifest(path: Path) -> dict:
    manifest = common.load_json_object(path, "manifest")
    if manifest.get("audit_kind") != "test-coverage":
        raise ValueError("manifest audit_kind must be 'test-coverage'.")
    source_files = manifest.get("source_files")
    batches = manifest.get("batches")
    coverage_units = manifest.get("coverage_units")
    if not isinstance(source_files, list):
        raise ValueError("manifest source_files must be a list.")
    if not isinstance(batches, list):
        raise ValueError("manifest batches must be a list.")
    if not isinstance(coverage_units, list):
        raise ValueError("manifest coverage_units must be a list.")

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

    coverage = manifest.get("test_coverage_audit")
    if not isinstance(coverage, dict):
        raise ValueError("manifest test_coverage_audit must be an object.")
    raw_targets = coverage.get("target_inventory")
    if not isinstance(raw_targets, list):
        raise ValueError("test_coverage_audit.target_inventory must be a list.")
    target_by_id: dict[str, dict] = {}
    targets_by_batch: dict[str, set[str]] = {batch_id: set() for batch_id in expected_by_batch}
    batch_by_unit = {unit: batch_id for batch_id, units in expected_by_batch.items() for unit in units}
    for index, target in enumerate(raw_targets):
        if not isinstance(target, dict):
            raise ValueError(f"target_inventory[{index}] must be an object.")
        target_id = target.get("target_id")
        unit_id = target.get("unit_id")
        rel_path = target.get("rel_path")
        if not isinstance(target_id, str) or not target_id.startswith("target-"):
            raise ValueError(f"target_inventory[{index}].target_id must be a deterministic target id.")
        if target_id in target_by_id:
            raise ValueError(f"target_inventory target ids must be unique: {target_id}")
        if unit_id not in unit_to_file or rel_path != unit_to_file.get(unit_id):
            raise ValueError(f"target_inventory[{index}] must bind an exact coverage unit and file.")
        if not isinstance(target.get("symbol"), str) or not target.get("symbol") or not isinstance(target.get("line"), int):
            raise ValueError(f"target_inventory[{index}] must include symbol and line.")
        target_by_id[target_id] = target
        targets_by_batch[batch_by_unit[unit_id]].add(target_id)
    if coverage.get("target_count") != len(raw_targets):
        raise ValueError("test_coverage_audit.target_count does not match target_inventory.")

    empirical = coverage.get("empirical_coverage", [])
    if not isinstance(empirical, list):
        raise ValueError("test_coverage_audit.empirical_coverage must be a list.")
    empirical_by_file: dict[str, set[int]] = {}
    empirical_issues: list[dict] = []
    for index, record in enumerate(empirical):
        if not isinstance(record, dict):
            empirical_issues.append({"record": index, "reason": "coverage evidence must be an object"})
            continue
        evidence_path = Path(str(record.get("path", "")))
        if not evidence_path.is_absolute() or evidence_path.is_symlink() or not evidence_path.is_file():
            empirical_issues.append({"record": index, "field": "path", "reason": "coverage evidence path must be an existing absolute regular file"})
        elif record.get("sha256") != common.sha256_file(evidence_path):
            empirical_issues.append({"record": index, "field": "sha256", "reason": "coverage evidence hash changed"})
        if record.get("format") not in {"lcov", "cobertura-xml", "coverage.py-json", "istanbul-json"}:
            empirical_issues.append({"record": index, "field": "format", "reason": "unsupported empirical coverage format"})
        files = record.get("files")
        if not isinstance(files, dict):
            empirical_issues.append({"record": index, "field": "files", "reason": "must be an object"})
            continue
        for rel_path, line_data in files.items():
            if rel_path not in source_set or not isinstance(line_data, dict):
                empirical_issues.append({"record": index, "file": rel_path, "reason": "coverage file is not a manifest source or line data is invalid"})
                continue
            measured = line_data.get("measured_lines")
            covered = line_data.get("covered_lines")
            if not isinstance(measured, list) or not all(isinstance(line, int) and not isinstance(line, bool) and line > 0 for line in measured):
                empirical_issues.append({"record": index, "file": rel_path, "field": "measured_lines", "reason": "must be positive integer lines"})
                continue
            if not isinstance(covered, list) or not all(isinstance(line, int) and not isinstance(line, bool) and line > 0 for line in covered) or not set(covered) <= set(measured):
                empirical_issues.append({"record": index, "file": rel_path, "field": "covered_lines", "reason": "must be a subset of measured lines"})
                continue
            empirical_by_file.setdefault(rel_path, set()).update(covered)

    manifest["_source_hashes"] = source_hashes
    manifest["_unit_hashes"] = unit_hashes
    manifest["_unit_to_file"] = unit_to_file
    manifest["_expected_by_batch"] = expected_by_batch
    manifest["_files_by_batch"] = files_by_batch
    manifest["_target_by_id"] = target_by_id
    manifest["_targets_by_batch"] = targets_by_batch
    manifest["_empirical_covered_lines"] = empirical_by_file
    manifest["_empirical_issues"] = empirical_issues
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
    return [part.strip() for part in parts if part.strip() and part.strip().lower() not in {"none", "not applicable"}]


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


def verify_test_reference(repo_root: Path, value: str) -> dict | None:
    normalized = value.strip().strip("`")
    if "#" not in normalized:
        return {"reason": "structural/empirical test evidence must be real test/path#test-symbol", "actual": value}
    raw_path, symbol = normalized.split("#", 1)
    rel_path = Path(raw_path)
    if rel_path.is_absolute() or ".." in rel_path.parts or not symbol.strip():
        return {"reason": "test evidence path#symbol is invalid", "actual": value}
    if not re.search(r"(?:^|/)(?:tests?|__tests__)(?:/|$)|\.(?:test|spec)\.", raw_path, re.IGNORECASE):
        return {"reason": "test evidence path is not test/spec-shaped", "actual": value}
    path = repo_root / rel_path
    try:
        resolved = path.resolve(strict=True)
        resolved.relative_to(repo_root.resolve())
    except (FileNotFoundError, ValueError):
        return {"reason": "test evidence path does not resolve inside the audited repo", "actual": value}
    if path.is_symlink() or not path.is_file():
        return {"reason": "test evidence must be a regular repository file", "actual": value}
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return {"reason": "test evidence file is not UTF-8", "actual": value}
    if symbol.strip() not in text:
        return {"reason": "test symbol/name is absent from the referenced test file", "actual": value}
    return None


def validate_findings(text: str, allowed_files: set[str], path: Path, section: str) -> list[dict]:
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
        unknown = sorted(set(files) - allowed_files)
        if unknown:
            issues.append({"path": str(path), "section": section, "finding": index, "field": "Files", "out_of_scope": unknown})
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

    inventory_rows = common.parse_markdown_table_dicts(bodies.get("test target inventory", ""))
    if not inventory_rows:
        issues.append({"path": str(path), "section": "Test Target Inventory", "reason": "missing target inventory table"})
    elif set(inventory_rows[0]) != TARGET_INVENTORY_COLUMNS:
        issues.append(
            {
                "path": str(path),
                "section": "Test Target Inventory",
                "reason": "target inventory headers must exactly expose disposition and evidence level",
                "expected": sorted(TARGET_INVENTORY_COLUMNS),
                "actual": sorted(inventory_rows[0]),
            }
        )
    expected_target_ids = manifest["_targets_by_batch"][batch_id]
    observed_target_ids = [row.get("target id", "") for row in inventory_rows]
    missing_targets = sorted(expected_target_ids - set(observed_target_ids))
    all_extra_targets = sorted(set(observed_target_ids) - expected_target_ids)
    extra_targets = sorted(item for item in all_extra_targets if not re.fullmatch(r"manual-[A-Za-z0-9_-]{4,80}", item))
    duplicate_targets = common.duplicate_values(observed_target_ids)
    if missing_targets or extra_targets or duplicate_targets:
        issues.append(
            {
                "path": str(path),
                "section": "Test Target Inventory",
                "reason": "every deterministic target needs exactly one tested/untested/not-reasonable mapping",
                "missing_targets": missing_targets,
                "missing_target_details": [manifest["_target_by_id"][item] for item in missing_targets if item in manifest["_target_by_id"]],
                "extra_targets": extra_targets,
                "duplicate_targets": duplicate_targets,
            }
        )
    untested_targets: set[str] = set()
    repo_root = Path(manifest["repo_root"])
    for row in inventory_rows:
        target_id = row.get("target id", "")
        target = manifest["_target_by_id"].get(target_id)
        unit = row.get("unit", "")
        rel_file = row.get("file", "")
        if unit not in expected_units:
            issues.append({"path": str(path), "section": "Test Target Inventory", "unit": unit, "reason": "unit is outside this batch"})
        expected_file = unit_to_file.get(unit)
        if rel_file not in expected_files or (expected_file and rel_file != expected_file):
            issues.append({"path": str(path), "section": "Test Target Inventory", "file": rel_file, "reason": "file is outside this batch or mismatched to unit"})
        for field in ("target", "kind", "disposition", "evidence level", "existing test evidence", "scenario assessment", "recommendation"):
            if not row.get(field, "").strip():
                issues.append({"path": str(path), "section": "Test Target Inventory", "unit": unit, "field": field, "reason": "empty"})
        if target:
            for field, expected in (("unit", target["unit_id"]), ("file", target["rel_path"]), ("target", target["symbol"]), ("kind", target["kind"])):
                if row.get(field) != expected:
                    issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, "field": field, "expected": expected, "actual": row.get(field)})
        disposition = row.get("disposition", "")
        level = row.get("evidence level", "")
        evidence_value = row.get("existing test evidence", "")
        if disposition not in DISPOSITIONS:
            issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, "field": "Disposition", "expected": sorted(DISPOSITIONS), "actual": disposition})
        if level not in EVIDENCE_LEVELS:
            issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, "field": "Evidence Level", "expected": sorted(EVIDENCE_LEVELS), "actual": level})
        if disposition == "TESTED":
            if level in {"STRUCTURAL", "EMPIRICAL"}:
                reference_issue = verify_test_reference(repo_root, evidence_value)
                if reference_issue:
                    issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, **reference_issue})
            elif level == "MANUAL":
                if not evidence_value.lower().startswith("manual:") or len(evidence_value.split(":", 1)[-1].strip()) < 12:
                    issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, "reason": "manual TESTED claims require concrete 'manual: ...' evidence"})
            else:
                issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, "reason": "TESTED targets cannot use NONE evidence"})
            if level == "EMPIRICAL" and target and target["line"] not in manifest["_empirical_covered_lines"].get(target["rel_path"], set()):
                issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, "reason": "EMPIRICAL claim is not backed by a supplied coverage report at the target line"})
        elif disposition == "UNTESTED":
            untested_targets.add(target_id)
            if level != "NONE" or evidence_value.strip().lower() != "none found":
                issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, "reason": "UNTESTED targets must use NONE / None found honestly"})
        elif disposition == "NOT_REASONABLE":
            if level != "MANUAL" or not evidence_value.lower().startswith("not reasonable:") or len(evidence_value.split(":", 1)[-1].strip()) < 12:
                issues.append({"path": str(path), "section": "Test Target Inventory", "target_id": target_id, "reason": "NOT_REASONABLE requires MANUAL evidence and a concrete 'not reasonable: ...' rationale"})

    findings_text = bodies.get("coverage findings", "")
    issues.extend(validate_findings(findings_text, expected_files, path, "Coverage Findings"))
    finding_blocks_parsed = finding_blocks(findings_text)
    for index, block in enumerate(finding_blocks_parsed, start=1):
        if not block.get("Target ID"):
            issues.append({"path": str(path), "section": "Coverage Findings", "finding": index, "missing_fields": ["Target ID"]})
    finding_target_ids = {block.get("Target ID", "") for block in finding_blocks_parsed}
    missing_untested_findings = sorted(untested_targets - finding_target_ids)
    if missing_untested_findings:
        issues.append({"path": str(path), "section": "Coverage Findings", "reason": "every UNTESTED target requires a finding bound by Target ID", "missing_targets": missing_untested_findings})
    if not bodies.get("batch summary", "").strip():
        issues.append({"path": str(path), "section": "Batch Summary", "reason": "empty"})
    if not bodies.get("no gap notes", "").strip():
        issues.append({"path": str(path), "section": "No Gap Notes", "reason": "empty"})
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
    finding_section = "findings"
    issues.extend(validate_findings(bodies.get(finding_section, ""), allowed_files, path, "Findings"))
    for section in expected_sections:
        if not bodies.get(section, "").strip():
            issues.append({"path": str(path), "section": section, "reason": "empty"})
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
        "audit_kind": "test-coverage",
        "manifest": "manifest.json",
        "audit_index": "audit_index.md",
        "effort_ledger": "effort_ledger.json",
        "excluded_files": "excluded_files.json",
        "reports_dir": "reports",
        "ownership_marker": ".full-repo-test-coverage-audit-artifacts.json",
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


def verify_effort_ledger(manifest_path: Path, manifest: dict) -> list[dict]:
    ledger_path = manifest_path.parent / "effort_ledger.json"
    if not ledger_path.is_file():
        return [{"path": str(ledger_path), "reason": "effort_ledger.json is missing"}]
    ledger = common.load_json_object(ledger_path, "effort_ledger.json")
    issues: list[dict] = []
    if ledger.get("run_id") != manifest["run_id"]:
        issues.append({"path": str(ledger_path), "field": "run_id", "expected": manifest["run_id"], "actual": ledger.get("run_id")})
    capability = ledger.get("subagent_capability_check", {})
    if not isinstance(capability, dict):
        issues.append({"path": str(ledger_path), "field": "subagent_capability_check", "reason": "must be an object"})
    else:
        if capability.get("status") != "completed":
            issues.append({"path": str(ledger_path), "field": "subagent_capability_check.status", "expected": "completed", "actual": capability.get("status")})
        if not isinstance(capability.get("can_set_reasoning_effort"), bool):
            issues.append({"path": str(ledger_path), "field": "subagent_capability_check.can_set_reasoning_effort", "expected": "boolean", "actual": capability.get("can_set_reasoning_effort")})
        if not capability.get("spawn_tool"):
            issues.append({"path": str(ledger_path), "field": "subagent_capability_check.spawn_tool", "expected": "non-empty string", "actual": capability.get("spawn_tool")})
    lead = ledger.get("lead_effort", {})
    if not isinstance(lead, dict) or lead.get("status") not in {"completed", "confirmed", "manual-fallback-completed"}:
        issues.append({"path": str(ledger_path), "field": "lead_effort.status", "expected": "completed/confirmed", "actual": lead.get("status") if isinstance(lead, dict) else None})
    elif lead.get("actual_reasoning_effort") not in {"high", "xhigh", "high-or-higher"}:
        issues.append({"path": str(ledger_path), "field": "lead_effort.actual_reasoning_effort", "expected": "high or xhigh", "actual": lead.get("actual_reasoning_effort")})
    if isinstance(lead, dict) and not lead.get("runtime_provenance"):
        issues.append({"path": str(ledger_path), "field": "lead_effort.runtime_provenance", "expected": "non-empty string", "actual": lead.get("runtime_provenance")})
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
            if row.get("actual_reasoning_effort") != "low" and row.get("status") != "manual-fallback-completed":
                issues.append({"path": str(ledger_path), "batch_id": batch["id"], "field": "actual_reasoning_effort", "expected": "low", "actual": row.get("actual_reasoning_effort")})
            if not row.get("agent_id"):
                issues.append({"path": str(ledger_path), "batch_id": batch["id"], "field": "agent_id", "expected": "non-empty string", "actual": row.get("agent_id")})
            if not row.get("runtime_provenance"):
                issues.append({"path": str(ledger_path), "batch_id": batch["id"], "field": "runtime_provenance", "expected": "non-empty string", "actual": row.get("runtime_provenance")})
    coverage = manifest.get("test_coverage_audit", {})
    if coverage.get("ui_required"):
        for key in ("ui_test_coverage_worker", "visual_e2e_coverage_worker"):
            row = ledger.get(key, {})
            if not isinstance(row, dict) or row.get("status") not in {"completed", "manual-fallback-completed"}:
                issues.append({"path": str(ledger_path), "field": f"{key}.status", "actual": row.get("status") if isinstance(row, dict) else None})
            else:
                if row.get("actual_reasoning_effort") != "low" and row.get("status") != "manual-fallback-completed":
                    issues.append({"path": str(ledger_path), "field": f"{key}.actual_reasoning_effort", "expected": "low", "actual": row.get("actual_reasoning_effort")})
                if not row.get("agent_id"):
                    issues.append({"path": str(ledger_path), "field": f"{key}.agent_id", "expected": "non-empty string", "actual": row.get("agent_id")})
                if not row.get("runtime_provenance"):
                    issues.append({"path": str(ledger_path), "field": f"{key}.runtime_provenance", "expected": "non-empty string", "actual": row.get("runtime_provenance")})
    return issues


def verify_current_hashes(manifest: dict) -> list[dict]:
    repo_root = Path(manifest["repo_root"])
    issues: list[dict] = []
    for rel_path, expected in manifest["_source_hashes"].items():
        path = repo_root / rel_path
        if not path.is_file():
            issues.append({"path": str(path), "reason": "source file is missing"})
            continue
        actual = common.sha256_file(path)
        if actual != expected:
            issues.append({"path": rel_path, "expected": expected, "actual": actual, "reason": "current source hash differs from manifest"})
    return issues


def verify(manifest_path: Path, reports: list[Path], *, skip_current_hash_check: bool = False) -> dict:
    manifest = load_manifest(manifest_path)
    issues: dict[str, list] = {
        "completion_marker_mismatches": verify_marker(manifest_path, manifest),
        "excluded_file_issues": [],
        "effort_ledger_issues": verify_effort_ledger(manifest_path, manifest),
        "missing_reports": [],
        "report_issues": [],
        "current_hash_mismatches": [],
        "empirical_coverage_issues": manifest["_empirical_issues"],
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

    coverage = manifest.get("test_coverage_audit", {})
    all_source_files = set(manifest["_source_hashes"])
    if coverage.get("ui_required"):
        ui_report = reports_by_name.get("ui_test_coverage_audit.md")
        visual_report = reports_by_name.get("visual_e2e_coverage_audit.md")
        if not ui_report:
            issues["missing_reports"].append({"report": "ui_test_coverage_audit.md"})
        else:
            issues["report_issues"].extend(verify_aux_report(ui_report, manifest, UI_SECTIONS, "ui_test_coverage", all_source_files))
        if not visual_report:
            issues["missing_reports"].append({"report": "visual_e2e_coverage_audit.md"})
        else:
            issues["report_issues"].extend(verify_aux_report(visual_report, manifest, VISUAL_SECTIONS, "visual_e2e_coverage", all_source_files))

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
