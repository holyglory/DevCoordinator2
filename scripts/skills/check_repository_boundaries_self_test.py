#!/usr/bin/env python3
"""Recall and precision tests for unified repository ownership."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path
from shutil import rmtree


SCRIPT = Path(__file__).with_name("check_repository_boundaries.py")
SPEC = importlib.util.spec_from_file_location("repository_boundaries", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load repository boundary checker")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def fixture(root: Path) -> None:
    for name in MODULE.CANONICAL_SKILLS:
        write(root / "skills" / name / "SKILL.md", f"---\nname: {name}\n---\n")
    write(root / MODULE.CANONICAL_POLICY, "# Universal Agent Instructions\n")
    write(root / "src" / "product.py", "PRODUCT = 'DevCoordinator2'\n")
    write(
        root / "docs" / "holy-skills-snapshot.md",
        "Imported /home/holyskills historically.\n",
    )


def rules(report: dict[str, object]) -> set[str]:
    return {finding["rule"] for finding in report["findings"]}


def main() -> int:
    raw = tempfile.mkdtemp(prefix="repository-boundary-self-test-")
    root = Path(raw)
    try:
        fixture(root)
        assert MODULE.audit_repository(root)["ok"] is True

        missing = root / "skills" / "dev-coordinator" / "SKILL.md"
        missing.unlink()
        assert "canonical-skill-set" in rules(MODULE.audit_repository(root))
        write(missing, "---\nname: dev-coordinator\n---\n")

        old_policy = root / "reference" / "codex-app-wide" / "AGENTS.md"
        write(old_policy, "old\n")
        assert "retired-path-present" in rules(MODULE.audit_repository(root))
        old_policy.unlink()
        old_policy.parent.rmdir()

        active = root / "scripts" / "install.py"
        write(active, "SOURCE = '/home/holyskills/skills'\n")  # public-artifact-guard: allow text-private-home
        assert "retired-checkout" in rules(MODULE.audit_repository(root))
        active.unlink()

        write(root / "README.md", "Install from github.com/example/holyskills.\n")
        assert "retired-remote" in rules(MODULE.audit_repository(root))

        print("repository boundary self-test ok")
        return 0
    finally:
        rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
