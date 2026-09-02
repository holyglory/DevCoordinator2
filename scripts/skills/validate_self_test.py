#!/usr/bin/env python3
"""Prove Rust-owned scheduling, preflight invalidation, and bounded receipts."""

from __future__ import annotations

import importlib.util
import json
import subprocess
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


def command(
    argv: list[str], *, cwd: Path, expected: int = 0
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        argv,
        cwd=cwd,
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )
    if completed.returncode != expected:
        raise AssertionError(
            f"expected {expected}, got {completed.returncode}: {argv}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed


def rust_source_digest(executor: Path, repository: Path) -> str:
    completed = command(
        [str(executor), "source-digest", "--worktree", str(repository)],
        cwd=repository,
    )
    receipt = json.loads(completed.stdout)
    digest = receipt.get("sha256")
    check(isinstance(digest, str) and len(digest) == 64, "source digest receipt is invalid")
    return digest


def test_plan_contract(base: Path) -> None:
    current = MODULE.ROOT / ".devcoordinator" / "agent-validation" / "shape-only"
    plan = MODULE.build_validation_plan(
        run_id="shape-only",
        current_dir=current,
        source_digest="a" * 64,
    )
    check(plan["schema"] == 2 and plan["proof"] == "complete", "plan is not strict schema 2")
    check(
        plan["requested_tier"] == "release" and plan["readiness_eligible"] is True,
        "complete validation must request release proof",
    )
    checks = plan["checks"]
    preflights = [row for row in checks if row["role"] == "preflight"]
    targets = [row for row in checks if row["role"] == "work"]
    preflight_names = {row["name"] for row in preflights}
    target_names = {row["name"] for row in targets}
    check(len(preflights) >= 10 and len(targets) >= 10, "complete validation coverage shrank")
    check(
        all(set(row["invalidates"]) == target_names for row in preflights),
        "each cheap preflight must invalidate every expensive target",
    )
    check(
        all(set(row["requires"]) == preflight_names for row in targets),
        "every expensive target must require every invalidating preflight",
    )
    check(
        all(row["on_failure"] == "continue" for row in checks),
        "all-settled validation must not fail-fast siblings",
    )
    check(
        all(row["timeout_seconds"] == 900 for row in checks),
        "every direct validation leaf must have a bounded failure deadline",
    )
    declared = {argument for row in checks for argument in row["command"]}
    for skill in MODULE.SKILL_NAMES:
        expected = f"skills/{skill}/scripts/self_test.py"
        check(expected in declared, f"missing complete self-test for {skill}")
    check(
        MODULE.executor_argv(Path("/executor"), base / "plan.json")
        == ["/executor", "run-local", str(base / "plan.json")],
        "top-level validation must dispatch only through Rust run-local",
    )
    workflow = (MODULE.ROOT / ".github" / "workflows" / "validate.yml").read_text(
        encoding="utf-8"
    )
    check("cargo test --locked --workspace" in workflow, "CI must test the Rust workspace")
    release_build = "cargo build --locked --release --package devcoordinator2-executor"
    check(
        workflow.count(release_build) >= 3,
        "each CI job that exercises the executor must build its release binary",
    )
    check("needs: rust" not in workflow, "independent CI jobs must start concurrently")


def test_bounded_receipt(base: Path) -> None:
    failures = [
        {
            "check": f"check-{index}",
            "status": "failed",
            "reason": "fixture",
            "output_ref": f"checks/check-{index}",
        }
        for index in range(25)
    ]
    log = base / "checks" / "check-0" / "stderr.log"
    log.parent.mkdir(parents=True)
    log.write_text("bounded diagnostic marker\n", encoding="utf-8")
    receipt = MODULE.bounded_receipt(
        {
            "schema": 2,
            "status": "failed",
            "counts": {"failed": 25},
            "checks": [{}] * 25,
            "failure_index": failures,
            "failure_index_truncated": False,
        },
        base / "check-report.json",
        include_diagnostics=True,
    )
    check(len(receipt["failure_index"]) == 20, "receipt failure index is not bounded")
    check(receipt["failure_index_truncated"] is True, "bounded receipt hid truncation")
    check(receipt["report"].endswith("check-report.json"), "receipt omitted report filename")
    check(
        receipt["failure_diagnostics"]
        == [{"check": "check-0", "stderr_tail": "bounded diagnostic marker"}],
        "CI failure receipt omitted the bounded cold-log diagnostic",
    )


def test_changed_visual_parity(base: Path) -> None:
    parity_root = base / "parity"
    parity_files = {
        "skills/formal-web-ui-verification/SKILL.md": (
            "review-queue.json formal_web_ui_review.py "
            "secondary-workflow-precedes-primary declared-theme-contradiction"
        ),
        "skills/user-journey-docs-audit/SKILL.md": (
            "Formal Web UI verification handoff continuation anchor changed visual review"
        ),
        "skills/ui-implementation-audit/SKILL.md": (
            "import_formal_web_evidence.py runtime/user-selected manual-review"
        ),
        "skills/full-repo-audit/SKILL.md": (
            "Changed Visual Review formal_web_ui_review.py manual-review"
        ),
        "full_repo_harness/evidence.py": (
            '"review-queue" "manual-review" formal-web-ui-manual-review'
        ),
        "full_repo_harness/queue.py": (
            "Changed Visual Review review-queue.json formal_web_ui_review.py"
        ),
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
            check("runtime/user-selected" in str(error), "parity failure lost exact drift")
        else:
            raise AssertionError("changed visual-review parity accepted missing behavior")
    finally:
        MODULE.ROOT = original_root
        MODULE.HARNESS = original_harness


def test_real_rust_invalidation(base: Path) -> None:
    executor = MODULE.EXECUTOR
    check(executor.is_file(), "release Rust executor is missing; build it before Python tests")
    repository = base / "rust-invalidation"
    repository.mkdir()
    (repository / "source.txt").write_text("fixture\n", encoding="utf-8")
    command(["git", "init", "-q"], cwd=repository)
    command(["git", "add", "source.txt"], cwd=repository)
    command(
        [
            "git",
            "-c",
            "user.name=validator-self-test",
            "-c",
            "user.email=validator@example.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
        cwd=repository,
    )
    current = repository / ".devcoordinator" / "self-test"
    current.mkdir(parents=True)
    independent_marker = current / "independent-ran"
    forbidden_marker = current / "invalidated-ran"

    def row(
        name: str,
        script: str,
        *,
        role: str = "work",
        requires: list[str] | None = None,
        invalidates: list[str] | None = None,
    ) -> dict:
        return {
            "name": name,
            "tier": "development",
            "role": role,
            "after": [],
            "requires": requires or [],
            "invalidates": invalidates or [],
            "cwd": ".",
            "env": {"PYTHONDONTWRITEBYTECODE": "1"},
            "timeout_seconds": None,
            "completion": "process",
            "on_failure": "continue",
            "produces": [],
            "command": [sys.executable, "-c", script],
        }

    plan = {
        "schema": 2,
        "run_id": "validator-self-test",
        "test": "validator",
        "worktree_root": str(repository),
        "current_dir": str(current),
        "requested_tier": "release",
        "readiness_eligible": True,
        "proof": "complete",
        "selection": [],
        "origin_run_id": None,
        "source_digest": rust_source_digest(executor, repository),
        "config_digest": "b" * 64,
        "reused": {},
        "checks": [
            row("gate", "raise SystemExit(7)", role="preflight", invalidates=["expensive"]),
            row(
                "expensive",
                f"from pathlib import Path; Path({str(forbidden_marker)!r}).write_text('bad')",
                requires=["gate"],
            ),
            row(
                "independent",
                f"from pathlib import Path; Path({str(independent_marker)!r}).write_text('ok')",
            ),
        ],
    }
    plan_path = current / "plan.json"
    plan_path.write_text(json.dumps(plan), encoding="utf-8")
    command([str(executor), "run-local", str(plan_path)], cwd=repository, expected=1)
    report = json.loads((current / "check-report.json").read_text(encoding="utf-8"))
    states = {row["name"]: row["status"] for row in report["checks"]}
    check(
        states == {"gate": "failed", "expensive": "invalidated", "independent": "passed"},
        f"Rust invalidation/all-settled states are wrong: {states}",
    )
    check(
        independent_marker.read_text(encoding="utf-8") == "ok",
        "later independent work did not finish after the ordinary failure",
    )
    check(not forbidden_marker.exists(), "invalidated expensive work executed")
    check(report["source_changed"] is False, "cold validation artifacts changed source digest")
    check(
        (current / "checks" / "gate" / "stderr.log").is_file(),
        "failed leaf did not retain a cold log",
    )


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="validate-rust-self-test-") as raw:
        base = Path(raw)
        test_plan_contract(base)
        test_bounded_receipt(base)
        test_changed_visual_parity(base)
        test_real_rust_invalidation(base)
    print("validation runner self-test ok (Rust dispatch, invalidation, all-settled evidence)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
