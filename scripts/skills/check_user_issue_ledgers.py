#!/usr/bin/env python3
"""Validate scoped project-root user issue prevention ledgers."""

from __future__ import annotations

import argparse
import os
import re
import stat
import sys
from pathlib import Path
from typing import NamedTuple


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from full_repo_harness import completion_ledger as secure_reader  # noqa: E402


DEFAULT_ROOT = ROOT / "UserIssueLedgers"
TITLE_PATTERN = re.compile(r"# User Issue Ledger: (?P<scope>\S(?:.*\S)?)")
PATH_NAME_PATTERN = re.compile(r"[A-Z][A-Za-z0-9-]*")
ID_PATTERN = re.compile(r"UIL-[A-Z0-9]+(?:-[A-Z0-9]+)*-[0-9]{3,}")
CAMEL_TOKEN_PATTERN = re.compile(r"[A-Z]+(?=[A-Z][a-z]|[0-9]|$)|[A-Z]?[a-z]+|[0-9]+")
BROAD_LEAF_SCOPES = {
    "all",
    "business",
    "businesslogic",
    "everything",
    "general",
    "issues",
    "misc",
    "miscellaneous",
    "shared",
    "userissues",
}
HEADER = (
    "ID",
    "Applies to",
    "Mistake pattern",
    "Required behavior",
    "Prevention and verification",
)


class IssueRow(NamedTuple):
    issue_id: str
    applies_to: str
    mistake_pattern: str
    required_behavior: str
    prevention_and_verification: str


def path_scope(relative: Path) -> tuple[tuple[str, ...], str]:
    """Return the title-token path and exact ID namespace owned by a ledger path."""

    components = (*relative.parent.parts, relative.stem)
    token_components: list[tuple[str, ...]] = []
    id_tokens: list[str] = []
    for component in components:
        tokens = tuple(token.casefold() for token in CAMEL_TOKEN_PATTERN.findall(component))
        if not tokens:
            raise ValueError("ledger path does not define a scope")
        token_components.append(tokens)
        id_tokens.extend(token.upper() for token in tokens)
    return tuple(token_components), "UIL-" + "-".join(id_tokens) + "-"


def title_scope(scope: str) -> tuple[tuple[str, ...], ...]:
    components: list[tuple[str, ...]] = []
    for component in scope.split("/"):
        words = tuple(word.casefold() for word in re.findall(r"[A-Za-z0-9]+", component))
        if not words:
            return ()
        components.append(words)
    return tuple(components)


def parse_row(line: str, line_number: int) -> tuple[str, ...]:
    if not line.startswith("|") or not line.endswith("|"):
        raise ValueError(f"line {line_number} must be a pipe-delimited table row")
    cells: list[str] = []
    cell: list[str] = []
    escaped = False
    for character in line[1:-1]:
        if escaped:
            cell.append(character)
            escaped = False
        elif character == "\\":
            escaped = True
        elif character == "|":
            cells.append("".join(cell).strip())
            cell = []
        else:
            cell.append(character)
    if escaped:
        cell.append("\\")
    cells.append("".join(cell).strip())
    parsed = tuple(cells)
    if len(parsed) != len(HEADER):
        raise ValueError(
            f"line {line_number} must contain exactly {len(HEADER)} cells; "
            "escape literal pipes inside cells"
        )
    return parsed


def inspect_ledger(text: str) -> tuple[list[str], list[IssueRow], str | None]:
    violations: list[str] = []
    rows: list[IssueRow] = []
    lines = text.splitlines()

    title = TITLE_PATTERN.fullmatch(lines[0]) if lines else None
    if not title:
        violations.append("line 1 must match '# User Issue Ledger: <scope>'")
        return violations, rows, None
    scope = title.group("scope")
    if len(lines) < 2 or lines[1] != "":
        violations.append("the title must be followed by one blank line")
    if len(lines) < 4:
        violations.append("ledger must contain the canonical five-column table")
        return violations, rows, scope

    try:
        header = parse_row(lines[2], 3)
    except ValueError as exc:
        violations.append(str(exc))
        return violations, rows, scope
    if header != HEADER:
        violations.append("table header must be exactly: " + " | ".join(HEADER))

    try:
        separator = parse_row(lines[3], 4)
    except ValueError as exc:
        violations.append(str(exc))
        return violations, rows, scope
    if separator != tuple("---" for _ in HEADER):
        violations.append("table separator must contain exactly five '---' cells")

    data_lines = lines[4:]
    if not data_lines:
        violations.append("an existing scoped ledger must contain at least one issue row")
        return violations, rows, scope

    seen_ids: set[str] = set()
    seen_patterns: set[tuple[str, str]] = set()
    for line_number, line in enumerate(data_lines, start=5):
        if not line:
            violations.append(
                f"line {line_number} is blank; prose and spacing are not allowed after the table header"
            )
            continue
        try:
            cells = parse_row(line, line_number)
        except ValueError as exc:
            violations.append(str(exc))
            continue
        issue_id, applies_to, mistake, required, guardrail = cells
        if not ID_PATTERN.fullmatch(issue_id):
            violations.append(f"line {line_number} ID must match UIL-<SCOPE>-NNN")
        elif issue_id in seen_ids:
            violations.append(f"line {line_number} duplicates ID {issue_id}")
        else:
            seen_ids.add(issue_id)

        for column, value in zip(HEADER[1:], (applies_to, mistake, required, guardrail)):
            if not value:
                violations.append(f"line {line_number} has an empty {column!r} cell")
            if "<br" in value.casefold():
                violations.append(f"line {line_number} must keep {column!r} to one compact table cell")

        normalized_pattern = (
            " ".join(applies_to.casefold().split()).rstrip(".!?"),
            " ".join(mistake.casefold().split()).rstrip(".!?"),
        )
        if all(normalized_pattern):
            if normalized_pattern in seen_patterns:
                violations.append(
                    f"line {line_number} duplicates an existing applicability and mistake pattern; merge the rows"
                )
            else:
                seen_patterns.add(normalized_pattern)
        rows.append(IssueRow(issue_id, applies_to, mistake, required, guardrail))

    return violations, rows, scope


def find_ledger_violations(text: str) -> list[str]:
    return inspect_ledger(text)[0]


def _project_anchor(path: Path) -> Path | None:
    absolute = Path.cwd() / path if not path.is_absolute() else path
    try:
        Path(os.path.abspath(absolute)).relative_to(Path(os.path.abspath(ROOT)))
    except ValueError:
        return None
    return ROOT


def _probe_root(root: Path) -> str | None:
    try:
        secure_reader.read_text_nofollow(
            root / ".user-issue-ledger-probe", anchor=_project_anchor(root)
        )
    except (OSError, secure_reader.LedgerError) as exc:
        return str(exc)
    return None


def scan_ledgers(root: Path) -> tuple[list[str], int]:
    violations: list[str] = []
    probe_error = _probe_root(root)
    if probe_error:
        return [probe_error], 0
    try:
        root_metadata = root.lstat()
    except FileNotFoundError:
        return [], 0
    except OSError as exc:
        return [f"cannot inspect ledger root: {exc}"], 0
    if not stat.S_ISDIR(root_metadata.st_mode):
        return ["UserIssueLedgers must be a regular project-owned directory"], 0

    seen_ids: dict[str, str] = {}
    seen_patterns: dict[tuple[str, str], str] = {}
    seen_scopes: dict[str, str] = {}
    ledger_count = 0

    def record_walk_error(error: OSError) -> None:
        try:
            path = Path(error.filename) if error.filename else root
            relative = path.relative_to(root).as_posix()
        except (TypeError, ValueError):
            relative = "<ledger tree>"
        violations.append(f"{relative}: cannot traverse scoped-ledger directory: {error}")

    for current_raw, directory_names, file_names in os.walk(
        root,
        topdown=True,
        onerror=record_walk_error,
        followlinks=False,
    ):
        current = Path(current_raw)
        safe_directories: list[str] = []
        for name in sorted(directory_names):
            path = current / name
            relative = path.relative_to(root).as_posix()
            try:
                metadata = path.lstat()
            except OSError as exc:
                violations.append(f"{relative}: cannot inspect directory: {exc}")
                continue
            if stat.S_ISLNK(metadata.st_mode):
                violations.append(f"{relative}: scoped-ledger directories must not be symlinks")
                continue
            if not stat.S_ISDIR(metadata.st_mode):
                violations.append(f"{relative}: expected a directory")
                continue
            if not PATH_NAME_PATTERN.fullmatch(name):
                violations.append(f"{relative}: directory names must be concise UpperCamelCase slugs")
                continue
            safe_directories.append(name)
        directory_names[:] = safe_directories

        for name in sorted(file_names):
            path = current / name
            relative = path.relative_to(root).as_posix()
            try:
                metadata = path.lstat()
            except OSError as exc:
                violations.append(f"{relative}: cannot inspect file: {exc}")
                continue
            if stat.S_ISLNK(metadata.st_mode):
                violations.append(f"{relative}: scoped ledger files must not be symlinks")
                continue
            if not stat.S_ISREG(metadata.st_mode):
                violations.append(f"{relative}: scoped ledger entries must be regular files")
                continue
            if path.suffix != ".md" or not PATH_NAME_PATTERN.fullmatch(path.stem):
                violations.append(f"{relative}: ledger filenames must be UpperCamelCase .md slugs")
                continue
            if path.stem.casefold() in BROAD_LEAF_SCOPES:
                violations.append(
                    f"{relative}: ledger scope is too broad; use a concrete surface or bounded "
                    "business-logic perspective"
                )
            try:
                text = secure_reader.read_text_nofollow(
                    path, anchor=_project_anchor(root)
                )
            except (OSError, secure_reader.LedgerError) as exc:
                violations.append(f"{relative}: {exc}")
                continue
            if text is None:
                violations.append(f"{relative}: ledger disappeared while being read")
                continue
            file_violations, rows, scope = inspect_ledger(text)
            violations.extend(f"{relative}: {message}" for message in file_violations)
            ledger_count += 1
            if scope:
                expected_title_scope, expected_id_prefix = path_scope(path.relative_to(root))
                if title_scope(scope) != expected_title_scope:
                    violations.append(
                        f"{relative}: title scope must match its relative path; nested path components "
                        "are separated with ' / '"
                    )
                normalized_scope = " ".join(scope.casefold().split())
                if normalized_scope in seen_scopes:
                    violations.append(
                        f"{relative}: duplicates scope title from {seen_scopes[normalized_scope]}"
                    )
                else:
                    seen_scopes[normalized_scope] = relative
            for row in rows:
                if scope and not row.issue_id.startswith(expected_id_prefix):
                    violations.append(
                        f"{relative}: ID {row.issue_id} must use path-owned namespace "
                        f"{expected_id_prefix}<NNN>"
                    )
                if row.issue_id in seen_ids:
                    violations.append(
                        f"{relative}: duplicates global ID {row.issue_id} from {seen_ids[row.issue_id]}"
                    )
                else:
                    seen_ids[row.issue_id] = relative
                normalized_pattern = (
                    " ".join(row.applies_to.casefold().split()).rstrip(".!?"),
                    " ".join(row.mistake_pattern.casefold().split()).rstrip(".!?"),
                )
                if normalized_pattern in seen_patterns:
                    violations.append(
                        f"{relative}: duplicates applicability and mistake pattern from "
                        f"{seen_patterns[normalized_pattern]}; keep one narrow owner"
                    )
                else:
                    seen_patterns[normalized_pattern] = relative

    final_probe_error = _probe_root(root)
    if final_probe_error:
        violations.append(final_probe_error)
    if ledger_count == 0:
        violations.append("an existing UserIssueLedgers directory must contain at least one scoped ledger")
    return violations, ledger_count


def audit_ledgers(root: Path) -> list[str]:
    return scan_ledgers(root)[0]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    args = parser.parse_args()

    violations, count = scan_ledgers(args.root)
    if violations:
        for violation in violations:
            print(f"user issue ledger violation: {violation}")
        return 1
    state = f"{count} scoped ledger(s)" if count else "absent"
    print(f"user issue ledger check ok ({state}: {args.root})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
