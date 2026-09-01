"""Parse retired CompletionLedger.md snapshots for migration and legacy fixtures."""

from __future__ import annotations

import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path


TITLE = "# Completion Ledger"
HEADERS = ("ID", "Remaining work", "Why it matters", "Status", "Verification")
ACTIVE_STATUSES = {
    "active",
    "blocked",
    "in progress",
    "incomplete",
    "open",
    "partial",
    "pending",
    "to do",
    "todo",
    "unresolved",
    "waiting",
}
TERMINAL_STATUSES = {
    "closed",
    "complete",
    "completed",
    "done",
    "fixed",
    "implemented",
    "implemented and verified",
    "resolved",
    "verified",
}
FUTURE_STATUS_MARKERS = {"after", "before", "once", "unless", "until", "when"}
NEGATION_QUALIFIERS = {"actually", "completely", "currently", "fully", "properly", "successfully", "yet"}
FUTURE_STATUS_PHRASES = (
    ("must", "be"),
    ("need", "to", "be"),
    ("needs", "to", "be"),
    ("to", "be"),
    ("will", "be"),
)


class LedgerError(ValueError):
    """Raised when a retired Markdown snapshot violates its legacy schema."""


@dataclass(frozen=True)
class LedgerRow:
    id: str
    remaining_work: str
    why_it_matters: str
    status: str
    verification: str

    def as_dict(self) -> dict[str, str]:
        return {
            "id": self.id,
            "remaining_work": self.remaining_work,
            "why_it_matters": self.why_it_matters,
            "status": self.status,
            "verification": self.verification,
        }


def _status_key(value: str) -> str:
    plain = re.sub(r"[`*_~]", "", value).strip().casefold()
    contraction = r"\b(?:isn|wasn|hasn|hadn|doesn|didn|won|can|couldn|shouldn|wouldn)['’]t\b"
    plain = re.sub(contraction, " not ", plain)
    plain = re.sub(r"[\s_-]+", " ", plain)
    return plain.strip(" .:;()[]{}")


def _matching_prefix(value: str, choices: set[str]) -> tuple[str, re.Match[str]] | None:
    for choice in sorted(choices, key=len, reverse=True):
        match = re.match(rf"^{re.escape(choice)}(?:\b|$)", value)
        if match:
            return choice, match
    return None


def _terminal_match_is_pending(key: str, active_end: int, terminal_start: int) -> bool:
    """Return whether a terminal word describes work that is still pending.

    A status such as ``Open — not implemented`` or ``Blocked — until fixed``
    is active.  The terminal-looking word describes the missing result or the
    future unblock condition; it does not assert the row is complete.
    """

    context = key[active_end:terminal_start]
    trailing = key[terminal_start:]
    if re.search(r"\b(?:is|are|be|must be|needs? to be)\s+required\b", trailing):
        return True
    clause = re.split(r"(?:[/|,;]|\s[-–—:]\s)", context)[-1]
    words = re.findall(r"[a-z0-9]+", clause)
    if set(words) & FUTURE_STATUS_MARKERS:
        return True
    for index in range(len(words) - 1, -1, -1):
        if words[index] in {"not", "never"}:
            if all(word in NEGATION_QUALIFIERS for word in words[index + 1 :]):
                return True
            break
        if words[index] == "no":
            if words[index + 1 :] in (["longer"], ["longer", "fully"]):
                return True
            break
        if words[index] == "without":
            if words[index + 1 :] in (["being"], ["being", "fully"]):
                return True
            break
    for phrase in FUTURE_STATUS_PHRASES:
        if len(words) >= len(phrase) and tuple(words[-len(phrase) :]) == phrase:
            return True
    return False


def status_classification(value: str) -> str | None:
    key = _status_key(value)
    active_prefix = _matching_prefix(key, ACTIVE_STATUSES)
    terminal_prefix = _matching_prefix(key, TERMINAL_STATUSES)
    if terminal_prefix is not None:
        return "terminal"
    if active_prefix is None:
        return None

    _active_status, active_match = active_prefix
    for status in sorted(TERMINAL_STATUSES, key=len, reverse=True):
        for terminal in re.finditer(rf"\b{re.escape(status)}(?:\b|$)", key):
            if terminal.start() < active_match.end():
                continue
            if _terminal_match_is_pending(key, active_match.end(), terminal.start()):
                continue
            return "terminal"
    return "active"


def _blocked_condition_is_meaningful(value: str) -> bool:
    key = _status_key(value)
    prefix = _matching_prefix(key, {"blocked"})
    if prefix is None:
        return True
    _status, match = prefix
    return len(re.findall(r"[a-z0-9]+", key[match.end() :])) >= 2


def _split_row(line: str) -> list[str]:
    stripped = line.strip()
    if not stripped.startswith("|") or not stripped.endswith("|"):
        raise LedgerError(f"ledger table row must begin and end with '|': {line!r}")
    cells: list[str] = []
    current: list[str] = []
    body = stripped[1:-1]
    index = 0
    while index < len(body):
        char = body[index]
        if char == "\\" and index + 1 < len(body) and body[index + 1] in {"\\", "|"}:
            current.append(body[index + 1])
            index += 2
            continue
        if char == "|":
            cells.append("".join(current).strip())
            current = []
        else:
            current.append(char)
        index += 1
    cells.append("".join(current).strip())
    return cells


def _is_separator(cells: list[str]) -> bool:
    return len(cells) == len(HEADERS) and all(re.fullmatch(r":?-{3,}:?", cell) for cell in cells)


def validate_rows(rows: list[LedgerRow], *, allow_empty: bool = False) -> None:
    if not rows and not allow_empty:
        raise LedgerError("present CompletionLedger.md must contain at least one active row")
    seen: set[str] = set()
    for row in rows:
        values = row.as_dict()
        empty = [name for name, value in values.items() if not isinstance(value, str) or not value.strip()]
        if empty:
            raise LedgerError(f"ledger row {row.id!r} has empty fields: {', '.join(empty)}")
        multiline = [name for name, value in values.items() if "\n" in value or "\r" in value]
        if multiline:
            raise LedgerError(f"ledger row {row.id!r} has multiline fields: {', '.join(multiline)}")
        padded = [name for name, value in values.items() if value != value.strip()]
        if padded:
            raise LedgerError(f"ledger row {row.id!r} has leading or trailing whitespace: {', '.join(padded)}")
        folded = row.id.casefold()
        if folded in seen:
            raise LedgerError(f"duplicate completion-ledger ID: {row.id}")
        seen.add(folded)
        classification = status_classification(row.status)
        if classification == "terminal":
            raise LedgerError(f"terminal Status {row.status!r} must be removed for row {row.id}")
        if classification != "active":
            raise LedgerError(f"unrecognized Status {row.status!r} for row {row.id}; use an active status")
        if not _blocked_condition_is_meaningful(row.status):
            raise LedgerError(
                f"blocked Status for row {row.id} must name a meaningful unblock condition"
            )


def parse_ledger(text: str) -> list[LedgerRow]:
    nonblank = [line.strip() for line in text.splitlines() if line.strip()]
    if len(nonblank) < 4 or nonblank[0] != TITLE:
        raise LedgerError(f"ledger must contain only {TITLE!r} and one active table")
    header = _split_row(nonblank[1])
    if tuple(header) != HEADERS:
        raise LedgerError("completion-ledger columns must be exactly: " + " | ".join(HEADERS))
    if not _is_separator(_split_row(nonblank[2])):
        raise LedgerError("completion-ledger table separator is malformed")
    rows: list[LedgerRow] = []
    for line in nonblank[3:]:
        cells = _split_row(line)
        if len(cells) != len(HEADERS):
            raise LedgerError(f"completion-ledger row has {len(cells)} cells; expected {len(HEADERS)}")
        rows.append(LedgerRow(*cells))
    validate_rows(rows)
    return rows


def read_text_nofollow(path: Path, *, anchor: Path | None = None) -> str | None:
    """Read a regular UTF-8 file without following any supplied path symlink.

    ``None`` means the path does not exist.  Other unsafe objects and unstable
    reads fail closed with ``LedgerError`` instead of being treated as absence.
    """

    expanded = path.expanduser()
    if ".." in expanded.parts:
        raise LedgerError(f"ledger path contains a parent traversal component: {path}")
    raw_path = Path(os.path.abspath(os.fspath(expanded)))
    if anchor is None:
        parts = raw_path.parts
        start = parts[0]
        components = parts[1:]
    else:
        expanded_anchor = anchor.expanduser()
        if ".." in expanded_anchor.parts:
            raise LedgerError(f"ledger anchor contains a parent traversal component: {anchor}")
        anchor_path = Path(os.path.abspath(os.fspath(expanded_anchor)))
        try:
            relative = raw_path.relative_to(anchor_path)
        except ValueError as exc:
            raise LedgerError(f"ledger path escapes its trusted project root: {path}") from exc
        start = os.fspath(anchor_path)
        components = relative.parts
    if not components:
        raise LedgerError(f"ledger path is not a regular file: {path}")

    nofollow = getattr(os, "O_NOFOLLOW", 0)
    directory_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | nofollow
    file_flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NONBLOCK | nofollow
    try:
        directory_fd = os.open(start, directory_flags)
    except OSError as exc:
        raise LedgerError(f"ledger anchor is symlinked or unsafe: {path}") from exc
    file_fd: int | None = None
    try:
        for component in components[:-1]:
            try:
                next_fd = os.open(component, directory_flags, dir_fd=directory_fd)
            except FileNotFoundError:
                return None
            except OSError as exc:
                raise LedgerError(
                    f"ledger path contains a symlink or unsafe directory component: {path}"
                ) from exc
            os.close(directory_fd)
            directory_fd = next_fd

        try:
            file_fd = os.open(components[-1], file_flags, dir_fd=directory_fd)
        except FileNotFoundError:
            return None
        except OSError as exc:
            raise LedgerError(f"ledger path is symlinked or unsafe: {path}") from exc

        before = os.fstat(file_fd)
        if not stat.S_ISREG(before.st_mode):
            raise LedgerError(f"ledger path is not a regular file: {path}")
        chunks: list[bytes] = []
        while True:
            chunk = os.read(file_fd, 1024 * 1024)
            if not chunk:
                break
            chunks.append(chunk)
        after = os.fstat(file_fd)
        before_identity = (
            before.st_dev,
            before.st_ino,
            before.st_size,
            before.st_mtime_ns,
            before.st_ctime_ns,
        )
        after_identity = (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_mtime_ns,
            after.st_ctime_ns,
        )
        if before_identity != after_identity:
            raise LedgerError(f"ledger changed while it was being read: {path}")
        try:
            path_after = os.stat(components[-1], dir_fd=directory_fd, follow_symlinks=False)
        except FileNotFoundError as exc:
            raise LedgerError(f"ledger was replaced while it was being read: {path}") from exc
        path_identity = (
            path_after.st_dev,
            path_after.st_ino,
            path_after.st_size,
            path_after.st_mtime_ns,
            path_after.st_ctime_ns,
        )
        if not stat.S_ISREG(path_after.st_mode) or path_identity != after_identity:
            raise LedgerError(f"ledger was replaced while it was being read: {path}")
        rebound_directory_fd: int | None = None
        try:
            rebound_directory_fd = os.open(start, directory_flags)
            for component in components[:-1]:
                next_fd = os.open(component, directory_flags, dir_fd=rebound_directory_fd)
                os.close(rebound_directory_fd)
                rebound_directory_fd = next_fd
            original_parent = os.fstat(directory_fd)
            rebound_parent = os.fstat(rebound_directory_fd)
            if (original_parent.st_dev, original_parent.st_ino) != (
                rebound_parent.st_dev,
                rebound_parent.st_ino,
            ):
                raise LedgerError(f"ledger parent path changed while it was being read: {path}")
        except LedgerError:
            raise
        except OSError as exc:
            raise LedgerError(f"ledger parent path changed while it was being read: {path}") from exc
        finally:
            if rebound_directory_fd is not None:
                os.close(rebound_directory_fd)
        try:
            return b"".join(chunks).decode("utf-8")
        except UnicodeDecodeError as exc:
            raise LedgerError(f"ledger is not valid UTF-8: {path}") from exc
    finally:
        if file_fd is not None:
            os.close(file_fd)
        os.close(directory_fd)


def _escape_cell(value: str) -> str:
    return value.replace("\\", "\\\\").replace("|", "\\|")


def render_row(row: LedgerRow) -> str:
    validate_rows([row])
    return "| " + " | ".join(_escape_cell(value) for value in row.as_dict().values()) + " |"


def render_ledger(rows: list[LedgerRow]) -> str:
    validate_rows(rows)
    lines = [
        TITLE,
        "",
        "| " + " | ".join(HEADERS) + " |",
        "| " + " | ".join("---" for _ in HEADERS) + " |",
    ]
    for row in rows:
        lines.append(render_row(row))
    return "\n".join(lines) + "\n"
