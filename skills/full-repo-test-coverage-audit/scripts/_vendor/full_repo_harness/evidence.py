"""Attested audit artifact manifests shared by repository-audit skills."""

from __future__ import annotations

import json
import re
import struct
from pathlib import Path
from typing import Any

from .verify_common import SHA256_RE, sha256_file


SCHEMA_VERSION = 1
VISUAL_EVIDENCE_FILENAME = "visual_evidence.json"
EVIDENCE_ID_RE = re.compile(r"^[A-Za-z][A-Za-z0-9_-]{0,63}$")
REFERENCE_RE = re.compile(r"\bevidence:([A-Za-z][A-Za-z0-9_-]{0,63})\b", re.IGNORECASE)
ALLOWED_KINDS = {
    "screenshot",
    "native-snapshot",
    "trace",
    "video",
    "formal-web-verifier",
    "review-queue",
    "manual-review",
}
IMAGE_KINDS = {"screenshot", "native-snapshot"}
SHA256_VALUE_RE = re.compile(r"^[0-9a-f]{64}$")


def evidence_references(text: str) -> set[str]:
    return {match.group(1) for match in REFERENCE_RE.finditer(text)}


def _inside(root: Path, child: Path) -> bool:
    try:
        child.relative_to(root)
        return True
    except ValueError:
        return False


def _detected_mime(data: bytes) -> str | None:
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if data.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if data.startswith((b"GIF87a", b"GIF89a")):
        return "image/gif"
    if data.startswith(b"RIFF") and data[8:12] == b"WEBP":
        return "image/webp"
    if data.startswith(b"PK\x03\x04"):
        return "application/zip"
    if len(data) >= 12 and data[4:8] == b"ftyp":
        return "video/mp4"
    try:
        json.loads(data.decode("utf-8"))
        return "application/json"
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None


def _jpeg_dimensions(data: bytes) -> tuple[int, int] | None:
    cursor = 2
    while cursor + 9 < len(data):
        if data[cursor] != 0xFF:
            cursor += 1
            continue
        marker = data[cursor + 1]
        cursor += 2
        if marker in {0xD8, 0xD9}:
            continue
        if cursor + 2 > len(data):
            return None
        length = int.from_bytes(data[cursor : cursor + 2], "big")
        if length < 2 or cursor + length > len(data):
            return None
        if marker in {0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF}:
            height = int.from_bytes(data[cursor + 3 : cursor + 5], "big")
            width = int.from_bytes(data[cursor + 5 : cursor + 7], "big")
            return width, height
        cursor += length
    return None


def image_dimensions(data: bytes, mime: str) -> tuple[int, int] | None:
    if mime == "image/png" and len(data) >= 24:
        return struct.unpack(">II", data[16:24])
    if mime == "image/gif" and len(data) >= 10:
        return struct.unpack("<HH", data[6:10])
    if mime == "image/jpeg":
        return _jpeg_dimensions(data)
    if mime == "image/webp" and len(data) >= 30:
        chunk = data[12:16]
        if chunk == b"VP8X":
            return int.from_bytes(data[24:27], "little") + 1, int.from_bytes(data[27:30], "little") + 1
    return None


def _validate_formal_report(path: Path, record_id: str) -> list[dict[str, Any]]:
    issues: list[dict[str, Any]] = []
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        return [{"record": record_id, "field": "path", "reason": f"formal verifier evidence is not valid JSON: {error}"}]
    if not isinstance(payload, dict):
        return [{"record": record_id, "field": "path", "reason": "formal verifier JSON must be an object"}]
    if payload.get("schemaVersion") != 2:
        issues.append({"record": record_id, "field": "schemaVersion", "expected": 2, "actual": payload.get("schemaVersion")})
    for field, expected_type in (("runId", str), ("pages", list), ("findings", list), ("coverage", dict)):
        if not isinstance(payload.get(field), expected_type):
            issues.append({"record": record_id, "field": field, "reason": f"formal verifier JSON requires {expected_type.__name__}"})
    coverage = payload.get("coverage") if isinstance(payload.get("coverage"), dict) else {}
    checked_pages = coverage.get("checkedPages")
    if not isinstance(checked_pages, int) or isinstance(checked_pages, bool) or checked_pages < 1:
        issues.append({"record": record_id, "field": "coverage.checkedPages", "reason": "formal verifier evidence must include at least one checked page"})
    if not isinstance(coverage.get("failed"), bool):
        issues.append({"record": record_id, "field": "coverage.failed", "reason": "formal verifier evidence must preserve coverage status"})
    for index, page in enumerate(payload.get("pages", []) if isinstance(payload.get("pages"), list) else []):
        if not isinstance(page, dict) or page.get("outcome") != "checked":
            continue
        metrics = page.get("metrics") if isinstance(page.get("metrics"), dict) else {}
        if not isinstance(metrics.get("visibleScrollbars"), list):
            issues.append(
                {
                    "record": record_id,
                    "field": f"pages[{index}].metrics.visibleScrollbars",
                    "reason": "checked formal-verifier pages must preserve the visible scrollbar inventory",
                }
            )
        screenshots = page.get("screenshots") if isinstance(page.get("screenshots"), dict) else {}
        for name in ("viewport", "fullPage"):
            screenshot = screenshots.get(name)
            if not isinstance(screenshot, dict):
                issues.append({"record": record_id, "field": f"pages[{index}].screenshots.{name}", "reason": "checked pages require both screenshot evidence records"})
                continue
            for field in ("path", "mime", "sha256", "width", "height"):
                if field not in screenshot:
                    issues.append({"record": record_id, "field": f"pages[{index}].screenshots.{name}.{field}", "reason": "missing screenshot evidence field"})
            if not SHA256_VALUE_RE.fullmatch(str(screenshot.get("sha256", ""))):
                issues.append({"record": record_id, "field": f"pages[{index}].screenshots.{name}.sha256", "reason": "must be SHA-256"})
        review = page.get("review") if isinstance(page.get("review"), dict) else {}
        for field in ("reviewCellKey", "sourceFingerprint", "intentFingerprint"):
            value = review.get(field)
            if not isinstance(value, str) or not value:
                issues.append({"record": record_id, "field": f"pages[{index}].review.{field}", "reason": "checked pages require changed-review identity"})
        for field in ("sourceFingerprint", "intentFingerprint"):
            if not SHA256_VALUE_RE.fullmatch(str(review.get(field, ""))):
                issues.append({"record": record_id, "field": f"pages[{index}].review.{field}", "reason": "must be SHA-256"})
    review = payload.get("review") if isinstance(payload.get("review"), dict) else {}
    if not SHA256_VALUE_RE.fullmatch(str(review.get("queueSha256", ""))):
        issues.append({"record": record_id, "field": "review.queueSha256", "reason": "formal report must bind its changed visual-review queue"})
    if not isinstance(review.get("cells"), list):
        issues.append({"record": record_id, "field": "review.cells", "reason": "formal report must preserve changed-review cells"})
    return issues


def _load_json_artifact(path: Path, record_id: str, label: str) -> tuple[dict[str, Any] | None, list[dict[str, Any]]]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        return None, [{"record": record_id, "field": "path", "reason": f"{label} is not valid JSON: {error}"}]
    if not isinstance(payload, dict):
        return None, [{"record": record_id, "field": "path", "reason": f"{label} must be a JSON object"}]
    return payload, []


def _validate_review_queue(path: Path, record_id: str) -> tuple[dict[str, Any] | None, list[dict[str, Any]]]:
    payload, issues = _load_json_artifact(path, record_id, "review queue")
    if payload is None:
        return None, issues
    if payload.get("schemaVersion") != 1 or payload.get("kind") != "formal-web-ui-review-queue":
        issues.append({"record": record_id, "field": "schema/kind", "reason": "unsupported formal review queue"})
    if not isinstance(payload.get("runId"), str) or not payload.get("runId"):
        issues.append({"record": record_id, "field": "runId", "reason": "review queue requires a run id"})
    entries = payload.get("entries")
    if not isinstance(entries, list):
        issues.append({"record": record_id, "field": "entries", "reason": "review queue entries must be a list"})
        entries = []
    seen: set[str] = set()
    for index, item in enumerate(entries):
        if not isinstance(item, dict):
            issues.append({"record": record_id, "field": f"entries[{index}]", "reason": "must be an object"})
            continue
        key = item.get("reviewCellKey")
        if not isinstance(key, str) or not key or key in seen:
            issues.append({"record": record_id, "field": f"entries[{index}].reviewCellKey", "reason": "must be unique and non-empty"})
        else:
            seen.add(key)
        for field in ("sourceFingerprint", "intentFingerprint"):
            if not SHA256_VALUE_RE.fullmatch(str(item.get(field, ""))):
                issues.append({"record": record_id, "field": f"entries[{index}].{field}", "reason": "must be SHA-256"})
        screenshots = item.get("screenshots") if isinstance(item.get("screenshots"), dict) else {}
        for name in ("viewport", "fullPage"):
            if not SHA256_VALUE_RE.fullmatch(str((screenshots.get(name) or {}).get("sha256", ""))):
                issues.append({"record": record_id, "field": f"entries[{index}].screenshots.{name}", "reason": "must bind screenshot SHA-256"})
    return payload, issues


def _validate_manual_review(path: Path, record_id: str) -> tuple[dict[str, Any] | None, list[dict[str, Any]]]:
    payload, issues = _load_json_artifact(path, record_id, "manual review")
    if payload is None:
        return None, issues
    if payload.get("schemaVersion") != 1 or payload.get("kind") != "formal-web-ui-manual-review":
        issues.append({"record": record_id, "field": "schema/kind", "reason": "unsupported formal manual-review manifest"})
    if not isinstance(payload.get("reviewedRunId"), str) or not payload.get("reviewedRunId"):
        issues.append({"record": record_id, "field": "reviewedRunId", "reason": "manual review requires a reviewed run id"})
    for field in ("reportSha256", "reviewQueueSha256"):
        if not SHA256_VALUE_RE.fullmatch(str(payload.get(field, ""))):
            issues.append({"record": record_id, "field": field, "reason": "must be SHA-256"})
    decisions = payload.get("decisions")
    if not isinstance(decisions, list) or not decisions:
        issues.append({"record": record_id, "field": "decisions", "reason": "manual review requires at least one decision"})
        decisions = []
    seen: set[str] = set()
    for index, item in enumerate(decisions):
        if not isinstance(item, dict):
            issues.append({"record": record_id, "field": f"decisions[{index}]", "reason": "must be an object"})
            continue
        key = item.get("reviewCellKey")
        if not isinstance(key, str) or not key or key in seen:
            issues.append({"record": record_id, "field": f"decisions[{index}].reviewCellKey", "reason": "must be unique and non-empty"})
        else:
            seen.add(key)
        if item.get("decision") not in {"pass", "gap", "blocked"}:
            issues.append({"record": record_id, "field": f"decisions[{index}].decision", "reason": "must be pass, gap, or blocked"})
        if item.get("decision") != "pass" and len(str(item.get("note", "")).strip()) < 2:
            issues.append({"record": record_id, "field": f"decisions[{index}].note", "reason": "gap/blocked decisions require a note"})
        for field in ("sourceFingerprint", "intentFingerprint"):
            if not SHA256_VALUE_RE.fullmatch(str(item.get(field, ""))):
                issues.append({"record": record_id, "field": f"decisions[{index}].{field}", "reason": "must be SHA-256"})
        screenshots = item.get("screenshots") if isinstance(item.get("screenshots"), dict) else {}
        for field in ("viewportSha256", "fullPageSha256"):
            if not SHA256_VALUE_RE.fullmatch(str(screenshots.get(field, ""))):
                issues.append({"record": record_id, "field": f"decisions[{index}].screenshots.{field}", "reason": "must be SHA-256"})
    return payload, issues


def validate_visual_evidence_manifest(
    audit_root: Path,
    expected_run_id: str,
    *,
    required: bool,
) -> tuple[dict[str, dict[str, Any]], list[dict[str, Any]]]:
    """Validate real artifact files and return records keyed by stable evidence id."""

    path = audit_root / VISUAL_EVIDENCE_FILENAME
    if not path.is_file() or path.is_symlink():
        return {}, ([{"path": str(path), "reason": "visual evidence manifest is missing"}] if required else [])
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        return {}, [{"path": str(path), "reason": f"visual evidence manifest is invalid JSON: {error}"}]
    if not isinstance(payload, dict):
        return {}, [{"path": str(path), "reason": "visual evidence manifest must be a JSON object"}]
    issues: list[dict[str, Any]] = []
    if payload.get("schema_version") != SCHEMA_VERSION:
        issues.append({"path": str(path), "field": "schema_version", "expected": SCHEMA_VERSION, "actual": payload.get("schema_version")})
    if payload.get("run_id") != expected_run_id:
        issues.append({"path": str(path), "field": "run_id", "expected": expected_run_id, "actual": payload.get("run_id")})
    artifacts = payload.get("artifacts")
    if not isinstance(artifacts, list):
        return {}, issues + [{"path": str(path), "field": "artifacts", "reason": "must be a list"}]
    root = audit_root.resolve()
    records: dict[str, dict[str, Any]] = {}
    actual_shas: dict[str, str] = {}
    formal_payloads: dict[str, dict[str, Any]] = {}
    queue_payloads: dict[str, dict[str, Any]] = {}
    review_payloads: dict[str, dict[str, Any]] = {}
    for index, record in enumerate(artifacts):
        if not isinstance(record, dict):
            issues.append({"path": str(path), "record": index, "reason": "artifact record must be an object"})
            continue
        record_id = record.get("id")
        if not isinstance(record_id, str) or not EVIDENCE_ID_RE.fullmatch(record_id):
            issues.append({"path": str(path), "record": index, "field": "id", "reason": "invalid evidence id"})
            continue
        if record_id in records:
            issues.append({"path": str(path), "record": record_id, "reason": "duplicate evidence id"})
            continue
        records[record_id] = record
        kind = record.get("kind")
        if kind not in ALLOWED_KINDS:
            issues.append({"path": str(path), "record": record_id, "field": "kind", "expected": sorted(ALLOWED_KINDS), "actual": kind})
        raw_artifact_path = record.get("path")
        if not isinstance(raw_artifact_path, str) or not raw_artifact_path or Path(raw_artifact_path).is_absolute() or ".." in Path(raw_artifact_path).parts:
            issues.append({"path": str(path), "record": record_id, "field": "path", "reason": "must be a confined relative path"})
            continue
        artifact_path = audit_root / raw_artifact_path
        resolved = artifact_path.resolve(strict=False)
        if not _inside(root, resolved) or artifact_path.is_symlink() or not artifact_path.is_file():
            issues.append({"path": str(path), "record": record_id, "field": "path", "reason": "artifact must be an existing regular non-symlink file inside the audit output"})
            continue
        data = artifact_path.read_bytes()
        actual_sha = sha256_file(artifact_path)
        actual_shas[record_id] = actual_sha
        if not isinstance(record.get("sha256"), str) or not SHA256_RE.fullmatch(record.get("sha256", "")) or record.get("sha256") != actual_sha:
            issues.append({"path": str(path), "record": record_id, "field": "sha256", "expected": actual_sha, "actual": record.get("sha256")})
        actual_mime = _detected_mime(data)
        if record.get("mime") != actual_mime:
            issues.append({"path": str(path), "record": record_id, "field": "mime", "expected": actual_mime, "actual": record.get("mime")})
        metadata_fields = ("captured_by",) if kind in {"review-queue", "manual-review"} else ("route", "state", "captured_by")
        for field in metadata_fields:
            if not isinstance(record.get(field), str) or len(record.get(field, "").strip()) < 2:
                issues.append({"path": str(path), "record": record_id, "field": field, "reason": "must be a non-empty metadata string"})
        viewport = record.get("viewport")
        if kind in {"review-queue", "manual-review"}:
            viewport = None
        elif not isinstance(viewport, dict):
            issues.append({"path": str(path), "record": record_id, "field": "viewport", "reason": "must be an object"})
        else:
            for field in ("width", "height"):
                value = viewport.get(field)
                if not isinstance(value, int) or isinstance(value, bool) or value < 1:
                    issues.append({"path": str(path), "record": record_id, "field": f"viewport.{field}", "reason": "must be a positive integer"})
            if not isinstance(viewport.get("label"), str) or not viewport.get("label"):
                issues.append({"path": str(path), "record": record_id, "field": "viewport.label", "reason": "must be a non-empty string"})
        if kind in IMAGE_KINDS:
            dimensions = image_dimensions(data, actual_mime or "")
            if not dimensions:
                issues.append({"path": str(path), "record": record_id, "field": "dimensions", "reason": "image dimensions could not be parsed"})
            else:
                width, height = dimensions
                if record.get("width") != width or record.get("height") != height:
                    issues.append(
                        {
                            "path": str(path),
                            "record": record_id,
                            "field": "dimensions",
                            "expected": {"width": width, "height": height},
                            "actual": {"width": record.get("width"), "height": record.get("height")},
                        }
                    )
            if actual_mime not in {"image/png", "image/jpeg", "image/gif", "image/webp"}:
                issues.append({"path": str(path), "record": record_id, "field": "mime", "reason": "screenshot evidence must be a supported raster image"})
        if kind == "formal-web-verifier":
            if actual_mime != "application/json":
                issues.append({"path": str(path), "record": record_id, "field": "mime", "reason": "formal verifier evidence must be JSON"})
            else:
                issues.extend(_validate_formal_report(artifact_path, record_id))
                loaded, load_issues = _load_json_artifact(artifact_path, record_id, "formal verifier report")
                issues.extend(load_issues)
                if loaded is not None:
                    formal_payloads[record_id] = loaded
        if kind == "review-queue":
            if actual_mime != "application/json":
                issues.append({"path": str(path), "record": record_id, "field": "mime", "reason": "review queue evidence must be JSON"})
            else:
                loaded, review_issues = _validate_review_queue(artifact_path, record_id)
                issues.extend(review_issues)
                if loaded is not None:
                    queue_payloads[record_id] = loaded
        if kind == "manual-review":
            if actual_mime != "application/json":
                issues.append({"path": str(path), "record": record_id, "field": "mime", "reason": "manual review evidence must be JSON"})
            else:
                loaded, review_issues = _validate_manual_review(artifact_path, record_id)
                issues.extend(review_issues)
                if loaded is not None:
                    review_payloads[record_id] = loaded
    screenshot_shas = {
        actual_shas[record_id]
        for record_id, record in records.items()
        if record.get("kind") in IMAGE_KINDS and record_id in actual_shas
    }
    for review_id, review_payload in review_payloads.items():
        report_matches = [
            record_id
            for record_id, payload_value in formal_payloads.items()
            if actual_shas.get(record_id) == review_payload.get("reportSha256")
            and payload_value.get("runId") == review_payload.get("reviewedRunId")
        ]
        queue_matches = [
            record_id
            for record_id, payload_value in queue_payloads.items()
            if actual_shas.get(record_id) == review_payload.get("reviewQueueSha256")
            and payload_value.get("runId") == review_payload.get("reviewedRunId")
        ]
        if not report_matches:
            issues.append({"path": str(path), "record": review_id, "reason": "manual review does not bind a registered formal report for the same run"})
        if not queue_matches:
            issues.append({"path": str(path), "record": review_id, "reason": "manual review does not bind a registered review queue for the same run"})
        for index, decision in enumerate(review_payload.get("decisions", [])):
            if not isinstance(decision, dict):
                continue
            screenshots = decision.get("screenshots") if isinstance(decision.get("screenshots"), dict) else {}
            for field in ("viewportSha256", "fullPageSha256"):
                if screenshots.get(field) not in screenshot_shas:
                    issues.append({
                        "path": str(path),
                        "record": review_id,
                        "field": f"decisions[{index}].screenshots.{field}",
                        "reason": "manual review screenshot hash is not registered as screenshot evidence",
                    })
        for report_id in report_matches:
            report_review = formal_payloads[report_id].get("review")
            if isinstance(report_review, dict) and report_review.get("queueSha256") != review_payload.get("reviewQueueSha256"):
                issues.append({"path": str(path), "record": review_id, "reason": "formal report and manual review bind different review queues"})
    if required and not artifacts:
        issues.append({"path": str(path), "field": "artifacts", "reason": "at least one visual evidence artifact is required"})
    return records, issues


def validate_references(
    text: str,
    records: dict[str, dict[str, Any]],
    *,
    required_kinds: set[str] | None = None,
) -> list[dict[str, Any]]:
    references = evidence_references(text)
    issues: list[dict[str, Any]] = []
    unknown = sorted(references - set(records))
    if unknown:
        issues.append({"reason": "report references unknown visual evidence ids", "unknown": unknown})
    if required_kinds:
        referenced_kinds = {records[item].get("kind") for item in references if item in records}
        missing = sorted(required_kinds - referenced_kinds)
        if missing:
            issues.append({"reason": "report does not bind required visual evidence kinds", "missing_kinds": missing})
    return issues
