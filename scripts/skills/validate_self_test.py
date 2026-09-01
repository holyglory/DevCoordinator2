#!/usr/bin/env python3
"""Prove the repository validation runner collects instead of short-circuiting."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


SCRIPT = Path(__file__).with_name("validate.py")
SPEC = importlib.util.spec_from_file_location("agent_skills_validate", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("unable to load validation runner")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def main() -> int:
    saved = list(MODULE.FAILURES)
    MODULE.FAILURES.clear()
    try:
        with tempfile.TemporaryDirectory(prefix="validate-cycle-") as raw:
            marker = Path(raw) / "later-command-ran"
            first = MODULE.run([sys.executable, "-c", "raise SystemExit(3)"])
            later = MODULE.run(
                [
                    sys.executable,
                    "-c",
                    "from pathlib import Path; Path({!r}).write_text('ran', encoding='utf-8')".format(
                        str(marker)
                    ),
                ]
            )
            second = MODULE.run([sys.executable, "-c", "raise SystemExit(7)"])
            check(not first and later and not second, "command statuses were not retained")
            check(marker.read_text(encoding="utf-8") == "ran", "later command did not run")
            check(len(MODULE.FAILURES) == 2, "both command failures must be collected")

            explicit_environment = MODULE.run(
                [
                    sys.executable,
                    "-c",
                    "import os; raise SystemExit(0 if os.environ.get('AGENT_SKILLS_VALIDATE_TEST_DEPENDENCY') == 'explicit' else 9)",
                ],
                extra_env={"AGENT_SKILLS_VALIDATE_TEST_DEPENDENCY": "explicit"},
            )
            check(explicit_environment, "explicit validation dependency environment was not forwarded")

            validate_source = SCRIPT.read_text(encoding="utf-8")
            check(
                'copied / "scripts" / "standalone_smoke.py"' in validate_source,
                "formal standalone validation must use the focused copied-package smoke",
            )
            check(
                'run([sys.executable, str(skill.relative_to(ROOT) / "scripts" / "self_test.py")])' in validate_source,
                "canonical validation must retain each complete skill self-test",
            )

            parity_root = Path(raw) / "parity"
            parity_files = {
                "skills/formal-web-ui-verification/SKILL.md": "review-queue.json formal_web_ui_review.py secondary-workflow-precedes-primary declared-theme-contradiction",
                "skills/user-journey-docs-audit/SKILL.md": "Formal Web UI verification handoff continuation anchor changed visual review",
                "skills/ui-implementation-audit/SKILL.md": (
                    "import_formal_web_evidence.py runtime/user-selected manual-review"),
                "skills/full-repo-audit/SKILL.md": "Changed Visual Review formal_web_ui_review.py manual-review",
                "full_repo_harness/evidence.py": '"review-queue" "manual-review" formal-web-ui-manual-review',
                "full_repo_harness/queue.py": "Changed Visual Review review-queue.json formal_web_ui_review.py",
            }
            for relative, content in parity_files.items():
                target = parity_root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content, encoding="utf-8")
            original_root = MODULE.ROOT
            original_harness = MODULE.HARNESS
            MODULE.ROOT = parity_root
            MODULE.HARNESS = parity_root / "full_repo_harness"
            try:
                MODULE.check_changed_visual_review_parity()
                broken = parity_root / "skills" / "ui-implementation-audit" / "SKILL.md"
                broken.write_text("import_formal_web_evidence.py manual-review", encoding="utf-8")
                try:
                    MODULE.check_changed_visual_review_parity()
                except SystemExit as error:
                    check(
                        "runtime/user-selected" in str(error),
                        "parity failure should identify the missing worker-effort boundary",
                    )
                else:
                    raise AssertionError("changed visual-review parity accepted a missing carried-image rule")
            finally:
                MODULE.ROOT = original_root
                MODULE.HARNESS = original_harness

            MODULE.attempt("first in-process check", lambda: (_ for _ in ()).throw(ValueError("gap")))
            in_process_marker: list[str] = []
            MODULE.attempt("later in-process check", lambda: in_process_marker.append("ran"))
            check(in_process_marker == ["ran"], "later in-process check did not run")
            check(
                any("first in-process check" in failure for failure in MODULE.FAILURES),
                "in-process failure was not retained",
            )
    finally:
        MODULE.FAILURES[:] = saved

    print("validation runner self-test ok (failures collected; later checks executed)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
