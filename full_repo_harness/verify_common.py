"""Reusable verifier helpers for full-repository audit skills."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path


SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SECTION_RE = re.compile(r"^##\s+(.+?)\s*$")

# Canonical interaction/metadata checklist labels shared by every UI-bearing
# audit skill. This is the single source of truth so the three verifiers cannot
# drift, and so the "hard reporting gate" in the SKILL.md contracts is enforced
# by code rather than prose.
INTERACTION_CHECKLIST_LABELS = (
    "badge-detail",
    "row-hit-target",
    "navigation-cursor",
    "transient-disclosure",
    "disclosure-scrollbar",
    "icon-meaning",
    "stable-expansion-width",
    "hover-copy",
    "status-summary",
    "message-metadata",
)
_CHECKLIST_STATUS_RE = r"(?:pass|passed|gap|blocked|not[\s\-]?applicable|n/?a)"


def interaction_checklist_missing(text: str) -> list[str]:
    """Return the checklist labels that are absent or not marked with a status.

    A label counts as marked when it is followed by ``=``, ``:`` or a table-cell
    ``|`` separator and a ``pass``/``gap``/``blocked``/``not-applicable`` status
    token. That matches both the inline ``badge-detail=pass; ...`` form and the
    table-cell form that the batch prompts instruct workers to emit.
    """
    missing: list[str] = []
    for label in INTERACTION_CHECKLIST_LABELS:
        pattern = re.compile(
            re.escape(label) + r"\s*(?:[=:]|\|)\s*" + _CHECKLIST_STATUS_RE,
            re.IGNORECASE,
        )
        if not pattern.search(text):
            missing.append(label)
    return missing


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def iter_report_files(paths: list[str]) -> list[Path]:
    reports: list[Path] = []
    for raw in paths:
        path = Path(raw)
        if path.is_dir():
            reports.extend(sorted(child for child in path.glob("*.md") if child.is_file()))
        elif path.is_file():
            reports.append(path)
    return reports


def duplicate_values(values: list[str]) -> list[str]:
    seen: set[str] = set()
    duplicates: set[str] = set()
    for value in values:
        if value in seen:
            duplicates.add(value)
        seen.add(value)
    return sorted(duplicates)


def load_json_object(path: Path, name: str) -> dict:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"{name} is not valid JSON: {path}") from exc
    if not isinstance(payload, dict):
        raise ValueError(f"{name} must be a JSON object.")
    return payload


def load_json_list(path: Path, name: str) -> list:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"{name} is not valid JSON: {path}") from exc
    if not isinstance(payload, list):
        raise ValueError(f"{name} must be a JSON list.")
    return payload


def canonical_json_sha256(data: dict | list) -> str:
    payload = json.dumps(data, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def section_bodies(text: str) -> dict[str, str]:
    bodies: dict[str, list[str]] = {}
    current: str | None = None
    for line in text.splitlines():
        match = SECTION_RE.match(line.strip())
        if match:
            current = match.group(1).strip().lower()
            bodies.setdefault(current, [])
            continue
        if current is not None:
            bodies[current].append(line)
    return {key: "\n".join(lines).strip() for key, lines in bodies.items()}


def section_order(text: str) -> list[str]:
    return [match.group(1).strip().lower() for line in text.splitlines() if (match := SECTION_RE.match(line.strip()))]


def split_markdown_row(row: str) -> list[str]:
    stripped = row.strip()
    if not stripped.startswith("|") or not stripped.endswith("|"):
        return []
    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for char in stripped[1:-1]:
        if escaped:
            if char == "|":
                current.append("|")
            else:
                current.append("\\")
                current.append(char)
            escaped = False
            continue
        if char == "\\":
            escaped = True
            continue
        if char == "|":
            cells.append("".join(current).strip())
            current = []
            continue
        current.append(char)
    if escaped:
        current.append("\\")
    cells.append("".join(current).strip())
    return cells


def is_separator_row(columns: list[str]) -> bool:
    return bool(columns) and all(re.fullmatch(r":?-{3,}:?", column.strip()) for column in columns)


def parse_markdown_table_dicts(text: str) -> list[dict[str, str]]:
    rows = [split_markdown_row(line) for line in text.splitlines()]
    rows = [row for row in rows if row]
    if len(rows) < 2:
        return []
    parsed: list[dict[str, str]] = []
    index = 0
    while index < len(rows) - 1:
        header = rows[index]
        separator = rows[index + 1]
        if is_separator_row(separator):
            cursor = index + 2
            while cursor < len(rows) and len(rows[cursor]) == len(header) and not is_separator_row(rows[cursor]):
                parsed.append({header[col].lower(): rows[cursor][col] for col in range(len(header))})
                cursor += 1
            index = cursor
        else:
            index += 1
    return parsed


def declared_values(text: str, heading: str) -> list[str]:
    bodies = section_bodies(text)
    body = bodies.get(heading.lower(), "")
    return [line.strip().strip("`") for line in body.splitlines() if line.strip()]
