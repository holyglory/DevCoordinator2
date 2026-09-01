from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from devcoordinator2.check_evidence import (
    EvidenceError,
    artifact_receipts,
    receipts_match,
    source_digest,
)
from devcoordinator2.daemon import tests_support
from devcoordinator2.daemon.repoconfig import CheckSpec
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


def test_artifact_receipts_are_exact_and_refuse_symlinks(tmp_path):
    repo = repository(tmp_path)
    artifact = repo / "build.bin"
    artifact.write_bytes(b"build")
    receipts = artifact_receipts(repo, ["build.bin"])
    assert receipts_match(repo, receipts)
    artifact.write_bytes(b"changed")
    assert not receipts_match(repo, receipts)
    artifact.unlink()
    artifact.symlink_to(repo / "tracked.txt")
    with pytest.raises(EvidenceError, match="unavailable"):
        artifact_receipts(repo, ["build.bin"])


def test_evidence_store_keeps_only_bounded_content_free_fields(tmp_path):
    repo = repository(tmp_path)
    (repo / ".devcoordinator" / "test").mkdir(parents=True)
    report = {
        "run_id": "trun", "test": "complete", "proof": "complete",
        "status": "failed", "source_digest": "a" * 64,
        "config_digest": "b" * 64, "selection": [],
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
    receipts = artifact_receipts(repo, ["build.bin"])
    build = CheckSpec(
        name="build", command=("true",), cwd=repo, env={}, after=(), requires=(),
        completion="process", on_failure="continue", produces=("build.bin",))
    failed = CheckSpec(
        name="unit", command=("false",), cwd=repo, env={}, after=(),
        requires=("build",), completion="process", on_failure="continue", produces=())
    spec = GovernedTestSpec(
        name="complete", command=None, cwd=repo, timeout_seconds=600, env={},
        checks=(build, failed), config_digest="b" * 64)
    origin = {
        "run_id": "torigin", "test": "complete", "proof": "complete",
        "status": "failed", "selection": [], "source_digest": "a" * 64,
        "config_digest": "b" * 64,
        "checks": [
            {"name": "build", "status": "passed", "artifacts": receipts},
            {"name": "unit", "status": "failed", "artifacts": []},
        ],
    }
    GovernedTestLifecycle._validate_retry(
        origin, "torigin", "unit", spec, "a" * 64, (build, failed))
    plan = GovernedTestLifecycle._build_plan(
        spec, (build, failed), (build, failed), "tretry", repo, current,
        "a" * 64, ("unit",), origin, "torigin")
    assert plan["proof"] == "diagnostic"
    assert plan["reused"] == {"build": receipts}
    artifact.write_bytes(b"stale")
    stale = GovernedTestLifecycle._build_plan(
        spec, (build, failed), (build, failed), "tretry2", repo, current,
        "a" * 64, ("unit",), origin, "torigin")
    assert stale["reused"] == {}


def test_retry_requires_a_failed_check_from_complete_matching_evidence(tmp_path):
    repo = repository(tmp_path)
    check = CheckSpec(
        name="unit", command=("true",), cwd=repo, env={}, after=(), requires=(),
        completion="process", on_failure="continue", produces=())
    spec = GovernedTestSpec(
        name="complete", command=None, cwd=repo, timeout_seconds=600, env={},
        checks=(check,), config_digest="b" * 64)
    origin = {
        "run_id": "torigin", "test": "complete", "proof": "diagnostic",
        "status": "failed", "selection": ["unit"], "source_digest": "a" * 64,
        "config_digest": "b" * 64,
        "checks": [{"name": "unit", "status": "failed", "artifacts": []}],
    }
    with pytest.raises(ProtocolError, match="original complete run"):
        GovernedTestLifecycle._validate_retry(
            origin, "torigin", "unit", spec, "a" * 64, (check,))
