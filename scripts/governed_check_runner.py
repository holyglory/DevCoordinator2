#!/usr/bin/env python3
"""Run a daemon-prepared governed-check plan as the physical caller."""

from __future__ import annotations

import sys
from pathlib import Path


def main() -> int:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
    from devcoordinator2.check_runner import main as runner_main
    return runner_main()


if __name__ == "__main__":
    raise SystemExit(main())
