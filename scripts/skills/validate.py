#!/usr/bin/env python3
"""Declare and run complete six-skill validation through the Rust executor."""

from __future__ import annotations

import hashlib
import json
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HARNESS = ROOT / "full_repo_harness"
EXECUTOR = ROOT / "target" / "release" / "devcoordinator2-executor"
RUN_ROOT = ROOT / ".devcoordinator" / "agent-validation"
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
        raise SystemExit(
            f"Canonical skill set mismatch; missing={missing}, unexpected={unexpected}"
        )
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


def _run_internal(args: list[str], *, cwd: Path = ROOT) -> None:
    """Run one dependency inside a named executor-owned assertion leaf."""

    environment = dict(os.environ)
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    completed = subprocess.run(args, cwd=cwd, env=environment, check=False)
    if completed.returncode != 0:
        raise SystemExit(f"internal command exited {completed.returncode}: {args[0]}")


def check_include_glob_exclusions() -> None:
    temporary = Path(tempfile.mkdtemp(prefix="include-glob-exclusion-"))
    try:
        repository = temporary / "repo"
        (repository / "src").mkdir(parents=True)
        (repository / "node_modules" / "pkg").mkdir(parents=True)
        (repository / "src" / "app.py").write_text("print(1)\n", encoding="utf-8")
        (repository / "node_modules" / "pkg" / "index.py").write_text(
            "print(2)\n", encoding="utf-8"
        )
        identity = [
            "-c",
            "user.name=agent-skills-validate",
            "-c",
            "user.email=validate@example.invalid",
        ]
        _run_internal(["git", "init", "-q"], cwd=repository)
        _run_internal(["git", "add", "src/app.py"], cwd=repository)
        _run_internal(["git", *identity, "commit", "-q", "-m", "init"], cwd=repository)

        broad = temporary / "broad"
        _run_internal(
            [
                sys.executable,
                "skills/full-repo-audit/scripts/build_audit_batches.py",
                "--repo",
                str(repository),
                "--out",
                str(broad),
                "--include-glob",
                "**/*.py",
            ]
        )
        broad_manifest = json.loads((broad / "manifest.json").read_text(encoding="utf-8"))
        broad_files = {item["rel_path"] for item in broad_manifest["source_files"]}
        if "node_modules/pkg/index.py" in broad_files:
            raise SystemExit("Broad --include-glob unexpectedly included node_modules")

        explicit = temporary / "explicit"
        _run_internal(
            [
                sys.executable,
                "skills/full-repo-audit/scripts/build_audit_batches.py",
                "--repo",
                str(repository),
                "--out",
                str(explicit),
                "--include-glob",
                "node_modules/**/*.py",
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
                    f"{verifier} redefines INTERACTION_CHECKLIST_LABELS; "
                    "import the shared constant"
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
            raise SystemExit(
                f"Changed visual-review contract drift in {path}: missing={missing}"
            )


INTERNAL_CHECKS = {
    "repository-layout": check_repository_layout,
    "canonical-harness": check_canonical_harness_ownership,
    "interaction-parity": check_interaction_label_parity,
    "visual-review-parity": check_changed_visual_review_parity,
    "include-glob-exclusions": check_include_glob_exclusions,
}


def _plan_check(
    name: str,
    command: list[str],
    *,
    tier: str,
    role: str,
    requires: list[str] | None = None,
    invalidates: list[str] | None = None,
    env: dict[str, str] | None = None,
) -> dict:
    return {
        "name": name,
        "tier": tier,
        "role": role,
        "after": [],
        "requires": list(requires or []),
        "invalidates": list(invalidates or []),
        "cwd": ".",
        "env": dict(env or {"PYTHONDONTWRITEBYTECODE": "1"}),
        "timeout_seconds": 900,
        "completion": "process",
        "on_failure": "continue",
        "produces": [],
        "command": command,
    }


def validation_checks(*, pycache_root: Path) -> list[dict]:
    """Return the complete, dependency-explicit six-skill validation graph."""

    python = sys.executable
    internal = lambda name: [  # noqa: E731 - compact declarative command factory
        python,
        "scripts/skills/validate.py",
        "--internal-check",
        name,
    ]
    preflight_commands = (
        ("repository-layout", internal("repository-layout")),
        ("validator-self-test", [python, "scripts/skills/validate_self_test.py"]),
        ("policy-self-test", [python, "scripts/skills/check_app_wide_policy_self_test.py"]),
        ("policy", [python, "scripts/skills/check_app_wide_policy.py"]),
        (
            "neutrality-self-test",
            [python, "scripts/skills/check_agent_neutrality_self_test.py"],
        ),
        ("neutrality", [python, "scripts/skills/check_agent_neutrality.py"]),
        ("ledger-self-test", [python, "scripts/skills/check_user_issue_ledgers_self_test.py"]),
        ("ledgers", [python, "scripts/skills/check_user_issue_ledgers.py"]),
        (
            "freshness-self-test",
            [python, "scripts/skills/check_repository_freshness_self_test.py"],
        ),
        (
            "boundaries-self-test",
            [python, "scripts/skills/check_repository_boundaries_self_test.py"],
        ),
        (
            "boundaries",
            [python, "scripts/skills/check_repository_boundaries.py", "--repo", str(ROOT)],
        ),
        ("ci-security-self-test", [python, "scripts/skills/check_ci_security_self_test.py"]),
        ("ci-security", [python, "scripts/skills/check_ci_security.py"]),
        ("canonical-harness", internal("canonical-harness")),
        (
            "public-artifact-self-test",
            [python, "scripts/skills/self_test_public_artifact_guard.py"],
        ),
        (
            "public-artifacts",
            [python, "scripts/skills/public_artifact_guard.py", "--repo", str(ROOT)],
        ),
    )
    target_commands = [
        ("interaction-parity", internal("interaction-parity")),
        ("visual-review-parity", internal("visual-review-parity")),
        ("include-glob-exclusions", internal("include-glob-exclusions")),
        ("skill-link-manager", [python, "scripts/skills/self_test_manage_skill_links.py"]),
        ("global-policy-manager", [python, "scripts/skills/manage_global_policy_self_test.py"]),
        ("merge-findings", [python, "scripts/skills/merge_findings_self_test.py"]),
    ]
    target_commands.extend(
        (
            f"skill-{skill.name}",
            [python, str(skill.relative_to(ROOT) / "scripts" / "self_test.py")],
        )
        for skill in SKILLS
    )
    target_names = [name for name, _command in target_commands]
    target_names.append("python-compile")
    preflight_names = [name for name, _command in preflight_commands]
    checks = [
        _plan_check(
            name,
            command,
            tier="development",
            role="preflight",
            invalidates=target_names,
        )
        for name, command in preflight_commands
    ]
    checks.extend(
        _plan_check(
            name,
            command,
            tier="release",
            role="work",
            requires=preflight_names,
        )
        for name, command in target_commands
    )
    checks.append(
        _plan_check(
            "python-compile",
            [
                python,
                "-m",
                "compileall",
                "-q",
                "scripts",
                "full_repo_harness",
                *[f"skills/{name}/scripts" for name in SKILL_NAMES],
            ],
            tier="release",
            role="work",
            requires=preflight_names,
            env={"PYTHONPYCACHEPREFIX": str(pycache_root)},
        )
    )
    return checks


def build_validation_plan(
    *,
    run_id: str,
    current_dir: Path,
    source_digest: str,
) -> dict:
    checks = validation_checks(pycache_root=current_dir / "pycache")
    contract = json.dumps(checks, sort_keys=True, separators=(",", ":")).encode()
    return {
        "schema": 2,
        "run_id": run_id,
        "test": "agent-skills",
        "worktree_root": str(ROOT),
        "current_dir": str(current_dir),
        "requested_tier": "release",
        "readiness_eligible": True,
        "proof": "complete",
        "selection": [],
        "origin_run_id": None,
        "source_digest": source_digest,
        "config_digest": hashlib.sha256(contract).hexdigest(),
        "reused": {},
        "checks": checks,
    }


def executor_argv(executor: Path, plan_path: Path) -> list[str]:
    """The only supported top-level scheduling surface for this validator."""

    return [str(executor), "run-local", str(plan_path)]


def _source_digest(executor: Path) -> str:
    completed = subprocess.run(
        [str(executor), "source-digest", "--worktree", str(ROOT)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "Rust source digest failed: " + (completed.stderr.strip()[-1000:] or "no detail")
        )
    try:
        receipt = json.loads(completed.stdout)
        digest = receipt["sha256"]
    except (json.JSONDecodeError, KeyError, TypeError) as error:
        raise RuntimeError("Rust source digest returned an invalid receipt") from error
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or any(character not in "0123456789abcdef" for character in digest)
    ):
        raise RuntimeError("Rust source digest returned an invalid sha256")
    return digest


def _write_plan(path: Path, plan: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=False)
    payload = (json.dumps(plan, sort_keys=True, separators=(",", ":")) + "\n").encode()
    temporary = path.with_name(f".{path.name}-{os.getpid()}")
    try:
        with temporary.open("xb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _read_report(path: Path) -> dict:
    try:
        details = path.stat()
        if not path.is_file() or details.st_size > 2 * 1024 * 1024:
            raise RuntimeError("Rust validation report is unavailable or oversized")
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError("Rust validation report is unavailable or invalid") from error
    if not isinstance(report, dict) or report.get("schema") != 2:
        raise RuntimeError("Rust validation report is not schema 2")
    return report


def _failure_diagnostics(failures: list, current_dir: Path) -> list[dict]:
    diagnostics = []
    for failure in failures:
        if len(diagnostics) >= 5 or not isinstance(failure, dict):
            break
        if failure.get("status") not in {"failed", "timed_out", "unsafe"}:
            continue
        check = failure.get("check")
        output_ref = failure.get("output_ref")
        if not isinstance(check, str) or output_ref != f"checks/{check}":
            continue
        log_path = current_dir / "checks" / check / "stderr.log"
        try:
            details = log_path.lstat()
            if log_path.is_symlink() or not log_path.is_file():
                continue
            with log_path.open("rb") as handle:
                handle.seek(max(0, details.st_size - 2048))
                tail = handle.read(2048).decode("utf-8", errors="replace")
        except OSError:
            continue
        tail = "".join(
            character if character in "\n\t" or character >= " " else "�" for character in tail
        ).strip()
        if tail:
            diagnostics.append({"check": check, "stderr_tail": tail})
    return diagnostics


def bounded_receipt(
    report: dict,
    report_path: Path,
    *,
    include_diagnostics: bool = False,
) -> dict:
    failures = report.get("failure_index")
    failures = failures if isinstance(failures, list) else []
    retained = failures[:20]
    receipt = {
        "schema": 2,
        "status": report.get("status"),
        "checks": len(report.get("checks", []))
        if isinstance(report.get("checks"), list)
        else 0,
        "counts": report.get("counts", {}),
        "failure_index": retained,
        "failure_index_truncated": bool(report.get("failure_index_truncated"))
        or len(failures) > len(retained),
        "report": str(report_path),
    }
    if include_diagnostics:
        receipt["failure_diagnostics"] = _failure_diagnostics(retained, report_path.parent)
    return receipt


def run_complete_validation(executor: Path = EXECUTOR) -> int:
    try:
        executor.lstat()
    except FileNotFoundError:
        print(
            "Rust executor is missing; run `cargo build --locked --release "
            "--package devcoordinator2-executor` first.",
            file=sys.stderr,
        )
        return 2
    if executor.is_symlink() or not executor.is_file() or not os.access(executor, os.X_OK):
        print("Rust executor must be a regular executable release binary.", file=sys.stderr)
        return 2
    timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    run_id = f"skills-{timestamp}-{os.getpid()}-{secrets.token_hex(3)}"
    current_dir = RUN_ROOT / run_id
    try:
        digest = _source_digest(executor)
        plan = build_validation_plan(
            run_id=run_id,
            current_dir=current_dir,
            source_digest=digest,
        )
        plan_path = current_dir / "validation-plan.json"
        _write_plan(plan_path, plan)
        completed = subprocess.run(
            executor_argv(executor, plan_path),
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        report_path = current_dir / "check-report.json"
        if not report_path.is_file():
            detail = completed.stderr.strip()[-1000:]
            raise RuntimeError(
                "Rust executor did not publish check-report.json: "
                + (detail or "no diagnostic")
            )
        report = _read_report(report_path)
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        return 2
    receipt = bounded_receipt(
        report,
        report_path,
        include_diagnostics=os.environ.get("GITHUB_ACTIONS") == "true",
    )
    print(json.dumps(receipt, separators=(",", ":")))
    expected = 0 if report.get("status") == "passed" else 1
    if completed.returncode != expected:
        detail = completed.stderr.strip()[-1000:]
        print(
            f"Rust executor exit/report mismatch ({completed.returncode} != {expected}): "
            f"{detail}",
            file=sys.stderr,
        )
        return 2
    return expected


def run_internal_check(name: str) -> int:
    operation = INTERNAL_CHECKS.get(name)
    if operation is None:
        print(f"unknown internal validation check: {name}", file=sys.stderr)
        return 2
    try:
        operation()
    except SystemExit as error:
        print(str(error) or f"internal validation check exited {error.code}", file=sys.stderr)
        return 1
    except Exception as error:
        print(f"{type(error).__name__}: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"schema": 2, "check": name, "status": "passed"}))
    return 0


def main(argv: list[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments:
        if len(arguments) == 2 and arguments[0] == "--internal-check":
            return run_internal_check(arguments[1])
        print("usage: validate.py [--internal-check NAME]", file=sys.stderr)
        return 2
    return run_complete_validation()


if __name__ == "__main__":
    raise SystemExit(main())
