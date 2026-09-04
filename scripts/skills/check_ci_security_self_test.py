#!/usr/bin/env python3
"""Recall and precision fixtures for the self-hosted CI security check."""

from __future__ import annotations

import importlib.util
import tempfile
from pathlib import Path


SCRIPT = Path(__file__).with_name("check_ci_security.py")
SPEC = importlib.util.spec_from_file_location("check_ci_security", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load CI security checker")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


VALID = """name: validate

on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:

jobs:
  hosted:
    runs-on: ubuntu-24.04
    steps:
      - run: true
  native-wsl:
    if: ${{ vars.WSL == 'enabled' && (github.event_name == 'push' || github.event_name == 'workflow_dispatch') }}
    runs-on: [self-hosted, linux, wsl]
    steps:
      - run: true
"""


def assert_passes(text: str, label: str) -> None:
    violations, _count = MODULE.find_violations(text)
    if violations:
        raise AssertionError("{} unexpectedly failed: {}".format(label, violations))


def assert_fails(text: str, label: str) -> None:
    violations, _count = MODULE.find_violations(text)
    if not violations:
        raise AssertionError("{} must be rejected".format(label))


def main() -> int:
    assert_passes(VALID, "trusted event allowlist")
    assert_passes(
        VALID.replace("  pull_request:\n", ""),
        "workflow without a pull-request trigger",
    )
    assert_passes(
        VALID.replace(
            "on:\n  push:\n    branches: [main]\n  pull_request:\n  workflow_dispatch:",
            "on: [push, pull_request, workflow_dispatch]",
        ),
        "inline trigger list",
    )
    assert_passes(
        VALID.replace(
            "runs-on: [self-hosted, linux, wsl]",
            "runs-on:\n      - self-hosted\n      - linux\n      - wsl",
        ),
        "multiline self-hosted labels",
    )
    assert_passes(
        VALID.replace("runs-on: [self-hosted, linux, wsl]", "runs-on: ubuntu-24.04").replace(
            "    if: ${{ vars.WSL == 'enabled' && (github.event_name == 'push' || github.event_name == 'workflow_dispatch') }}\n",
            "",
        ),
        "hosted jobs need no self-hosted guard",
    )
    assert_passes(
        VALID.replace(
            "  native-wsl:\n",
            "  native-wsl:\n    strategy:\n      matrix:\n        os: [ubuntu-24.04, windows-2025, macos-14]\n",
        ).replace("runs-on: [self-hosted, linux, wsl]", "runs-on: ${{ matrix.os }}").replace(
            "    if: ${{ vars.WSL == 'enabled' && (github.event_name == 'push' || github.event_name == 'workflow_dispatch') }}\n",
            "",
        ),
        "known hosted matrix needs no guard",
    )

    guarded = "    if: ${{ vars.WSL == 'enabled' && (github.event_name == 'push' || github.event_name == 'workflow_dispatch') }}\n"
    assert_fails(VALID.replace(guarded, ""), "unguarded self-hosted PR job")
    assert_fails(
        VALID.replace(guarded, "    if: ${{ vars.WSL == 'enabled' }}\n"),
        "feature flag without event allowlist",
    )
    assert_fails(
        VALID.replace(guarded, "    if: ${{ github.event_name != 'pull_request' }}\n"),
        "denylist instead of trusted-event allowlist",
    )
    assert_fails(
        VALID.replace(
            guarded,
            "    if: ${{ github.event_name == 'push' || github.event_name == 'workflow_dispatch' || github.event_name == 'pull_request' }}\n",
        ),
        "allowlist widened back to pull requests",
    )
    assert_fails(
        VALID.replace(guarded, guarded.rstrip()[:-3] + " || true }}\n"),
        "trusted events plus true disjunct",
    )
    assert_fails(
        VALID.replace(guarded, guarded.rstrip()[:-3] + " || always() }}\n"),
        "trusted events plus function disjunct",
    )
    assert_fails(
        VALID.replace("runs-on: [self-hosted, linux, wsl]", "runs-on: ${{ inputs.runner }}").replace(
            guarded,
            "",
        ),
        "unresolved runner expression",
    )
    assert_fails(
        VALID.replace("runs-on: [self-hosted, linux, wsl]", "runs-on: private-linux").replace(
            guarded,
            "",
        ),
        "unknown literal runner label",
    )
    assert_fails(
        VALID.replace(
            guarded,
            "    steps:\n      - if: ${{ github.event_name == 'push' || github.event_name == 'workflow_dispatch' }}\n        run: true\n",
        ),
        "step guard cannot protect runner allocation",
    )

    with tempfile.TemporaryDirectory(prefix="ci-security-self-test-") as raw:
        root = Path(raw)
        regular = root / "validate.yml"
        regular.write_text(VALID, encoding="utf-8")
        violations, count, pull_request = MODULE.check_workflow(regular)
        if violations or count != 1 or not pull_request:
            raise AssertionError("regular workflow check returned incorrect metadata")
        linked = root / "linked.yml"
        linked.symlink_to(regular)
        try:
            MODULE.check_workflow(linked)
        except MODULE.SecurityError:
            pass
        else:
            raise AssertionError("symlinked workflow input must be rejected")

    print("ci security checker self-test ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
