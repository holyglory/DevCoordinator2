#!/usr/bin/env python3
"""One-time import of a DecisionHistory.md file into the coordinator's
decision history (REQ-PLAN-10, DC2-2026-08-24-PLANNING-LEDGER).

Parses the established format — `## <REF> — <title>` sections with
`**Decision.**` / `**Alternatives.**` / `**Owner context.**` paragraphs —
and records each entry through the reviewed `decision.record` command with
its stable ref preserved, in file order so sequence numbers follow history.
Idempotent: an entry whose ref already exists is skipped. Run with
`--dry-run` first and review the aspect mapping; aspects are keyword
guesses and `--aspect REF=aspect` overrides any of them.

Usage:
    python3 scripts/decision_import.py --path /path/to/repo [--file DecisionHistory.md]
        [--dry-run] [--aspect REF=aspect ...]
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from devcoordinator2.client.common import DaemonUnavailable, call
from devcoordinator2.paths import load_instance_config

BODY_MAX = 4000
TITLE_MAX = 120

_ASPECT_KEYWORDS = (
    ("ui", ("console", "button", "page", "view", "ux", "pop-up", "dialog")),
    ("deployment", ("deploy", "edge", "route", "domain", "proxy", "canary",
                    "cutover", "install", "docker", "container")),
    ("data", ("database", "schema", "sqlite", "table", "ledger", "import",
              "export", "storage")),
    ("security", ("access", "permission", "grant", "secret", "auth", "socket",
                  "trust", "acl")),
    ("testing", ("test", "verification", "playwright", "acceptance")),
    ("process", ("workflow", "phase", "owner decision", "handover", "skill")),
)


@dataclass
class Entry:
    ref: str
    title: str
    aspect: str
    body: str
    technical_note: str | None


def guess_aspect(text: str) -> str:
    lowered = text.lower()
    best, hits = "architecture", 0
    for aspect, words in _ASPECT_KEYWORDS:
        count = sum(lowered.count(w) for w in words)
        if count > hits:
            best, hits = aspect, count
    return best


def _plain(text: str) -> str:
    text = re.sub(r"\*\*(.+?)\*\*", r"\1", text, flags=re.S)  # drop bold markers
    text = re.sub(r"[ \t]+", " ", text)
    return re.sub(r"\n{3,}", "\n\n", text).strip()


def parse(text: str) -> list[Entry]:
    entries: list[Entry] = []
    sections = re.split(r"^## +", text, flags=re.M)[1:]
    for section in sections:
        head, _, rest = section.partition("\n")
        match = re.match(r"(\S+) +— +(.+)", head.strip())
        if not match:
            continue
        ref, title = match.group(1), match.group(2).strip()
        if len(title) > TITLE_MAX:
            title = title[: TITLE_MAX - 1].rstrip() + "…"
        body = _plain(rest)
        technical_note = None
        if len(body) > BODY_MAX:
            suffix = "\n\n(continued in the technical note)"
            cut = body.rfind("\n\n", 0, BODY_MAX - len(suffix))
            cut = cut if cut > 0 else BODY_MAX - len(suffix)
            body, technical_note = body[:cut].rstrip() + suffix, body[cut:].strip()
        entries.append(Entry(ref=ref, title=title, aspect=guess_aspect(section),
                             body=body, technical_note=technical_note))
    return entries


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--path", required=True,
                        help="path inside the target repository")
    parser.add_argument("--file", default=None,
                        help="markdown file (default: <path>/DecisionHistory.md)")
    parser.add_argument("--dry-run", action="store_true",
                        help="print the planned import without recording")
    parser.add_argument("--aspect", action="append", default=[],
                        metavar="REF=aspect", help="override a guessed aspect")
    ns = parser.parse_args(argv)
    repo = Path(ns.path).resolve()
    source = Path(ns.file) if ns.file else repo / "DecisionHistory.md"
    overrides = dict(item.split("=", 1) for item in ns.aspect)
    entries = parse(source.read_text())
    for entry in entries:
        entry.aspect = overrides.get(entry.ref, entry.aspect)
    if not entries:
        print(f"nothing to import: no `## REF — title` sections in {source}")
        return 1
    if ns.dry_run:
        for e in entries:
            note = f" (+{len(e.technical_note)} chars technical note)" \
                if e.technical_note else ""
            print(f"{e.ref}: aspect={e.aspect} title={e.title!r}"
                  f" body={len(e.body)} chars{note}")
        print(f"dry run: {len(entries)} entries, nothing recorded")
        return 0
    config = load_instance_config()
    imported = skipped = failed = 0
    for e in entries:
        args = {"path": str(repo), "aspect": e.aspect, "title": e.title,
                "body": e.body, "ref": e.ref}
        if e.technical_note:
            args["technical_note"] = e.technical_note
        try:
            response = call(config.socket_path, "decision.record", args)
        except DaemonUnavailable as exc:
            print(f"ABORT: daemon unavailable at {e.ref}: {exc}")
            return 2
        if response.get("ok"):
            imported += 1
            print(f"imported {e.ref} as seq {response['result']['seq']}")
        elif "already used" in response.get("error", {}).get("message", ""):
            skipped += 1
            print(f"skipped {e.ref}: already imported")
        else:
            failed += 1
            print(f"FAILED {e.ref}: {response.get('error')}")
    print(f"done: {imported} imported, {skipped} already present, {failed} failed")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
