#!/usr/bin/env python3
"""Compatibility wrapper for the shared full-repository audit queue harness."""

from __future__ import annotations

import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
REPO_ROOT = Path(__file__).resolve().parents[3]
if not (REPO_ROOT / "full_repo_harness" / "queue.py").is_file():
    raise RuntimeError("full-repo-audit must resolve from the canonical DevCoordinator2 skill link")
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

import full_repo_harness.queue as _queue

_queue.COMPANION_SCRIPT_DIR = SCRIPT_DIR

from full_repo_harness.queue import *  # noqa: F401,F403,E402
from full_repo_harness.queue import main  # noqa: E402


if __name__ == "__main__":
    raise SystemExit(main())
