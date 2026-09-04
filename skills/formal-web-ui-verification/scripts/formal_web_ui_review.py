#!/usr/bin/env python3
"""Finalize or validate manual review of changed formal Web UI evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path


SCHEMA_VERSION = 1
KIND = "formal-web-ui-manual-review"
QUEUE_KIND = "formal-web-ui-review-queue"
DECISIONS = {"pass", "gap", "blocked"}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_object(path: Path, label: str) -> dict:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"{label} must be a regular non-symlink JSON file: {path}")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must contain a JSON object")
    return value


def load_inputs(path: Path | None) -> list[dict]:
    if path is None:
        return []
    value = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(value, dict):
        value = value.get("decisions")
    if not isinstance(value, list):
        raise ValueError("decision input must be a JSON list or an object with a decisions list")
    return value


def screenshot_hashes(cell: dict) -> dict[str, str]:
    screenshots = cell.get("screenshots")
    if not isinstance(screenshots, dict):
        raise ValueError(f"review cell {cell.get('reviewCellKey')} is missing screenshots")
    result: dict[str, str] = {}
    for key, output_key in (("viewport", "viewportSha256"), ("fullPage", "fullPageSha256")):
        item = screenshots.get(key)
        if not isinstance(item, dict):
            raise ValueError(f"review cell {cell.get('reviewCellKey')} is missing {key} screenshot evidence")
        artifact = Path(str(item.get("path", "")))
        expected = item.get("sha256")
        if not artifact.is_file() or artifact.is_symlink():
            raise ValueError(f"review screenshot is unavailable or symlinked: {artifact}")
        actual = sha256_file(artifact)
        if expected != actual:
            raise ValueError(f"review screenshot hash mismatch: {artifact}")
        result[output_key] = actual
    return result


def load_run(report_path: Path, queue_path: Path) -> tuple[dict, dict, list[dict]]:
    report = load_object(report_path, "report")
    queue = load_object(queue_path, "review queue")
    if report.get("schemaVersion") != 2:
        raise ValueError("report schemaVersion must be 2")
    if queue.get("schemaVersion") != 1 or queue.get("kind") != QUEUE_KIND:
        raise ValueError("review queue has an unsupported schema or kind")
    if report.get("runId") != queue.get("runId"):
        raise ValueError("report and review queue run ids differ")
    review = report.get("review")
    if not isinstance(review, dict) or not isinstance(review.get("cells"), list):
        raise ValueError("report is missing changed visual-review cells")
    queue_hash = sha256_file(queue_path)
    if review.get("queueSha256") != queue_hash:
        raise ValueError("report does not bind the supplied review queue bytes")
    cells = review["cells"]
    keys = [cell.get("reviewCellKey") for cell in cells]
    if any(not isinstance(key, str) or not key for key in keys) or len(keys) != len(set(keys)):
        raise ValueError("report review cells require unique non-empty reviewCellKey values")
    queued_keys = {
        item.get("reviewCellKey")
        for item in queue.get("entries", [])
        if isinstance(item, dict) and isinstance(item.get("reviewCellKey"), str)
    }
    expected_queued = {cell["reviewCellKey"] for cell in cells if cell.get("status") == "review-required"}
    if queued_keys != expected_queued or len(queued_keys) != len(queue.get("entries", [])):
        raise ValueError("review queue entries do not exactly match report review-required cells")
    return report, queue, cells


def normalize_agent_decisions(raw: list[dict], required_keys: set[str]) -> dict[str, dict]:
    decisions: dict[str, dict] = {}
    for index, item in enumerate(raw):
        if not isinstance(item, dict):
            raise ValueError(f"decision {index} must be an object")
        key = item.get("reviewCellKey")
        decision = item.get("decision")
        note = item.get("note", "")
        if not isinstance(key, str) or not key:
            raise ValueError(f"decision {index} requires reviewCellKey")
        if key in decisions:
            raise ValueError(f"duplicate decision for {key}")
        if decision not in DECISIONS:
            raise ValueError(f"decision for {key} must be pass, gap, or blocked")
        if decision != "pass" and (not isinstance(note, str) or not note.strip()):
            raise ValueError(f"{decision} decision for {key} requires a note")
        decisions[key] = {"decision": decision, "note": str(note).strip()}
    missing = sorted(required_keys - set(decisions))
    extra = sorted(set(decisions) - required_keys)
    if missing or extra:
        raise ValueError(f"decision keys must exactly match the review queue; missing={missing}; extra={extra}")
    return decisions


def finalize(report_path: Path, queue_path: Path, decision_path: Path | None) -> dict:
    report, queue, cells = load_run(report_path, queue_path)
    queued_keys = {
        item.get("reviewCellKey")
        for item in queue.get("entries", [])
        if isinstance(item, dict) and isinstance(item.get("reviewCellKey"), str)
    }
    if len(queued_keys) != len(queue.get("entries", [])):
        raise ValueError("review queue entries require unique non-empty reviewCellKey values")
    supplied = normalize_agent_decisions(load_inputs(decision_path), queued_keys)
    decisions: list[dict] = []
    for cell in cells:
        key = cell["reviewCellKey"]
        status = cell.get("status")
        if status == "review-required":
            selected = supplied[key]
            basis = "agent-reviewed-current-screenshots"
        elif isinstance(status, str) and status.startswith("carried-"):
            selected = {"decision": cell.get("decision"), "note": cell.get("note", "")}
            if selected["decision"] not in DECISIONS:
                raise ValueError(f"carried review cell {key} has no valid prior decision")
            basis = "carried-unchanged-ui-inputs-and-intent"
        else:
            raise ValueError(f"review cell {key} has unsupported status {status}")
        decisions.append(
            {
                "reviewCellKey": key,
                "targetName": cell.get("targetName"),
                "requestedPath": cell.get("requestedPath"),
                "stateName": cell.get("stateName"),
                "viewport": cell.get("viewport"),
                "decision": selected["decision"],
                "note": selected.get("note", ""),
                "basis": basis,
                "sourceFingerprint": cell.get("sourceFingerprint"),
                "intentFingerprint": cell.get("intentFingerprint"),
                "screenshots": screenshot_hashes(cell),
            }
        )
    return {
        "schemaVersion": SCHEMA_VERSION,
        "kind": KIND,
        "reviewedRunId": report["runId"],
        "reviewedAt": datetime.now(timezone.utc).isoformat(),
        "reportSha256": sha256_file(report_path),
        "reviewQueueSha256": sha256_file(queue_path),
        "decisions": decisions,
    }


def validate(review_path: Path, report_path: Path, queue_path: Path) -> dict:
    report, _queue, cells = load_run(report_path, queue_path)
    actual = load_object(review_path, "manual review")
    if actual.get("schemaVersion") != SCHEMA_VERSION or actual.get("kind") != KIND:
        raise ValueError("manual review has an unsupported schema or kind")
    expected_bindings = {
        "reviewedRunId": report["runId"],
        "reportSha256": sha256_file(report_path),
        "reviewQueueSha256": sha256_file(queue_path),
    }
    for field, value in expected_bindings.items():
        if actual.get(field) != value:
            raise ValueError(f"manual review {field} does not match the supplied run evidence")
    actual_decisions = actual.get("decisions")
    if not isinstance(actual_decisions, list):
        raise ValueError("manual review decisions must be a list")
    expected_cells = {
        item["reviewCellKey"]: {
            "sourceFingerprint": item.get("sourceFingerprint"),
            "intentFingerprint": item.get("intentFingerprint"),
            "screenshots": screenshot_hashes(item),
        }
        for item in cells
    }
    seen: set[str] = set()
    for index, item in enumerate(actual_decisions):
        if not isinstance(item, dict) or item.get("reviewCellKey") not in expected_cells:
            raise ValueError(f"manual review decision {index} references an unknown cell")
        key = item["reviewCellKey"]
        if key in seen:
            raise ValueError(f"manual review repeats cell {key}")
        seen.add(key)
        if item.get("decision") not in DECISIONS:
            raise ValueError(f"manual review decision for {key} is invalid")
        if item.get("decision") != "pass" and not str(item.get("note", "")).strip():
            raise ValueError(f"manual review {item.get('decision')} for {key} requires a note")
        for field in ("sourceFingerprint", "intentFingerprint", "screenshots"):
            if item.get(field) != expected_cells[key].get(field):
                raise ValueError(f"manual review decision for {key} does not bind current {field}")
    missing = sorted(set(expected_cells) - seen)
    if missing:
        raise ValueError(f"manual review omits current cells: {missing}")
    return actual


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--queue", type=Path, required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--decisions", type=Path, help="Agent decisions JSON used to finalize the current review.")
    mode.add_argument("--review", type=Path, help="Existing manual-review manifest to validate.")
    parser.add_argument("--out", type=Path, help="Output manual-review path; required with --decisions.")
    args = parser.parse_args()
    try:
        if args.review:
            review = validate(args.review, args.report, args.queue)
        else:
            if args.out is None:
                raise ValueError("--out is required with --decisions")
            review = finalize(args.report, args.queue, args.decisions)
            args.out.parent.mkdir(parents=True, exist_ok=True)
            if args.out.exists() or args.out.is_symlink():
                raise ValueError("manual-review output must be a new non-symlink file")
            with args.out.open("x", encoding="utf-8") as handle:
                handle.write(json.dumps(review, indent=2, sort_keys=True) + "\n")
        blocking = [item for item in review["decisions"] if item["decision"] != "pass"]
        print(
            json.dumps(
                {
                    "ok": not blocking,
                    "reviewedRunId": review["reviewedRunId"],
                    "decisionCount": len(review["decisions"]),
                    "blockingCount": len(blocking),
                },
                sort_keys=True,
            )
        )
        return 0 if not blocking else 1
    except (KeyError, OSError, ValueError, json.JSONDecodeError) as error:
        print(json.dumps({"ok": False, "error": str(error)}, sort_keys=True))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
