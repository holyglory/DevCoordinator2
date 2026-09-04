#!/usr/bin/env python3
"""Fail if committable repository content contains installation-specific strings.

The pattern list is deliberately untracked (instance/forbidden-strings.txt)
so the forbidden strings themselves never enter the repository. Scans every
file git would track (cached + untracked, minus gitignored), case-insensitive
substring match. Exit 0 = clean, 1 = findings, 2 = cannot run.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PATTERNS_FILE = REPO / "instance" / "forbidden-strings.txt"


def load_patterns() -> list[str]:
    if not PATTERNS_FILE.is_file():
        print(f"missing pattern list: {PATTERNS_FILE}", file=sys.stderr)
        sys.exit(2)
    patterns = []
    for line in PATTERNS_FILE.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            patterns.append(line.lower())
    if not patterns:
        print("pattern list is empty", file=sys.stderr)
        sys.exit(2)
    return patterns


def committable_files() -> list[Path]:
    proc = subprocess.run(
        ["git", "-C", str(REPO), "ls-files", "--cached", "--others",
         "--exclude-standard", "-z"],
        capture_output=True, check=True,
    )
    names = [n for n in proc.stdout.decode("utf-8").split("\0") if n]
    return [REPO / n for n in names]


def main() -> int:
    patterns = load_patterns()
    findings = 0
    for path in committable_files():
        try:
            data = path.read_bytes()
        except OSError:
            continue
        try:
            text = data.decode("utf-8").lower()
        except UnicodeDecodeError:
            continue  # binary; instance strings are textual
        for lineno, line in enumerate(text.splitlines(), 1):
            for pat in patterns:
                if pat in line:
                    rel = path.relative_to(REPO)
                    print(f"{rel}:{lineno}: contains forbidden pattern")
                    findings += 1
    if findings:
        print(f"{findings} instance-data finding(s); see instance/forbidden-strings.txt")
        return 1
    print("clean: no instance-specific strings in committable content")
    return 0


if __name__ == "__main__":
    sys.exit(main())
