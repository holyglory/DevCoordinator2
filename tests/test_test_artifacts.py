from __future__ import annotations

import base64
import hashlib
import json
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from devcoordinator2.check_evidence import source_digest
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.handlers import build_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.repoconfig import load_test_spec
from devcoordinator2.daemon.server import Caller
from devcoordinator2.daemon.test_artifacts import TestArtifactService as _TestArtifactService
from devcoordinator2.daemon.tests_lifecycle import TestLifecycle as _TestLifecycle
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

RUN_ID = "t20260903T120718Z-57067c"
CHECK = "browser"


def caller(identity="owner@example.test") -> Caller:
    return Caller(
        pid=os.getpid(), uid=os.getuid(), gid=os.getgid(),
        client_kind="edge" if identity else "codex", client_session=None,
        identity=identity,
    )


def tree_digest(entries: list[dict]) -> str:
    digest = hashlib.sha256(b"devcoordinator2-retained-artifact-tree-v1\0")
    for entry in entries:
        digest.update(entry["path"].encode())
        digest.update(b"\0")
        digest.update(str(entry["size"]).encode())
        digest.update(b"\0")
        digest.update(entry["sha256"].encode())
        digest.update(b"\0")
    return digest.hexdigest()


@pytest.fixture
def world(tmp_path: Path):
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock", state_dir=tmp_path / "state",
        unit_prefix="dc2-test", slice_name="dc2-tests.slice", client_group="",
    )
    db = Database(config.database_path)
    registry = Registry(db)
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    registration = registry.register(repo, os.getuid(), os.getgid())
    evidence = (
        repo / ".devcoordinator" / "test" / "logs" / "runs" / RUN_ID
        / "checks" / CHECK / "check" / "evidence"
    )
    retained = evidence / "retained" / "production"
    (retained / "nested").mkdir(parents=True)
    files = {
        "report.json": b'{"ok":true}\n',
        "nested/screenshot.png": b"fixture-png-bytes",
    }
    entries = []
    for relative, payload in files.items():
        target = retained / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(payload)
        entries.append({
            "path": relative,
            "size": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        })
    entries.sort(key=lambda item: item["path"])
    artifact = {
        "name": "production",
        "size": sum(item["size"] for item in entries),
        "files": len(entries),
        "sha256": tree_digest(entries),
        "entries": entries,
    }
    manifest = {
        "schema": 1,
        "kind": "devcoordinator2-retained-artifact-trees",
        "run_id": RUN_ID,
        "test": "browser-release",
        "check": CHECK,
        "requested_tier": "release",
        "readiness_eligible": True,
        "proof": "complete",
        "source_sha256": "a" * 64,
        "config_sha256": "b" * 64,
        "artifacts": [artifact],
    }
    (evidence / "retained-artifacts.json").write_text(
        json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8")
    run_metadata = {
        "schema": 2,
        "run_id": RUN_ID,
        "test": "browser-release",
        "started_at_epoch_ms": 100,
        "finished_at_epoch_ms": 200,
        "status": "passed",
        "complete": True,
    }
    run_root = repo / ".devcoordinator" / "test" / "logs" / "runs" / RUN_ID
    (run_root / "run.json").write_text(
        json.dumps(run_metadata, separators=(",", ":")) + "\n", encoding="utf-8")
    service = _TestArtifactService(db, registry)
    yield SimpleNamespace(
        config=config, db=db, registry=registry, repo=repo,
        registration=registration, service=service, evidence=evidence,
        artifact=artifact, files=files,
    )
    db.close()


def test_catalog_and_file_chunks_are_verified_and_path_free(world):
    root = world.service.catalog(
        world.repo, {"run_id": RUN_ID, "check": CHECK}, caller())
    assert root["artifacts"] == [{
        key: world.artifact[key] for key in ("name", "size", "files", "sha256")
    }]
    assert root["source_sha256"] == "a" * 64
    assert root["readiness_eligible"] is True
    assert root["run_status"] == "passed" and root["run_complete"] is True
    encoded = json.dumps(root)
    assert str(world.repo) not in encoded
    page = world.service.catalog(world.repo, {
        "run_id": RUN_ID,
        "check": CHECK,
        "artifact": "production",
        "manifest_sha256": root["manifest_sha256"],
        "offset": 0,
        "limit": 1,
    }, caller())
    assert len(page["entries"]) == 1 and page["next_offset"] == 1
    second = world.service.catalog(world.repo, {
        "run_id": RUN_ID,
        "check": CHECK,
        "artifact": "production",
        "manifest_sha256": root["manifest_sha256"],
        "offset": 1,
        "limit": 1,
    }, caller())
    assert len(second["entries"]) == 1 and second["next_offset"] is None

    entry = next(item for item in world.artifact["entries"]
                 if item["path"] == "nested/screenshot.png")
    chunk = world.service.file(world.repo, {
        "run_id": RUN_ID,
        "check": CHECK,
        "artifact": "production",
        "file": entry["path"],
        "manifest_sha256": root["manifest_sha256"],
        "offset": 0,
        "max_bytes": 7,
    }, caller())
    assert base64.b64decode(chunk["base64"]) == world.files[entry["path"]][:7]
    assert chunk["next_offset"] == 7


def test_tampered_or_aliased_retained_file_is_rejected(world):
    root = world.service.catalog(
        world.repo, {"run_id": RUN_ID, "check": CHECK}, caller())
    target = world.evidence / "retained" / "production" / "report.json"
    target.write_bytes(b"same-size-bad\n")
    with pytest.raises(ProtocolError) as caught:
        world.service.catalog(world.repo, {
            "run_id": RUN_ID,
            "check": CHECK,
            "artifact": "production",
            "manifest_sha256": root["manifest_sha256"],
        }, caller())
    assert caught.value.code == "test_artifact_tampered"

    target.unlink()
    target.symlink_to("nested/screenshot.png")
    with pytest.raises(ProtocolError) as caught:
        world.service.file(world.repo, {
            "run_id": RUN_ID,
            "check": CHECK,
            "artifact": "production",
            "file": "report.json",
            "manifest_sha256": root["manifest_sha256"],
        }, caller())
    assert caught.value.code == "test_artifact_tampered"


def test_handlers_require_exact_retained_artifact_arguments(world):
    handlers = build_handlers(
        world.config, world.registry, test_artifacts=world.service)
    assert {"test.artifact.catalog", "test.artifact.file"} <= set(handlers)
    with pytest.raises(ProtocolError, match="required"):
        handlers["test.artifact.catalog"](
            {"run_id": RUN_ID, "check": CHECK}, caller())
    with pytest.raises(ProtocolError, match="unknown"):
        handlers["test.artifact.catalog"]({
            "path": str(world.repo), "run_id": RUN_ID, "check": CHECK,
            "extra": True,
        }, caller())


def test_release_executor_snapshot_is_readable_through_daemon_contract(tmp_path: Path):
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock", state_dir=tmp_path / "state",
        unit_prefix="dc2-test", slice_name="dc2-tests.slice", client_group="",
    )
    db = Database(config.database_path)
    registry = Registry(db)
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    (repo / "README.md").write_text("fixture\n")
    (repo / ".gitignore").write_text("browser-output/\n")
    script = (
        "from pathlib import Path; "
        "p=Path('browser-output/production'); p.mkdir(parents=True); "
        "(p/'interaction-evidence.json').write_text('production'); "
        "d=Path('browser-output/developer-test'); d.mkdir(parents=True); "
        "(d/'raw-observations.json').write_text('developer')"
    )
    (repo / ".devcoordinator.toml").write_text(f'''
schema = 2
[test.browser]
[[test.browser.check]]
name = "main"
tier = "release"
command = ["python3", "-c", {json.dumps(script)}]
retained_artifacts = [
  {{ name = "production", path = "browser-output/production", max_bytes = 1024 }},
  {{ name = "developer-test", path = "browser-output/developer-test", max_bytes = 1024 }},
]
''')
    subprocess.run(["git", "add", "."], cwd=repo, check=True)
    registry.register(repo, os.getuid(), os.getgid())
    run_id = "t20260903T130000Z-abcdef"
    current = repo / ".devcoordinator/test/current"
    log_dir = repo / ".devcoordinator/test/logs/runs" / run_id
    current.mkdir(parents=True)
    log_dir.mkdir(parents=True)
    spec = load_test_spec(repo, "browser")
    plan = _TestLifecycle._build_plan(
        spec, spec.checks, spec.checks, run_id, repo, current, log_dir,
        source_digest(repo), (), None, None, "release")
    plan_path = current / "check-plan.json"
    plan_path.write_text(json.dumps(plan))
    executor = Path(__file__).resolve().parents[1] / "target/release/devcoordinator2-executor"
    completed = subprocess.run(
        [str(executor), "run-local", str(plan_path)],
        cwd=repo, capture_output=True, text=True, check=False, timeout=30)
    assert completed.returncode == 0, (completed.stdout, completed.stderr)
    report = json.loads((current / "check-report.json").read_text())
    assert [item["name"] for item in report["checks"][0]["retained_artifacts"]] == [
        "production", "developer-test"]

    service = _TestArtifactService(db, registry)
    catalog = service.catalog(
        repo, {"run_id": run_id, "check": "main", "artifact": "production"},
        caller(identity=None))
    assert catalog["run_status"] == "passed"
    assert catalog["readiness_eligible"] is True
    assert catalog["entries"] == [{
        "path": "interaction-evidence.json",
        "size": 10,
        "sha256": hashlib.sha256(b"production").hexdigest(),
    }]
    developer = service.catalog(
        repo, {"run_id": run_id, "check": "main", "artifact": "developer-test"},
        caller(identity=None))
    assert developer["entries"] == [{
        "path": "raw-observations.json",
        "size": 9,
        "sha256": hashlib.sha256(b"developer").hexdigest(),
    }]
    db.close()
