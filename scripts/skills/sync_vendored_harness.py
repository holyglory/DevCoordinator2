#!/usr/bin/env python3
"""Synchronize audit-skill fallback harnesses from the canonical package."""

from __future__ import annotations

import argparse
import hashlib
import shutil
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "full_repo_harness"
TARGETS = [
    ROOT / "skills" / "full-repo-audit" / "scripts" / "_vendor" / "full_repo_harness",
    ROOT / "skills" / "full-repo-test-coverage-audit" / "scripts" / "_vendor" / "full_repo_harness",
    ROOT / "skills" / "ui-implementation-audit" / "scripts" / "_vendor" / "full_repo_harness",
]


def files(root: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.is_symlink() or "__pycache__" in path.parts or path.suffix == ".pyc":
            continue
        result[path.relative_to(root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def sync_target(target: Path) -> None:
    expected_parent = ROOT / "skills"
    target.resolve().relative_to(expected_parent.resolve())
    if target.is_symlink():
        raise SystemExit(f"Refusing to replace symlinked vendor directory: {target}")
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(SOURCE, target, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))


def main() -> int:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    args = parser.parse_args()

    expected = files(SOURCE)
    if not expected:
        raise SystemExit(f"Canonical harness is empty: {SOURCE}")
    if args.write:
        for target in TARGETS:
            sync_target(target)

    stale = [str(target) for target in TARGETS if not target.is_dir() or files(target) != expected]
    if stale:
        raise SystemExit("Stale vendored harnesses:\n" + "\n".join(stale))
    print(f"vendored harnesses synchronized: {len(TARGETS)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
