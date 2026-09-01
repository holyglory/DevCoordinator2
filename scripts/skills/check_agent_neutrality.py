#!/usr/bin/env python3
"""Reject runtime-specific assumptions from shared agent contracts and prompts."""

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MARKUP_PATTERNS = (
    ("runtime-name", re.compile(r"\b(?:Codex|Claude(?: Code)?)\b", re.IGNORECASE)),
    ("runtime-home", re.compile(r"\.(?:codex|claude)(?:[/\\]|\b)", re.IGNORECASE)),
    ("runtime-api", re.compile(r"\b(?:fork_turns|request_user_input|AskUserQuestion)\b")),
)
PROMPT_PATTERNS = (
    ("runtime-name", re.compile(r"\b(?:in Codex|Codex workers?|Codex skills?|Claude Code)\b", re.IGNORECASE)),
    ("runtime-api", re.compile(r"\b(?:fork_turns|request_user_input|AskUserQuestion)\b")),
)


@dataclass(frozen=True)
class Finding:
    rule: str
    path: str
    line: int


def shared_markup(root: Path) -> list[Path]:
    result = [root / "reference" / "universal" / "AGENTS.md", root / "SKILL_AUDIT.md"]
    for skill in sorted((root / "skills").iterdir()):
        if not skill.is_dir():
            continue
        for name in ("SKILL.md", "README.md"):
            path = skill / name
            if path.is_file():
                result.append(path)
        result.extend(sorted((skill / "references").glob("**/*.md")))
        result.extend(sorted((skill / "agents").glob("*.yaml")))
    return result


def prompt_sources(root: Path) -> list[Path]:
    candidates = [root / "full_repo_harness" / "queue.py"]
    candidates.extend((root / "skills").glob("*/scripts/build*.py"))
    candidates.extend((root / "skills").glob("*/scripts/_vendor/full_repo_harness/queue.py"))
    return sorted(path for path in candidates if path.is_file())


def scan(paths: list[Path], patterns: tuple[tuple[str, re.Pattern[str]], ...], root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for path in paths:
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
            for rule, pattern in patterns:
                if pattern.search(line):
                    findings.append(Finding(rule, path.relative_to(root).as_posix(), line_number))
    return findings


def audit(root: Path) -> list[Finding]:
    root = root.resolve()
    return scan(shared_markup(root), MARKUP_PATTERNS, root) + scan(
        prompt_sources(root), PROMPT_PATTERNS, root
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    findings = audit(args.root)
    for finding in findings:
        print(f"{finding.path}:{finding.line}: {finding.rule}: shared contract is runtime-specific")
    if findings:
        return 1
    print("agent neutrality check ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
