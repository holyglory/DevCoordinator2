#!/usr/bin/env python3
"""Import a formal Web UI evidence bundle into a UI-audit evidence manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
REPO_ROOT = Path(__file__).resolve().parents[3]
VENDOR_ROOT = SCRIPT_DIR / "_vendor"
DEV_SKILL_DIR = (REPO_ROOT / "skills" / "ui-implementation-audit").resolve()
running_in_dev_repo = DEV_SKILL_DIR == SKILL_DIR.resolve() and (REPO_ROOT / "full_repo_harness" / "evidence.py").is_file()
for root in reversed([item for item in ([REPO_ROOT, VENDOR_ROOT] if running_in_dev_repo else [VENDOR_ROOT]) if item.is_dir()]):
    root_text = str(root)
    if root_text not in sys.path:
        sys.path.insert(0, root_text)

from full_repo_harness import evidence as audit_evidence


EVIDENCE_FILE = "visual_evidence.json"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_object(path: Path, label: str) -> dict:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"{label} must be a regular non-symlink JSON file")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"{label} must contain valid UTF-8 JSON: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{label} must contain a JSON object")
    return value


def confined_path(root: Path, path: Path, label: str) -> tuple[Path, str]:
    if not path.is_absolute():
        path = (root / path).resolve(strict=False)
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"{label} must be an existing regular non-symlink file")
    resolved = path.resolve()
    try:
        relative = resolved.relative_to(root)
    except ValueError as error:
        raise ValueError(f"{label} must remain inside the audit output") from error
    return resolved, relative.as_posix()


def safe_id(value: str) -> str:
    folded = re.sub(r"[^A-Za-z0-9_-]+", "-", value).strip("-")
    if not folded or not folded[0].isalpha():
        folded = f"cell-{folded or 'unknown'}"
    return folded[:40]


def screenshot_record(root: Path, page: dict, role: str) -> dict:
    screenshot = page.get("screenshots", {}).get(role)
    if not isinstance(screenshot, dict):
        raise ValueError(f"formal page {page.get('cellId')} lacks {role} screenshot evidence")
    path, relative = confined_path(root, Path(str(screenshot.get("path", ""))), f"{role} screenshot")
    actual = sha256_file(path)
    if screenshot.get("sha256") != actual:
        raise ValueError(f"{role} screenshot hash does not match the formal report")
    viewport = page.get("viewport") if isinstance(page.get("viewport"), dict) else {}
    return {
        "id": f"formal-{safe_id(str(page.get('cellId', 'cell')))}-{role.lower()}",
        "kind": "screenshot",
        "path": relative,
        "sha256": actual,
        "mime": "image/png",
        "route": str(page.get("finalPath") or page.get("requestedPath") or "/"),
        "state": str(page.get("target", {}).get("stateName") or "base"),
        "viewport": {
            "width": int(viewport.get("width") or screenshot.get("width") or 1),
            "height": int(viewport.get("height") or screenshot.get("height") or 1),
            "label": str(viewport.get("name") or role),
        },
        "captured_by": "formal-web-ui-verification",
        "width": int(screenshot.get("width") or 1),
        "height": int(screenshot.get("height") or 1),
    }


def json_record(root: Path, path: Path, record_id: str, kind: str, *, formal: dict | None = None) -> dict:
    resolved, relative = confined_path(root, path, record_id)
    record = {
        "id": record_id,
        "kind": kind,
        "path": relative,
        "sha256": sha256_file(resolved),
        "mime": "application/json",
        "captured_by": "formal-web-ui-verification evidence importer",
    }
    if kind == "formal-web-verifier":
        pages = [item for item in (formal or {}).get("pages", []) if isinstance(item, dict)]
        first = pages[0] if pages else {}
        viewport = first.get("viewport") if isinstance(first.get("viewport"), dict) else {}
        record.update(
            {
                "route": "multiple formal web targets",
                "state": "multiple formal web states",
                "viewport": {
                    "width": int(viewport.get("width") or 1),
                    "height": int(viewport.get("height") or 1),
                    "label": "formal web route/state/viewport set",
                },
            }
        )
    return record


def merge_records(existing: list[dict], imported: list[dict]) -> list[dict]:
    by_id = {item.get("id"): item for item in existing if isinstance(item, dict) and isinstance(item.get("id"), str)}
    if len(by_id) != len(existing):
        raise ValueError("existing visual evidence contains invalid or duplicate ids")
    for record in imported:
        previous = by_id.get(record["id"])
        if previous is not None and previous != record:
            raise ValueError(f"visual evidence id collision: {record['id']}")
        by_id[record["id"]] = record
    return [by_id[key] for key in sorted(by_id)]


def import_bundle(audit_root: Path, run_id: str, report_path: Path, queue_path: Path, review_path: Path) -> dict:
    root = audit_root.resolve()
    manifest_path = root / EVIDENCE_FILE
    manifest = load_object(manifest_path, EVIDENCE_FILE)
    if manifest.get("schema_version") != 1 or manifest.get("run_id") != run_id:
        raise ValueError("visual evidence manifest schema or audit run id does not match")
    report_resolved, _ = confined_path(root, report_path, "formal report")
    queue_resolved, _ = confined_path(root, queue_path, "formal review queue")
    review_resolved, _ = confined_path(root, review_path, "formal manual review")
    report = load_object(report_resolved, "formal report")
    queue = load_object(queue_resolved, "formal review queue")
    review = load_object(review_resolved, "formal manual review")
    formal_run_id = report.get("runId")
    if report.get("schemaVersion") != 2 or not isinstance(formal_run_id, str):
        raise ValueError("formal report must use schemaVersion 2 and contain runId")
    if queue.get("kind") != "formal-web-ui-review-queue" or queue.get("runId") != formal_run_id:
        raise ValueError("review queue does not belong to the formal report")
    if review.get("kind") != "formal-web-ui-manual-review" or review.get("reviewedRunId") != formal_run_id:
        raise ValueError("manual review does not belong to the formal report")
    if review.get("reportSha256") != sha256_file(report_resolved):
        raise ValueError("manual review does not bind the formal report bytes")
    if review.get("reviewQueueSha256") != sha256_file(queue_resolved):
        raise ValueError("manual review does not bind the review queue bytes")

    checked_pages = [item for item in report.get("pages", []) if isinstance(item, dict) and item.get("outcome") == "checked"]
    if not checked_pages:
        raise ValueError("formal report contains no checked pages to import")
    imported = [
        json_record(root, report_resolved, "formal-web", "formal-web-verifier", formal=report),
        json_record(root, queue_resolved, "formal-review-queue", "review-queue"),
        json_record(root, review_resolved, "formal-manual-review", "manual-review"),
    ]
    for page in checked_pages:
        imported.append(screenshot_record(root, page, "viewport"))
        imported.append(screenshot_record(root, page, "fullPage"))
    updated = {**manifest, "artifacts": merge_records(manifest.get("artifacts", []), imported)}
    original = manifest_path.read_bytes()
    manifest_path.write_text(json.dumps(updated, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    _, issues = audit_evidence.validate_visual_evidence_manifest(root, run_id, required=True)
    if issues:
        manifest_path.write_bytes(original)
        raise ValueError("imported evidence failed validation: " + json.dumps(issues[:5], sort_keys=True))
    return {
        "ok": True,
        "auditRunId": run_id,
        "formalRunId": formal_run_id,
        "importedIds": [item["id"] for item in imported],
        "evidenceManifest": str(manifest_path),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audit-root", type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--formal-report", type=Path, required=True)
    parser.add_argument("--review-queue", type=Path, required=True)
    parser.add_argument("--manual-review", type=Path, required=True)
    args = parser.parse_args()
    try:
        result = import_bundle(args.audit_root, args.run_id, args.formal_report, args.review_queue, args.manual_review)
        print(json.dumps(result, sort_keys=True))
        return 0
    except (OSError, ValueError) as error:
        print(json.dumps({"ok": False, "error": str(error)}, sort_keys=True))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
