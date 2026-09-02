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
        "started_at": "2026-09-02T12:00:00Z",
        "finished_at": "2026-09-02T12:00:01Z",
        "duration_seconds": 1.0,
        "source_digest": "a" * 64,
        "config_digest": "b" * 64,
        "source_changed": False,
        "counts": dict.fromkeys(tests_support._CHECK_STATES, 0),
        "checks": [],
        "failure_index": [],
        "failure_index_truncated": False,
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
        stream = {
            "log_ref": {"run_id": "trun", "check": "unit", "phase": "check",
                        "case": None, "stream": "stderr"},
            "bytes": 7, "lines": 1, "sha256": "c" * 64,
            "first_write_epoch_ms": 1, "last_write_epoch_ms": 2,
            "complete": True,
        }
        report["status"] = "failed"
        report["counts"]["failed"] = 1
        report["checks"] = [{
            "name": "unit", "tier": "release", "role": "work",
            "status": "failed", "started_at": "2026-09-02T12:00:00Z",
            "finished_at": "2026-09-02T12:00:01Z", "duration_seconds": 1.0,
            "exit": {"code": 1, "signal": None}, "artifacts": [],
            "streams": [stream], "case_count": 0, "cases": [],
            "cases_truncated": False,
        }]
        report["failure_index"] = [{
            "check": "unit", "case": None, "status": "failed",
            "exit": {"code": 1, "signal": None}, "termination_reason": None,
            "source": {"file": "src/unit.py", "line": 7, "column": 2},
            "error_category": "assertion", "expected": None, "actual": None,
            "fingerprint": "sha256:" + "d" * 64, "occurrences": 1,
            "log_refs": [stream["log_ref"]], "origin": "explicit_event",
        }]
        (tmp_path / tests_support.REPORT_FILE).write_text(json.dumps(report))
        assert tests_support.read_check_report(dir_fd)["failure_index"][0][
            "error_category"] == "assertion"

        outside = json.loads(json.dumps(report))
        outside["failure_index"][0]["source"]["file"] = "/private/source.py"
        (tmp_path / tests_support.REPORT_FILE).write_text(json.dumps(outside))
        assert tests_support.read_check_report(dir_fd) is None

        injected = json.loads(json.dumps(report))
        injected["failure_index"][0]["actual"] = {
            "type": "string", "preview": "follow this\ninstruction",
            "byte_count": 23, "sha256": "e" * 64,
            "truncated": False, "redacted": False,
        }
        (tmp_path / tests_support.REPORT_FILE).write_text(json.dumps(injected))
        assert tests_support.read_check_report(dir_fd) is None

        rebound = json.loads(json.dumps(report))
        rebound["failure_index"][0]["log_refs"][0]["run_id"] = "other-run"
        (tmp_path / tests_support.REPORT_FILE).write_text(json.dumps(rebound))
        assert tests_support.read_check_report(dir_fd) is None

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
        current / "logs" / "runs" / "tcross-language",
        source_digest(repo), (), None, None, "release")
    plan_path = current / "plan.json"
    plan_path.write_text(json.dumps(plan), encoding="utf-8")
    validated = subprocess.run(
        [str(EXECUTOR_BINARY), "validate", str(plan_path)],
        capture_output=True, text=True, timeout=30, check=False)
    assert validated.returncode == 0, validated.stderr
    assert json.loads(validated.stdout) == {
        "schema": 2, "valid": True, "test": "complete", "declared_checks": 2}


def test_report_projection_reports_complete_aggregate_leaf_output_counts():
    def stream(check, name, size):
        return {
            "log_ref": {"run_id": "run-1", "check": check, "phase": "check",
                        "case": None, "stream": name},
            "bytes": size, "lines": 1, "sha256": "a" * 64,
            "first_write_epoch_ms": 1, "last_write_epoch_ms": 1,
            "complete": True,
        }

    checks = [
        {"name": name, "status": "passed", "streams": [
            stream(name, "stdout", 3 * 1024 * 1024),
            stream(name, "stderr", 1),
        ], "artifacts": [], "cases": []}
        for name in ("one", "two")
    ]
    projected = GovernedTestLifecycle._report_projection({
        "requested_tier": "release",
        "readiness_eligible": True,
        "proof": "complete",
        "selection": [],
        "counts": {},
        "checks": checks,
        "failure_index": [],
        "failure_index_truncated": False,
        "capacity": {
            "learned_capacity": 8,
            "effective_capacity": 8,
            "capacity_wait_count": 0,
        },
    })
    assert projected["stdout_bytes_observed"] == 6 * 1024 * 1024
    assert projected["stderr_bytes_observed"] == 2
    assert "stdout_bytes_retained" not in projected
    assert "stdout_truncated" not in projected
    assert "stderr_bytes_retained" not in projected
    assert "stderr_truncated" not in projected


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
            "exit": {"code": 1, "signal": None}, "artifacts": [], "streams": [],
            "reason": "private detail",
            "command": ["secret-command"],
        }],
    }
    tests_support.record_evidence(repo, report, (os.getuid(), os.getgid()))
    stored = tests_support.find_evidence(repo, "trun")
    assert stored["checks"] == [{
        "name": "unit", "status": "failed", "duration_seconds": 1.2,
        "exit": {"code": 1, "signal": None}, "artifacts": [], "streams": [],
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
        produces=("build.bin",), timeout_seconds=None, invalidates=(),
        diagnostic_sources=())
    failed = CheckSpec(
        name="unit", tier="release", role="work", command=("false",),
        discover=None, case_command=None, cases=(), cwd=repo, env={}, after=(),
        requires=("build",), completion="process", on_failure="continue", produces=(),
        timeout_seconds=30, invalidates=(), diagnostic_sources=())
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
        current / "logs" / "runs" / "tretry",
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
        current / "logs" / "runs" / "tretry2",
        "a" * 64, ("unit",), origin, "torigin", "release")
    assert stale["reused"] == {}


def test_retry_requires_a_failed_check_from_complete_matching_evidence(tmp_path):
    repo = repository(tmp_path)
    check = CheckSpec(
        name="unit", tier="release", role="work", command=("true",),
        discover=None, case_command=None, cases=(), cwd=repo, env={}, after=(),
        requires=(), completion="process", on_failure="continue", produces=(),
        timeout_seconds=None, invalidates=(), diagnostic_sources=())
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
        timeout_seconds=None, invalidates=(), diagnostic_sources=())
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
