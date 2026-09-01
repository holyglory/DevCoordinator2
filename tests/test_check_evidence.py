from __future__ import annotations

import hashlib
import json
import os
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest

from devcoordinator2.check_evidence import (
    EXECUTOR_BINARY,
    EvidenceError,
    receipts_match,
    source_digest,
)
from devcoordinator2.daemon import tests_support
from devcoordinator2.daemon.repoconfig import CheckSpec, load_test_spec
from devcoordinator2.daemon.repoconfig import TestSpec as GovernedTestSpec
from devcoordinator2.daemon.tests_lifecycle import TestLifecycle as GovernedTestLifecycle
from devcoordinator2.protocol import ProtocolError


def repository(tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".gitignore").write_text(".devcoordinator/\nignored/\n")
    (repo / "tracked.txt").write_text("one\n")
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    subprocess.run(["git", "add", "."], cwd=repo, check=True)
    return repo


def receipt(path: Path, relative: str) -> dict:
    payload = path.read_bytes()
    return {
        "path": relative,
        "size": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def test_source_digest_tracks_source_but_not_ignored_runtime_state(tmp_path):
    repo = repository(tmp_path)
    first = source_digest(repo)
    ignored = repo / ".devcoordinator" / "test"
    ignored.mkdir(parents=True)
    (ignored / "report.json").write_text("runtime")
    assert source_digest(repo) == first
    (repo / "tracked.txt").write_text("two\n")
    second = source_digest(repo)
    assert second != first
    (repo / "untracked.txt").write_text("new\n")
    assert source_digest(repo) != second


def test_report_reader_accepts_only_strict_executor_schema_two(tmp_path):
    report = {
        "schema": 2,
        "run_id": "trun",
        "test": "complete",
        "requested_tier": "release",
        "readiness_eligible": True,
        "proof": "complete",
        "selection": [],
        "origin_run_id": None,
        "status": "passed",
        "source_digest": "a" * 64,
        "config_digest": "b" * 64,
        "unsafe_reason": None,
        "counts": dict.fromkeys(tests_support._CHECK_STATES, 0),
        "checks": [],
        "failure_index": [],
        "capacity": {
            "learned_capacity": 64,
            "effective_capacity": 48,
            "capacity_wait_count": 2,
        },
    }
    (tmp_path / tests_support.REPORT_FILE).write_text(json.dumps(report))
    dir_fd = os.open(tmp_path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        assert tests_support.read_check_report(dir_fd)["schema"] == 2
        report["schema"] = 1
        (tmp_path / tests_support.REPORT_FILE).write_text(json.dumps(report))
        assert tests_support.read_check_report(dir_fd) is None
    finally:
        os.close(dir_fd)


def test_python_backend_plan_is_accepted_by_release_rust_executor(tmp_path):
    repo = repository(tmp_path)
    (repo / ".devcoordinator.toml").write_text('''
schema = 2
[test.complete]
[[test.complete.check]]
name = "preflight"
tier = "development"
role = "preflight"
command = ["true"]
invalidates = ["unit"]
[[test.complete.check]]
name = "unit"
tier = "release"
command = ["true"]
''')
    spec = load_test_spec(repo, None)
    current = repo / ".devcoordinator" / "test" / "current"
    current.mkdir(parents=True)
    plan = GovernedTestLifecycle._build_plan(
        spec, spec.checks, spec.checks, "tcross-language", repo, current,
        source_digest(repo), (), None, None, "release")
    plan_path = current / "plan.json"
    plan_path.write_text(json.dumps(plan), encoding="utf-8")
    validated = subprocess.run(
        [str(EXECUTOR_BINARY), "validate", str(plan_path)],
        capture_output=True, text=True, timeout=30, check=False)
    assert validated.returncode == 0, validated.stderr
    assert json.loads(validated.stdout) == {
        "schema": 2, "valid": True, "test": "complete", "declared_checks": 2}


def test_artifact_receipts_are_exact_and_refuse_symlinks(tmp_path):
    repo = repository(tmp_path)
    artifact = repo / "build.bin"
    artifact.write_bytes(b"build")
    receipts = [receipt(artifact, "build.bin")]
    assert receipts_match(repo, receipts)
    artifact.write_bytes(b"changed")
    assert not receipts_match(repo, receipts)
    artifact.unlink()
    artifact.symlink_to(repo / "tracked.txt")
    with pytest.raises(EvidenceError, match="not a regular file"):
        receipts_match(repo, receipts)


def test_evidence_store_keeps_only_bounded_content_free_fields(tmp_path):
    repo = repository(tmp_path)
    (repo / ".devcoordinator" / "test").mkdir(parents=True)
    report = {
        "run_id": "trun", "test": "complete", "proof": "complete",
        "status": "failed", "source_digest": "a" * 64,
        "config_digest": "b" * 64, "selection": [],
        "requested_tier": "release", "readiness_eligible": True,
        "checks": [{
            "name": "unit", "status": "failed", "duration_seconds": 1.2,
            "exit_code": 1, "artifacts": [], "reason": "private detail",
            "command": ["secret-command"],
        }],
    }
    tests_support.record_evidence(repo, report, (os.getuid(), os.getgid()))
    stored = tests_support.find_evidence(repo, "trun")
    assert stored["checks"] == [{
        "name": "unit", "status": "failed", "duration_seconds": 1.2,
        "exit_code": 1, "artifacts": [],
        "stdout_bytes_observed": None, "stdout_bytes_retained": None,
        "stdout_truncated": None, "stderr_bytes_observed": None,
        "stderr_bytes_retained": None, "stderr_truncated": None,
    }]
    assert "private detail" not in (repo / ".devcoordinator/test/evidence.json").read_text()
    assert "secret-command" not in (repo / ".devcoordinator/test/evidence.json").read_text()


def test_retry_plan_reuses_only_matching_declared_artifacts(tmp_path):
    repo = repository(tmp_path)
    current = repo / ".devcoordinator" / "test" / "current"
    current.mkdir(parents=True)
    artifact = repo / "build.bin"
    artifact.write_bytes(b"build")
    receipts = [receipt(artifact, "build.bin")]
    build = CheckSpec(
        name="build", tier="development", role="work", command=("true",),
        discover=None, case_command=None, cases=(), cwd=repo, env={}, after=(),
        requires=(), completion="process", on_failure="continue",
        produces=("build.bin",), timeout_seconds=None, invalidates=())
    failed = CheckSpec(
        name="unit", tier="release", role="work", command=("false",),
        discover=None, case_command=None, cases=(), cwd=repo, env={}, after=(),
        requires=("build",), completion="process", on_failure="continue", produces=(),
        timeout_seconds=30, invalidates=())
    spec = GovernedTestSpec(
        name="complete", cwd=repo, timeout_seconds=600, env={},
        checks=(build, failed), config_digest="b" * 64)
    origin = {
        "run_id": "torigin", "test": "complete", "proof": "complete",
        "status": "failed", "selection": [], "source_digest": "a" * 64,
        "config_digest": "b" * 64,
        "requested_tier": "release", "readiness_eligible": True,
        "checks": [
            {"name": "build", "status": "passed", "artifacts": receipts},
            {"name": "unit", "status": "failed", "artifacts": []},
        ],
    }
    GovernedTestLifecycle._validate_retry(
        origin, "torigin", "unit", spec, "a" * 64, (build, failed))
    plan = GovernedTestLifecycle._build_plan(
        spec, (build, failed), (build, failed), "tretry", repo, current,
        "a" * 64, ("unit",), origin, "torigin", "release")
    assert plan["schema"] == 2
    assert plan["requested_tier"] == "release"
    assert plan["readiness_eligible"] is False
    assert plan["checks"][1]["timeout_seconds"] == 30
    assert plan["checks"][1]["cwd"] == "."
    assert plan["proof"] == "retry"
    assert plan["reused"] == {"build": receipts}
    artifact.write_bytes(b"stale")
    stale = GovernedTestLifecycle._build_plan(
        spec, (build, failed), (build, failed), "tretry2", repo, current,
        "a" * 64, ("unit",), origin, "torigin", "release")
    assert stale["reused"] == {}


def test_retry_requires_a_failed_check_from_complete_matching_evidence(tmp_path):
    repo = repository(tmp_path)
    check = CheckSpec(
        name="unit", tier="release", role="work", command=("true",),
        discover=None, case_command=None, cases=(), cwd=repo, env={}, after=(),
        requires=(), completion="process", on_failure="continue", produces=(),
        timeout_seconds=None, invalidates=())
    spec = GovernedTestSpec(
        name="complete", cwd=repo, timeout_seconds=600, env={},
        checks=(check,), config_digest="b" * 64)
    origin = {
        "run_id": "torigin", "test": "complete", "proof": "selected",
        "status": "failed", "selection": ["unit"], "source_digest": "a" * 64,
        "config_digest": "b" * 64,
        "requested_tier": "release", "readiness_eligible": False,
        "checks": [{"name": "unit", "status": "failed", "artifacts": []}],
    }
    with pytest.raises(ProtocolError, match="original complete run"):
        GovernedTestLifecycle._validate_retry(
            origin, "torigin", "unit", spec, "a" * 64, (check,))


def test_requested_tier_is_monotonic_and_selection_cannot_escape_it(tmp_path):
    repo = repository(tmp_path)
    development = CheckSpec(
        name="unit", tier="development", role="work", command=("true",),
        discover=None, case_command=None, cases=(), cwd=repo, env={}, after=(),
        requires=(), completion="process", on_failure="continue", produces=(),
        timeout_seconds=None, invalidates=())
    premerge = replace(
        development, name="browser", tier="pre-merge", requires=("unit",))
    release = replace(
        development, name="release", tier="release", requires=("browser",))
    configured = (development, premerge, release)
    assert [row.name for row in GovernedTestLifecycle._selected_closure(
        configured, (), "development")] == ["unit"]
    assert [row.name for row in GovernedTestLifecycle._selected_closure(
        configured, (), "pre-merge")] == ["unit", "browser"]
    assert [row.name for row in GovernedTestLifecycle._selected_closure(
        configured, (), "release")] == ["unit", "browser", "release"]
    with pytest.raises(ProtocolError, match="outside requested development"):
        GovernedTestLifecycle._selected_closure(
            configured, ("release",), "development")
