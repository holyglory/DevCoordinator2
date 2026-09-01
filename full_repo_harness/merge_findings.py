#!/usr/bin/env python3
"""Merge findings across many audit batch/worker reports into one ranked list.

The audit skills dispatch one worker per deterministic file batch, so a large
repository produces dozens or hundreds of report files. The lead agent is then
expected to deduplicate and rank findings across all of them by hand, which does
not scale and is the step most likely to drop or double-count issues.

This tool does the mechanical part: it reads the exact manifest-authorized
reports (or every ``*.md`` report only when no manifest is supplied), extracts
findings from any ``## ...Findings`` / ``## ...Gap`` section
(handling both the ``### P1 - title`` heading form used by full-repo-audit and
the ``- Priority: P1`` field-block form used by the UI and test-coverage
skills), deduplicates findings only when all immutable finding content matches,
ranks them P0 -> P3, and writes a
consolidated JSON + Markdown digest. Conservative under-deduplication is
intentional: the tool never invents severity or lets a truncated key collapse
distinct obligations, and it preserves source report names for lead review.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import unicodedata
from pathlib import Path


SECTION_RE = re.compile(r"^##\s+(.+?)\s*$")
# A findings section is any H2 whose title mentions "finding" or "gap"
# (e.g. "Findings", "Implementation Gap Findings", "Coverage Findings",
# "File-Level Findings"). "No gap notes" is intentionally excluded below.
FINDINGS_SECTION_RE = re.compile(r"\b(finding|gap)s?\b", re.IGNORECASE)
HEADING_FINDING_RE = re.compile(r"^###\s+(P[0-3])\s*[-–—:]\s*(.+?)\s*$", re.IGNORECASE)
PRIORITY_FIELD_RE = re.compile(r"^-\s*Priority\s*:\s*(P[0-3])\b", re.IGNORECASE)
FIELD_RE = re.compile(r"^-\s*([^:]+?)\s*:\s*(.*)$")
PATH_IN_BACKTICKS_RE = re.compile(r"`([^`]+)`")
PRIORITY_ORDER = {"P0": 0, "P1": 1, "P2": 2, "P3": 3}
NO_FINDINGS_SENTINELS = {"no findings.", "no findings", "none.", "none"}
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
VERIFICATION_RECEIPT_KEYS = {
    "schema_version",
    "audit_kind",
    "run_id",
    "repo_root",
    "manifest_sha256",
    "reports_dir",
    "report_sha256",
    "verifier_result_sha256",
}


def section_blocks(text: str) -> dict[str, str]:
    blocks: dict[str, list[str]] = {}
    current: str | None = None
    for line in text.splitlines():
        match = SECTION_RE.match(line.strip())
        if match:
            current = match.group(1).strip()
            blocks.setdefault(current, [])
            continue
        if current is not None:
            blocks[current].append(line)
    return {title: "\n".join(lines).strip() for title, lines in blocks.items()}


def is_findings_section(title: str) -> bool:
    lowered = title.lower()
    if "no gap" in lowered or "no finding" in lowered:
        return False
    return bool(FINDINGS_SECTION_RE.search(lowered))


def split_files(value: str) -> list[str]:
    refs = PATH_IN_BACKTICKS_RE.findall(value)
    if refs:
        candidates = refs
    else:
        candidates = re.split(r"[,;]", value)
    out: list[str] = []
    for candidate in candidates:
        cleaned = candidate.strip().strip("`").strip()
        if cleaned and cleaned.lower() not in {"none", "n/a", "not applicable"}:
            out.append(cleaned)
    return out


def parse_findings_from_section(body: str) -> list[dict]:
    """Parse both the heading form and the field-block form from one section."""
    lines = body.splitlines()
    findings: list[dict] = []
    current: dict | None = None

    def flush() -> None:
        nonlocal current
        if current is not None:
            findings.append(current)
            current = None

    for raw in lines:
        line = raw.rstrip()
        heading = HEADING_FINDING_RE.match(line.strip())
        if heading:
            flush()
            current = {"priority": heading.group(1).upper(), "summary": heading.group(2).strip(), "fields": {}}
            continue
        priority_field = PRIORITY_FIELD_RE.match(line.strip())
        if priority_field:
            # New field-block finding begins at each "- Priority:" line.
            flush()
            current = {"priority": priority_field.group(1).upper(), "summary": "", "fields": {}}
            continue
        field = FIELD_RE.match(line.strip())
        if field and current is not None:
            key = field.group(1).strip().lower()
            value = field.group(2).strip()
            current["fields"][key] = value
    flush()

    for finding in findings:
        fields = finding["fields"]
        if not finding["summary"]:
            finding["summary"] = (
                fields.get("summary")
                or fields.get("gap")
                or fields.get("evidence")
                or fields.get("missing scenarios/boundaries")
                or "(no summary)"
            )
        files = []
        for key in ("files", "docs/files", "file"):
            if fields.get(key):
                files = split_files(fields[key])
                break
        finding["files"] = files
        finding["evidence"] = fields.get("evidence") or fields.get("interface evidence") or ""
        finding["expected_behavior"] = fields.get("expected behavior/standard") or ""
        finding["gap"] = fields.get("gap") or ""
        finding["suggested_direction"] = (
            fields.get("suggested direction")
            or fields.get("suggested implementation direction")
            or ""
        )
    return findings


def normalize_summary(summary: str) -> str:
    # Finding identity is intentionally lossless: punctuation, operators, and
    # case can change code meaning. NFC only canonicalizes equivalent Unicode
    # encodings; the parser has already removed surrounding field whitespace.
    return unicodedata.normalize("NFC", summary)


def dedupe_key(finding: dict) -> tuple:
    return (
        tuple(finding["files"]),
        normalize_summary(finding["summary"]),
        normalize_summary(finding.get("evidence", "")),
        normalize_summary(finding.get("expected_behavior", "")),
        normalize_summary(finding.get("gap", "")),
        normalize_summary(finding.get("suggested_direction", "")),
    )


def candidate_id(finding: dict) -> str:
    identity = "\0".join(
        [
            "\0".join(finding["files"]),
            normalize_summary(finding["summary"]),
            normalize_summary(finding.get("evidence", "")),
            normalize_summary(finding.get("expected_behavior", "")),
            normalize_summary(finding.get("gap", "")),
            normalize_summary(finding.get("suggested_direction", "")),
        ]
    )
    return "FRA-C-" + hashlib.sha256(identity.encode("utf-8")).hexdigest()[:12].upper()


def manifest_report_names(manifest: dict) -> list[str]:
    """Return exact full-repo-audit report basenames authorized by a manifest."""
    names: list[str] = []
    for batch in manifest.get("batches", []):
        if not isinstance(batch, dict) or not isinstance(batch.get("id"), str):
            raise ValueError("manifest batches must contain string ids")
        names.append(f"{batch['id']}.md")

    journey = manifest.get("journey_audit", {})
    if isinstance(journey, dict) and journey.get("required") is True:
        for field in ("source_report", "visual_report"):
            report = journey.get(field)
            if not isinstance(report, str):
                raise ValueError(f"manifest journey_audit.{field} must be a string")
            path = Path(report)
            if path.parent != Path("reports") or path.name != report.removeprefix("reports/"):
                raise ValueError(f"manifest journey_audit.{field} must be a direct reports/ child")
            names.append(path.name)

    lead = manifest.get("lead_reconciliation")
    if not isinstance(lead, dict) or lead.get("required") is not True:
        raise ValueError(
            "manifest mode is full-repo-audit-only and requires lead_reconciliation; "
            "other audit skills must omit --manifest"
        )
    lead_report = lead.get("report")
    if not isinstance(lead_report, str):
        raise ValueError("manifest lead_reconciliation.report must be a string")
    lead_path = Path(lead_report)
    if lead_path.parent != Path("reports") or lead_path.name != lead_report.removeprefix("reports/"):
        raise ValueError("manifest lead_reconciliation.report must be a direct reports/ child")
    names.append(lead_path.name)

    if len(names) != len(set(names)):
        raise ValueError("manifest report allowlist contains duplicates")
    return names


def merge_findings(reports_dir: Path, *, report_names: list[str] | None = None) -> dict:
    all_markdown = sorted(path for path in reports_dir.glob("*.md") if path.is_file())
    if report_names is None:
        report_paths = all_markdown
        ignored_reports: list[str] = []
    else:
        if len(report_names) != len(set(report_names)):
            raise ValueError("report allowlist contains duplicates")
        invalid = [name for name in report_names if Path(name).name != name or not name.endswith(".md")]
        if invalid:
            raise ValueError(f"report allowlist must contain direct Markdown basenames: {invalid}")
        report_paths = [reports_dir / name for name in report_names]
        missing = [path.name for path in report_paths if not path.is_file() or path.is_symlink()]
        if missing:
            raise ValueError(f"required verified reports are missing or symlinked: {missing}")
        allowed = set(report_names)
        ignored_reports = [path.name for path in all_markdown if path.name not in allowed]

    merged: dict[tuple, dict] = {}
    total_raw = 0
    report_hashes: dict[str, str] = {}
    for report_path in report_paths:
        try:
            report_bytes = report_path.read_bytes()
            text = report_bytes.decode("utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            raise ValueError(f"could not read report {report_path.name}: {exc}") from exc
        report_hashes[report_path.name] = hashlib.sha256(report_bytes).hexdigest()
        for title, body in section_blocks(text).items():
            if not is_findings_section(title):
                continue
            if body.strip().lower() in NO_FINDINGS_SENTINELS:
                continue
            for finding in parse_findings_from_section(body):
                total_raw += 1
                key = dedupe_key(finding)
                if key in merged:
                    entry = merged[key]
                    entry["sources"].append(report_path.name)
                    # Keep the most severe priority seen for a duplicated finding.
                    if PRIORITY_ORDER.get(finding["priority"], 9) < PRIORITY_ORDER.get(entry["priority"], 9):
                        entry["priority"] = finding["priority"]
                else:
                    merged[key] = {
                        "priority": finding["priority"],
                        "summary": finding["summary"],
                        "files": list(finding["files"]),
                        "evidence": finding["evidence"],
                        "expected_behavior": finding["expected_behavior"],
                        "gap": finding["gap"],
                        "suggested_direction": finding["suggested_direction"],
                        "sources": [report_path.name],
                    }
    consolidated = sorted(
        merged.values(),
        key=lambda item: (PRIORITY_ORDER.get(item["priority"], 9), item["files"][0] if item["files"] else "", item["summary"]),
    )
    counts = {priority: 0 for priority in ("P0", "P1", "P2", "P3")}
    for item in consolidated:
        item["candidate_id"] = candidate_id(item)
        counts[item["priority"]] = counts.get(item["priority"], 0) + 1
    return {
        "reports_scanned": len(report_paths),
        "report_sha256": report_hashes,
        "ignored_unverified_reports": ignored_reports,
        "raw_findings": total_raw,
        "unique_findings": len(consolidated),
        "priority_counts": counts,
        "findings": consolidated,
    }


def canonical_json_sha256(value: object) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def render_completion_ledger_projection(
    result: dict,
    *,
    run_id: str,
    repo_root: str,
    manifest_sha256: str,
) -> dict:
    candidates = []
    for finding in result["findings"]:
        ledger_suffix = finding["candidate_id"].removeprefix("FRA-C-")
        candidates.append(
            {
                "candidate_id": finding["candidate_id"],
                "priority": finding["priority"],
                "summary": finding["summary"],
                "files": finding["files"],
                "evidence": finding.get("evidence", ""),
                "expected_behavior": finding.get("expected_behavior", ""),
                "gap": finding.get("gap", ""),
                "suggested_direction": finding.get("suggested_direction", ""),
                "source_reports": sorted(set(finding["sources"])),
                "disposition": "pending",
                "disposition_reason": "",
                "ledger_row": {
                    "id": f"FRA-{ledger_suffix}",
                    "remaining_work": "",
                    "why_it_matters": "",
                    "status": "Open",
                    "verification": "",
                },
            }
        )
    return {
        "schema_version": 1,
        "run_id": run_id,
        "repo_root": repo_root,
        "manifest_sha256": manifest_sha256,
        "consolidated_findings_sha256": canonical_json_sha256(result),
        "review_status": "pending",
        "review_instructions": (
            "Review every candidate. Set disposition to confirmed, duplicate, hypothesis, invalid, or out_of_scope; "
            "complete active ledger fields only for confirmed candidates and explain every excluded or duplicate disposition. "
            "Candidates must already be atomic; if one is compound, reissue atomic findings in an authorized report, "
            "rerun verification and consolidation, and review the regenerated projection."
        ),
        "candidates": candidates,
    }


def render_markdown(result: dict) -> str:
    lines = ["# Consolidated Audit Findings", ""]
    counts = result["priority_counts"]
    lines.append(
        f"{result['unique_findings']} unique findings from {result['raw_findings']} raw findings "
        f"across {result['reports_scanned']} reports "
        f"(P0 {counts.get('P0', 0)}, P1 {counts.get('P1', 0)}, P2 {counts.get('P2', 0)}, P3 {counts.get('P3', 0)})."
    )
    lines.append("")
    current_priority: str | None = None
    for item in result["findings"]:
        if item["priority"] != current_priority:
            current_priority = item["priority"]
            lines.append(f"## {current_priority}")
            lines.append("")
        files = ", ".join(f"`{path}`" for path in item["files"]) or "_no file cited_"
        sources = ", ".join(sorted(set(item["sources"])))
        lines.append(f"- **{item['summary']}**")
        lines.append(f"  - Files: {files}")
        if item["evidence"]:
            lines.append(f"  - Evidence: {item['evidence']}")
        lines.append(f"  - Reported by: {sources}")
        lines.append("")
    if not result["findings"]:
        lines.append("_No findings reported across the scanned reports._")
        lines.append("")
    return "\n".join(lines)


def strict_json_object(data: bytes, label: str) -> dict:
    def object_pairs(pairs: list[tuple[str, object]]) -> dict:
        value: dict[str, object] = {}
        for key, item in pairs:
            if key in value:
                raise ValueError(f"{label} contains duplicate JSON key {key!r}")
            value[key] = item
        return value

    def reject_constant(value: str) -> None:
        raise ValueError(f"{label} contains non-finite JSON number {value!r}")

    try:
        parsed = json.loads(data.decode("utf-8"), object_pairs_hook=object_pairs, parse_constant=reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"{label} is not valid UTF-8 JSON: {exc}") from exc
    if not isinstance(parsed, dict):
        raise ValueError(f"{label} must be a JSON object")
    return parsed


def read_stable_regular_file(path: Path, label: str) -> tuple[bytes, tuple[int, ...]]:
    try:
        before = os.lstat(path)
    except OSError as exc:
        raise ValueError(f"could not inspect {label} {path}: {exc}") from exc
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        raise ValueError(f"{label} must be a non-symlinked regular file: {path}")
    try:
        with path.open("rb") as handle:
            opened = os.fstat(handle.fileno())
            data = handle.read()
            finished = os.fstat(handle.fileno())
        after = os.lstat(path)
    except OSError as exc:
        raise ValueError(f"could not read {label} {path}: {exc}") from exc

    def identity(value: os.stat_result) -> tuple[int, ...]:
        return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)

    expected = identity(before)
    if expected != identity(opened) or expected != identity(finished) or expected != identity(after):
        raise ValueError(f"{label} changed while it was read: {path}")
    return data, expected


def validate_verification_receipt(
    receipt: dict,
    *,
    manifest: dict,
    manifest_sha256: str,
    reports_dir: Path,
    report_names: list[str],
) -> dict[str, str]:
    if set(receipt) != VERIFICATION_RECEIPT_KEYS:
        raise ValueError(
            "verification receipt keys must be exact; "
            f"missing={sorted(VERIFICATION_RECEIPT_KEYS - set(receipt))}, "
            f"extra={sorted(set(receipt) - VERIFICATION_RECEIPT_KEYS)}"
        )
    expected_scalars = {
        "schema_version": 1,
        "audit_kind": "full-repo-audit",
        "run_id": manifest.get("run_id"),
        "repo_root": manifest.get("repo_root"),
        "manifest_sha256": manifest_sha256,
        "reports_dir": str(reports_dir),
    }
    for field, expected in expected_scalars.items():
        if receipt.get(field) != expected:
            raise ValueError(f"verification receipt {field} does not match the audit manifest/report root")
    verifier_hash = receipt.get("verifier_result_sha256")
    if not isinstance(verifier_hash, str) or not SHA256_RE.fullmatch(verifier_hash):
        raise ValueError("verification receipt verifier_result_sha256 must be a lowercase SHA-256 digest")
    report_hashes = receipt.get("report_sha256")
    if not isinstance(report_hashes, dict) or set(report_hashes) != set(report_names):
        raise ValueError("verification receipt report_sha256 must bind every authorized report exactly once")
    if not all(isinstance(value, str) and SHA256_RE.fullmatch(value) for value in report_hashes.values()):
        raise ValueError("verification receipt report hashes must be lowercase SHA-256 digests")
    return report_hashes


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Merge and rank findings across audit reports.")
    parser.add_argument("--reports", required=True, help="Directory containing batch/worker *.md reports.")
    parser.add_argument("--json-out", help="Write the consolidated findings JSON here.")
    parser.add_argument("--markdown-out", help="Write the consolidated findings Markdown here.")
    parser.add_argument(
        "--manifest",
        help="Full-repo-audit manifest used with its verifier receipt; unsupported for other audit schemas.",
    )
    parser.add_argument(
        "--verification-receipt",
        help="Passing full-repo-audit verifier receipt; defaults to <manifest-dir>/verification_receipt.json.",
    )
    parser.add_argument(
        "--ledger-projection-out",
        help="Write a lead-reviewable completion-ledger projection here; requires --manifest.",
    )
    parser.add_argument("--json", action="store_true", help="Print consolidated JSON to stdout.")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    reports_input = Path(args.reports).expanduser()
    if reports_input.is_symlink():
        print(f"reports directory must not be a symlink: {reports_input}", file=sys.stderr)
        return 2
    reports_dir = reports_input.resolve()
    if not reports_dir.is_dir():
        print(f"reports directory not found: {reports_dir}", file=sys.stderr)
        return 2
    manifest = None
    manifest_path = None
    manifest_bytes = None
    manifest_sha256 = None
    receipt_path = None
    receipt_bytes = None
    report_names = None
    receipt_report_hashes = None
    if args.manifest:
        manifest_path = Path(args.manifest).expanduser().resolve()
        try:
            manifest_bytes, _ = read_stable_regular_file(manifest_path, "audit manifest")
            manifest = strict_json_object(manifest_bytes, "audit manifest")
            manifest_sha256 = hashlib.sha256(manifest_bytes).hexdigest()
            report_names = manifest_report_names(manifest)
            declared_reports = manifest.get("reports_dir")
            expected_reports = manifest_path.parent / "reports"
            if expected_reports.is_symlink():
                raise ValueError(f"manifest-owned reports directory must not be a symlink: {expected_reports}")
            if (
                not isinstance(declared_reports, str)
                or Path(declared_reports).expanduser().resolve() != reports_dir
                or expected_reports.resolve() != reports_dir
            ):
                raise ValueError("--reports must be the exact non-symlinked reports directory owned by the manifest")
            receipt_path = (
                Path(args.verification_receipt).expanduser().resolve()
                if args.verification_receipt
                else manifest_path.parent / "verification_receipt.json"
            )
            receipt_bytes, _ = read_stable_regular_file(receipt_path, "verification receipt")
            receipt = strict_json_object(receipt_bytes, "verification receipt")
            receipt_report_hashes = validate_verification_receipt(
                receipt,
                manifest=manifest,
                manifest_sha256=manifest_sha256,
                reports_dir=reports_dir,
                report_names=report_names,
            )
        except (OSError, ValueError) as exc:
            print(f"could not read audit manifest: {exc}", file=sys.stderr)
            return 2
    elif args.verification_receipt:
        print("--verification-receipt requires --manifest", file=sys.stderr)
        return 2
    try:
        result = merge_findings(
            reports_dir,
            report_names=report_names,
        )
    except (OSError, ValueError) as exc:
        print(f"could not merge audit reports: {exc}", file=sys.stderr)
        return 2

    if receipt_report_hashes is not None and result.get("report_sha256") != receipt_report_hashes:
        print("verified audit reports changed after receipt creation; rerun the verifier", file=sys.stderr)
        return 2

    def inputs_still_stable() -> bool:
        if manifest_path is None or manifest_bytes is None or receipt_path is None or receipt_bytes is None:
            return True
        try:
            current_manifest, _ = read_stable_regular_file(manifest_path, "audit manifest")
            current_receipt, _ = read_stable_regular_file(receipt_path, "verification receipt")
        except ValueError as exc:
            print(f"verified audit input changed before publication: {exc}", file=sys.stderr)
            return False
        if current_manifest != manifest_bytes or current_receipt != receipt_bytes:
            print("audit manifest or verification receipt changed before publication", file=sys.stderr)
            return False
        return True

    projection = None
    if args.ledger_projection_out:
        if not args.manifest:
            print("--ledger-projection-out requires --manifest", file=sys.stderr)
            return 2
        assert manifest is not None and manifest_sha256 is not None
        run_id = manifest.get("run_id")
        repo_root = manifest.get("repo_root")
        if not isinstance(run_id, str) or not run_id or not isinstance(repo_root, str) or not repo_root:
            print("audit manifest must contain non-empty run_id and repo_root", file=sys.stderr)
            return 2
        projection = render_completion_ledger_projection(
            result,
            run_id=run_id,
            repo_root=repo_root,
            manifest_sha256=manifest_sha256,
        )

    if not inputs_still_stable():
        return 2
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if args.markdown_out:
        Path(args.markdown_out).write_text(render_markdown(result) + "\n", encoding="utf-8")
    if args.ledger_projection_out:
        assert projection is not None
        Path(args.ledger_projection_out).write_text(
            json.dumps(projection, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        counts = result["priority_counts"]
        print(
            f"{result['unique_findings']} unique findings from {result['raw_findings']} raw "
            f"across {result['reports_scanned']} reports "
            f"(P0 {counts.get('P0', 0)}, P1 {counts.get('P1', 0)}, P2 {counts.get('P2', 0)}, P3 {counts.get('P3', 0)})"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
