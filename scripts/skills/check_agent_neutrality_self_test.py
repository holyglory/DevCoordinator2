#!/usr/bin/env python3
"""Recall and precision tests for shared agent-contract neutrality."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path
from shutil import rmtree


SCRIPT = Path(__file__).with_name("check_agent_neutrality.py")
SPEC = importlib.util.spec_from_file_location("agent_neutrality", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load neutrality checker")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def fixture(root: Path) -> None:
    write(root / "reference/universal/AGENTS.md", "# Universal Agent Instructions\n")
    write(root / "SKILL_AUDIT.md", "# Agent Skills Audit\n")
    write(root / "skills/sample/SKILL.md", "---\nname: sample\n---\nUse an isolated worker.\n")
    write(root / "skills/sample/agents/openai.yaml", "interface:\n  default_prompt: Use an isolated worker.\n")
    write(root / "full_repo_harness/queue.py", "PROMPT = 'Use an isolated worker.'\n")


def main() -> int:
    raw = tempfile.mkdtemp(prefix="agent-neutrality-self-test-")
    root = Path(raw)
    try:
        fixture(root)
        assert not MODULE.audit(root)

        write(root / "skills/sample/SKILL.md", "In Codex set fork_turns to none.\n")
        rules = {finding.rule for finding in MODULE.audit(root)}
        assert {"runtime-name", "runtime-api"} <= rules

        write(root / "skills/sample/SKILL.md", "Use an isolated worker.\n")
        write(root / "adapter.txt", "Install in ~/.codex/skills.\n")
        assert not MODULE.audit(root), "adapter-only runtime naming must remain out of shared scope"

        print("agent neutrality self-test ok")
        return 0
    finally:
        rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
