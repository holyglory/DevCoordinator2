#!/usr/bin/env python3
"""Compatibility wrapper for the shared full-repository audit queue harness."""

from __future__ import annotations

import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
REPO_ROOT = Path(__file__).resolve().parents[3]
VENDOR_ROOT = Path(__file__).resolve().parent / "_vendor"
DEV_SKILL_DIR = (REPO_ROOT / "skills" / "full-repo-audit").resolve()
running_in_dev_repo = DEV_SKILL_DIR == SKILL_DIR.resolve() and (REPO_ROOT / "full_repo_harness" / "queue.py").is_file()

path_roots = [REPO_ROOT, VENDOR_ROOT] if running_in_dev_repo else [VENDOR_ROOT]
for root in reversed([item for item in path_roots if item.is_dir()]):
    root_text = str(root)
    if root_text not in sys.path:
        sys.path.insert(0, root_text)

import full_repo_harness.queue as _queue

_queue.COMPANION_SCRIPT_DIR = SCRIPT_DIR

from full_repo_harness.queue import *  # noqa: F401,F403,E402
from full_repo_harness.queue import main  # noqa: E402


if __name__ == "__main__":
    raise SystemExit(main())
