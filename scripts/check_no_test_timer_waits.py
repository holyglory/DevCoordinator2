#!/usr/bin/env python3
"""Reject clock-based progression in governed test and verification code."""

from __future__ import annotations

import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PYTHON_TARGETS = (
    "src/devcoordinator2/check_runner.py",
    "src/devcoordinator2/daemon/test_admission.py",
    "src/devcoordinator2/daemon/test_postgres.py",
    "src/devcoordinator2/daemon/tests_lifecycle.py",
    "src/devcoordinator2/daemon/systemd_unit.py",
    "src/devcoordinator2/daemon/metrics_sampler.py",
    "scripts/install.py",
    "tests/integration/helpers.py",
    *(
        str(path.relative_to(ROOT))
        for path in sorted((ROOT / "tests" / "integration").glob("test_*.py"))
    ),
)
JAVASCRIPT_TARGETS = ("console/verify.mjs",)


def _call_name(call: ast.Call) -> str:
    value = call.func
    parts = []
    while isinstance(value, ast.Attribute):
        parts.append(value.attr)
        value = value.value
    if isinstance(value, ast.Name):
        parts.append(value.id)
    return ".".join(reversed(parts))


def scan_python(path: Path, relative: str) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=relative)
    failures = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call) or _call_name(node) not in (
                "time.sleep", "asyncio.sleep"):
            continue
        duration = None
        if node.args and isinstance(node.args[0], ast.Constant) \
                and isinstance(node.args[0].value, int | float):
            duration = float(node.args[0].value)
        allowed_bounded_fallback = duration is not None and duration <= 0.1
        if not allowed_bounded_fallback:
            failures.append(
                f"{relative}:{node.lineno}: clock-based sleep is not completion evidence")
    return failures


def scan_javascript(path: Path, relative: str) -> list[str]:
    text = path.read_text(encoding="utf-8")
    failures = []
    for marker in ("waitForTimeout(", "setTimeout("):
        offset = 0
        while (index := text.find(marker, offset)) >= 0:
            line = text.count("\n", 0, index) + 1
            failures.append(
                f"{relative}:{line}: {marker[:-1]} is not completion evidence")
            offset = index + len(marker)
    return failures


def main() -> int:
    failures = []
    for relative in PYTHON_TARGETS:
        failures.extend(scan_python(ROOT / relative, relative))
    for relative in JAVASCRIPT_TARGETS:
        failures.extend(scan_javascript(ROOT / relative, relative))
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("governed test and verification waits are event-driven")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
