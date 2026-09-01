#!/usr/bin/env python3
"""Complete validation for the six DevCoordinator2 agent skills."""

from __future__ import annotations

import atexit
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable


ROOT = Path(__file__).resolve().parents[2]
HARNESS = ROOT / "full_repo_harness"
PYCACHE_ROOT = Path(tempfile.mkdtemp(prefix="devcoordinator2-agent-validation-pycache-"))
atexit.register(shutil.rmtree, PYCACHE_ROOT, True)
SKILL_NAMES = (
    "dev-coordinator",
    "formal-web-ui-verification",
    "full-repo-audit",
    "full-repo-test-coverage-audit",
    "ui-implementation-audit",
    "user-journey-docs-audit",
)
SKILLS_WITH_REQUIRED_README = set(SKILL_NAMES)
SKILLS = tuple(ROOT / "skills" / name for name in SKILL_NAMES)
HARNESS_SKILL_NAMES = (
    "full-repo-audit",
    "full-repo-test-coverage-audit",
    "ui-implementation-audit",
)
FAILURES: list[str] = []


def run(
    args: list[str],
    *,
    cwd: Path = ROOT,
    extra_env: dict[str, str] | None = None,
) -> bool:
    """Run one command, retain its failure, and let the validation pass continue."""

    rendered = shlex.join(args)
    print("+", rendered, flush=True)
    environment = dict(os.environ)
    environment["PYTHONPYCACHEPREFIX"] = str(PYCACHE_ROOT)
    if extra_env:
        environment.update(extra_env)
    completed = subprocess.run(args, cwd=cwd, env=environment, check=False)
    if completed.returncode != 0:
        FAILURES.append(f"command exited {completed.returncode}: {rendered}")
        return False
    return True


def attempt(label: str, operation: Callable[[], object]) -> bool:
    """Run an in-process check without aborting independent later checks."""

    try:
        operation()
    except SystemExit as error:
        detail = str(error) or f"exit {error.code}"
        FAILURES.append(f"{label}: {detail}")
        return False
    except Exception as error:  # noqa: BLE001 - the pass must collect independent failures
        FAILURES.append(f"{label}: {type(error).__name__}: {error}")
        return False
    return True


def print_failure_summary() -> None:
    print(f"validation failed with {len(FAILURES)} collected failure(s):", flush=True)
    for index, failure in enumerate(FAILURES, start=1):
        print(f"  {index}. {failure}", flush=True)


def check_repository_layout() -> None:
    skills_root = ROOT / "skills"
    actual = {
        path.name
        for path in skills_root.iterdir()
        if path.is_dir() and not path.is_symlink() and (path / "SKILL.md").is_file()
    }
    expected = set(SKILL_NAMES)
    if actual != expected:
        missing = sorted(expected - actual)
        unexpected = sorted(actual - expected)
        raise SystemExit(f"Canonical skill set mismatch; missing={missing}, unexpected={unexpected}")
    for skill in SKILLS:
        required = [
            skill / "SKILL.md",
            skill / "agents" / "openai.yaml",
            skill / "scripts" / "self_test.py",
        ]
        if skill.name in SKILLS_WITH_REQUIRED_README:
            required.append(skill / "README.md")
        absent = [path.relative_to(ROOT).as_posix() for path in required if not path.is_file()]
        if absent:
            raise SystemExit(f"Incomplete skill {skill.name}: {', '.join(absent)}")


def check_canonical_harness_ownership() -> None:
    required_modules = ("queue.py", "verify_common.py", "evidence.py", "merge_findings.py")
    missing = [name for name in required_modules if not (HARNESS / name).is_file()]
    if missing:
        raise SystemExit(f"Canonical full_repo_harness is incomplete: {missing}")
    for skill_name in HARNESS_SKILL_NAMES:
        vendor = ROOT / "skills" / skill_name / "scripts" / "_vendor" / "full_repo_harness"
        if vendor.exists():
            raise SystemExit(f"Shared harness copy must not exist: {vendor}")
        scripts = ROOT / "skills" / skill_name / "scripts"
        for script in scripts.glob("*.py"):
            source = script.read_text(encoding="utf-8")
            if "_vendor/full_repo_harness" in source or "VENDOR_ROOT" in source:
                raise SystemExit(f"Skill still contains a shared-harness fallback: {script}")


def check_include_glob_exclusions() -> None:
    temporary = Path(tempfile.mkdtemp(prefix="include-glob-exclusion-"))
    try:
        repository = temporary / "repo"
        (repository / "src").mkdir(parents=True)
        (repository / "node_modules" / "pkg").mkdir(parents=True)
        (repository / "src" / "app.py").write_text("print(1)\n", encoding="utf-8")
        (repository / "node_modules" / "pkg" / "index.py").write_text("print(2)\n", encoding="utf-8")
        identity = [
            "-c", "user.name=agent-skills-validate",
            "-c", "user.email=validate@example.invalid",
        ]
        run(["git", "init", "-q"], cwd=repository)
        run(["git", "add", "src/app.py"], cwd=repository)
        run(["git", *identity, "commit", "-q", "-m", "init"], cwd=repository)

        broad = temporary / "broad"
        run(
            [
                sys.executable,
                "skills/full-repo-audit/scripts/build_audit_batches.py",
                "--repo", str(repository),
                "--out", str(broad),
                "--include-glob", "**/*.py",
            ]
        )
        broad_manifest = json.loads((broad / "manifest.json").read_text(encoding="utf-8"))
        broad_files = {item["rel_path"] for item in broad_manifest["source_files"]}
        if "node_modules/pkg/index.py" in broad_files:
            raise SystemExit("Broad --include-glob unexpectedly included node_modules")

        explicit = temporary / "explicit"
        run(
            [
                sys.executable,
                "skills/full-repo-audit/scripts/build_audit_batches.py",
                "--repo", str(repository),
                "--out", str(explicit),
                "--include-glob", "node_modules/**/*.py",
            ]
        )
        explicit_manifest = json.loads((explicit / "manifest.json").read_text(encoding="utf-8"))
        explicit_files = {item["rel_path"] for item in explicit_manifest["source_files"]}
        if "node_modules/pkg/index.py" not in explicit_files:
            raise SystemExit("Explicit --include-glob should include its targeted vendor path")
    finally:
        shutil.rmtree(temporary, ignore_errors=True)


def check_interaction_label_parity() -> None:
    canonical = HARNESS / "verify_common.py"
    text = canonical.read_text(encoding="utf-8")
    labels = (
        "badge-detail",
        "row-hit-target",
        "navigation-cursor",
        "transient-disclosure",
        "disclosure-scrollbar",
        "icon-meaning",
        "stable-expansion-width",
        "hover-copy",
        "status-summary",
        "message-metadata",
    )
    for label in labels:
        if label not in text:
            raise SystemExit(f"Canonical interaction checklist label missing: {label}")
    for skill in SKILLS:
        for verifier in (skill / "scripts").glob("verify_*.py"):
            if "INTERACTION_CHECKLIST_LABELS" in verifier.read_text(encoding="utf-8"):
                raise SystemExit(
                    f"{verifier} redefines INTERACTION_CHECKLIST_LABELS; import the shared constant"
                )


def check_changed_visual_review_parity() -> None:
    required = {
        ROOT / "skills" / "formal-web-ui-verification" / "SKILL.md": (
            "review-queue.json",
            "formal_web_ui_review.py",
            "secondary-workflow-precedes-primary",
            "declared-theme-contradiction",
        ),
        ROOT / "skills" / "user-journey-docs-audit" / "SKILL.md": (
            "Formal Web UI verification handoff",
            "continuation anchor",
            "changed visual review",
        ),
        ROOT / "skills" / "ui-implementation-audit" / "SKILL.md": (
            "import_formal_web_evidence.py",
            "runtime/user-selected",
            "manual-review",
        ),
        ROOT / "skills" / "full-repo-audit" / "SKILL.md": (
            "Changed Visual Review",
            "formal_web_ui_review.py",
            "manual-review",
        ),
        HARNESS / "evidence.py": (
            '"review-queue"',
            '"manual-review"',
            "formal-web-ui-manual-review",
        ),
        HARNESS / "queue.py": (
            "Changed Visual Review",
            "review-queue.json",
            "formal_web_ui_review.py",
        ),
    }
    for path, tokens in required.items():
        text = path.read_text(encoding="utf-8")
        missing = [token for token in tokens if token not in text]
        if missing:
            raise SystemExit(f"Changed visual-review contract drift in {path}: missing={missing}")


def main() -> int:
    FAILURES.clear()
    attempt("repository layout", check_repository_layout)
    run([sys.executable, "scripts/skills/validate_self_test.py"])
    run([sys.executable, "scripts/skills/check_app_wide_policy_self_test.py"])
    run([sys.executable, "scripts/skills/check_app_wide_policy.py"])
    run([sys.executable, "scripts/skills/check_agent_neutrality_self_test.py"])
    run([sys.executable, "scripts/skills/check_agent_neutrality.py"])
    run([sys.executable, "scripts/skills/check_user_issue_ledgers_self_test.py"])
    run([sys.executable, "scripts/skills/check_user_issue_ledgers.py"])
    run([sys.executable, "scripts/skills/check_repository_freshness_self_test.py"])
    run([sys.executable, "scripts/skills/check_repository_boundaries_self_test.py"])
    run([sys.executable, "scripts/skills/check_repository_boundaries.py", "--repo", str(ROOT)])
    run([sys.executable, "scripts/skills/check_ci_security_self_test.py"])
    run([sys.executable, "scripts/skills/check_ci_security.py"])
    attempt("canonical harness ownership", check_canonical_harness_ownership)
    attempt("interaction label parity", check_interaction_label_parity)
    attempt("changed visual-review parity", check_changed_visual_review_parity)
    attempt("include-glob exclusions", check_include_glob_exclusions)
    run([sys.executable, "scripts/skills/self_test_manage_skill_links.py"])
    run([sys.executable, "scripts/skills/manage_global_policy_self_test.py"])
    run([sys.executable, "scripts/skills/merge_findings_self_test.py"])
    run([sys.executable, "scripts/skills/self_test_public_artifact_guard.py"])
    run([sys.executable, "scripts/skills/public_artifact_guard.py", "--repo", str(ROOT)])
    for skill in SKILLS:
        run([sys.executable, str(skill.relative_to(ROOT) / "scripts" / "self_test.py")])

    run(
        [
            sys.executable,
            "-m",
            "compileall",
            "scripts",
            "full_repo_harness",
            *[f"skills/{name}/scripts" for name in SKILL_NAMES],
        ]
    )

    if FAILURES:
        print_failure_summary()
        return 1

    print(
        f"validation ok ({len(SKILL_NAMES)} canonical linked skills; one shared harness)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
