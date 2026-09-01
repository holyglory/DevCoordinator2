"""Deterministic structural test targets and optional empirical coverage ingestion."""

from __future__ import annotations

import hashlib
import json
import re
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any, Iterable

from .verify_common import sha256_file


FUNCTION_PATTERNS: dict[str, tuple[tuple[str, str], ...]] = {
    ".py": ((r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)\s*\(", "function"),),
    ".ts": (
        (r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(", "function"),
        (r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>", "function"),
    ),
    ".tsx": (
        (r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(", "component/function"),
        (r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>", "component/function"),
    ),
    ".js": (
        (r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(", "function"),
        (r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>", "function"),
    ),
    ".jsx": ((r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(", "component/function"),),
    ".swift": ((r"^\s*(?:(?:public|private|internal|fileprivate|open|static|class|mutating)\s+)*(?:func|init)\s*([A-Za-z_]\w*)?\s*\(", "method"),),
    ".rs": ((r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_]\w*)\s*\(", "function"),),
    ".go": ((r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)\s*\(", "function"),),
}
UI_CONTROL_RE = re.compile(r"<(button|a|input|select|textarea|form)\b[^>]*>([^<]{0,80})", re.IGNORECASE)


def _target_id(unit_id: str, kind: str, symbol: str, line: int) -> str:
    digest = hashlib.sha256(f"{unit_id}\0{kind}\0{symbol}\0{line}".encode("utf-8")).hexdigest()[:16]
    return f"target-{digest}"


def discover_targets(repo: Path, units: Iterable[Any]) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    text_cache: dict[str, list[str]] = {}
    for unit in units:
        rel_path = unit.rel_path if hasattr(unit, "rel_path") else unit["rel_path"]
        unit_id = unit.unit_id if hasattr(unit, "unit_id") else unit["unit_id"]
        start_line = unit.start_line if hasattr(unit, "start_line") else unit.get("start_line")
        end_line = unit.end_line if hasattr(unit, "end_line") else unit.get("end_line")
        interface_relevant = unit.interface_relevant if hasattr(unit, "interface_relevant") else unit.get("interface_relevant", False)
        if rel_path not in text_cache:
            try:
                text_cache[rel_path] = (repo / rel_path).read_text(encoding="utf-8").splitlines()
            except (OSError, UnicodeDecodeError):
                text_cache[rel_path] = []
        lines = text_cache[rel_path]
        suffix = Path(rel_path).suffix.lower()
        found: list[tuple[str, str, int]] = []
        if lines and not (hasattr(unit, "start_byte") and unit.start_byte is not None):
            first = start_line or 1
            last = end_line or len(lines)
            for line_number in range(first, min(last, len(lines)) + 1):
                line = lines[line_number - 1]
                for pattern, kind in FUNCTION_PATTERNS.get(suffix, ()):
                    match = re.search(pattern, line)
                    if match:
                        symbol = match.group(1) or "init"
                        found.append((symbol, kind, line_number))
                if interface_relevant:
                    match = UI_CONTROL_RE.search(line)
                    if match:
                        label = re.sub(r"\s+", " ", match.group(2)).strip() or match.group(1).lower()
                        found.append((f"{match.group(1).lower()}:{label}", "ui-control", line_number))
        deduplicated: set[tuple[str, str, int]] = set()
        for symbol, kind, line_number in found:
            key = (symbol, kind, line_number)
            if key in deduplicated:
                continue
            deduplicated.add(key)
            records.append(
                {
                    "target_id": _target_id(unit_id, kind, symbol, line_number),
                    "unit_id": unit_id,
                    "rel_path": rel_path,
                    "symbol": symbol,
                    "kind": kind,
                    "line": line_number,
                    "structural_basis": f"deterministic {suffix or 'file'} source scan",
                }
            )
        if not found:
            symbol = f"unit-review:{unit_id}"
            line_number = start_line or 1
            records.append(
                {
                    "target_id": _target_id(unit_id, "unit-review", symbol, line_number),
                    "unit_id": unit_id,
                    "rel_path": rel_path,
                    "symbol": symbol,
                    "kind": "unit-review",
                    "line": line_number,
                    "structural_basis": "no supported behavior symbol detected; explicit reviewed-target or not-reasonable decision required",
                }
            )
    return records


def _normalize_coverage_path(repo: Path, raw: str) -> str | None:
    candidate = Path(raw)
    if candidate.is_absolute():
        try:
            return candidate.resolve().relative_to(repo.resolve()).as_posix()
        except ValueError:
            return None
    normalized = Path(raw.lstrip("./"))
    if ".." in normalized.parts:
        return None
    if (repo / normalized).is_file():
        return normalized.as_posix()
    return None


def _add_line(index: dict[str, dict[str, set[int]]], rel_path: str | None, line: int, hits: int) -> None:
    if not rel_path or line < 1:
        return
    row = index.setdefault(rel_path, {"measured": set(), "covered": set()})
    row["measured"].add(line)
    if hits > 0:
        row["covered"].add(line)


def _parse_lcov(repo: Path, text: str) -> dict[str, dict[str, set[int]]]:
    index: dict[str, dict[str, set[int]]] = {}
    current: str | None = None
    for raw in text.splitlines():
        if raw.startswith("SF:"):
            current = _normalize_coverage_path(repo, raw[3:].strip())
        elif raw.startswith("DA:") and current:
            values = raw[3:].split(",")
            if len(values) >= 2 and values[0].isdigit() and values[1].lstrip("-").isdigit():
                _add_line(index, current, int(values[0]), int(values[1]))
        elif raw == "end_of_record":
            current = None
    return index


def _parse_xml(repo: Path, text: str) -> dict[str, dict[str, set[int]]]:
    index: dict[str, dict[str, set[int]]] = {}
    root = ET.fromstring(text)
    for class_node in root.findall(".//class"):
        rel_path = _normalize_coverage_path(repo, class_node.attrib.get("filename", ""))
        for line in class_node.findall(".//line"):
            try:
                _add_line(index, rel_path, int(line.attrib["number"]), int(float(line.attrib.get("hits", "0"))))
            except (KeyError, ValueError):
                continue
    return index


def _parse_json(repo: Path, payload: dict[str, Any]) -> tuple[str, dict[str, dict[str, set[int]]]]:
    index: dict[str, dict[str, set[int]]] = {}
    if isinstance(payload.get("files"), dict):
        for raw_path, row in payload["files"].items():
            if not isinstance(row, dict):
                continue
            rel_path = _normalize_coverage_path(repo, raw_path)
            executed = {int(value) for value in row.get("executed_lines", []) if isinstance(value, int)}
            missing = {int(value) for value in row.get("missing_lines", []) if isinstance(value, int)}
            for line in executed | missing:
                _add_line(index, rel_path, line, 1 if line in executed else 0)
        return "coverage.py-json", index
    for raw_path, row in payload.items():
        if not isinstance(row, dict) or not isinstance(row.get("statementMap"), dict) or not isinstance(row.get("s"), dict):
            continue
        rel_path = _normalize_coverage_path(repo, raw_path)
        for statement_id, location in row["statementMap"].items():
            try:
                line = int(location["start"]["line"])
                hits = int(row["s"].get(statement_id, 0))
            except (KeyError, TypeError, ValueError):
                continue
            _add_line(index, rel_path, line, hits)
    return "istanbul-json", index


def ingest_coverage_reports(repo: Path, raw_paths: Iterable[str]) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for raw_path in raw_paths:
        path = Path(raw_path)
        if not path.is_absolute():
            path = repo / path
        path = path.resolve(strict=True)
        data = path.read_bytes()
        text = data.decode("utf-8")
        suffix = path.suffix.lower()
        if suffix == ".info" or text.lstrip().startswith("TN:") or "\nSF:" in text:
            format_name = "lcov"
            index = _parse_lcov(repo, text)
        elif suffix == ".xml" or text.lstrip().startswith("<?xml") or text.lstrip().startswith("<coverage"):
            format_name = "cobertura-xml"
            index = _parse_xml(repo, text)
        else:
            payload = json.loads(text)
            if not isinstance(payload, dict):
                raise ValueError(f"coverage JSON must be an object: {path}")
            format_name, index = _parse_json(repo, payload)
        records.append(
            {
                "evidence_id": f"coverage-{hashlib.sha256(str(path).encode('utf-8')).hexdigest()[:12]}",
                "path": str(path),
                "sha256": sha256_file(path),
                "format": format_name,
                "files": {
                    rel_path: {
                        "measured_lines": sorted(values["measured"]),
                        "covered_lines": sorted(values["covered"]),
                    }
                    for rel_path, values in sorted(index.items())
                },
            }
        )
    return records
