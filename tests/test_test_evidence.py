from __future__ import annotations

import base64
import hashlib
import json
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.handlers import build_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller
from devcoordinator2.daemon.test_evidence import TestEvidenceService as _TestEvidenceService
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import MAX_RESPONSE_BYTES, ProtocolError, success_response

PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk"
    "/x8AAusB9Y9Z4rUAAAAASUVORK5CYII="
)
RUN_ID = "t20260902T010203Z-abcdef"


def caller(identity="owner@example.test") -> Caller:
    return Caller(
        pid=os.getpid(),
        uid=os.getuid(),
        gid=os.getgid(),
        client_kind="edge" if identity else "codex",
        client_session=None,
        identity=identity,
    )


@pytest.fixture
def world(tmp_path: Path):
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock",
        state_dir=tmp_path / "state",
        unit_prefix="dc2-test",
        slice_name="dc2-tests.slice",
        client_group="",
    )
    db = Database(config.database_path)
    registry = Registry(db)
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    registration = registry.register(repo, os.getuid(), os.getgid())
    service = _TestEvidenceService(db, registry)
    evidence = (
        repo
        / ".devcoordinator"
        / "test"
        / "logs"
        / "runs"
        / RUN_ID
        / "checks"
        / "formal-ui"
        / "check"
        / "evidence"
    )
    screenshots = evidence / "screenshots"
    screenshots.mkdir(parents=True)
    screenshot = screenshots / "cell-1-desktop-viewport.png"
    screenshot.write_bytes(PNG)
    digest = hashlib.sha256(PNG).hexdigest()
    manifest = {
        "schemaVersion": 1,
        "kind": "formal-web-ui-journey-evidence",
        "runId": "formal-web-ui-example",
        "governedRunId": RUN_ID,
        "governedCheck": "formal-ui",
        "generatedAt": "2026-09-02T01:03:00.000Z",
        "browser": "playwright-managed-browser",
        "coverage": {
            "checkedPages": 1,
            "plannedPages": 1,
            "failed": False,
            "readinessEligible": True,
        },
        "cells": [
            {
                "cellId": "cell-1",
                "reviewCellKey": "a" * 64,
                "planIndex": 0,
                "targetName": "Sign in [invalid password]",
                "primaryJourney": "sign-in",
                "stateName": "invalid-password",
                "requestedPath": "/sign-in",
                "finalPath": "/sign-in",
                "viewport": {
                    "name": "desktop",
                    "width": 1440,
                    "height": 900,
                    "sampling": {"mode": "sampled-only"},
                },
                "startedAt": "2026-09-02T01:02:58.000Z",
                "endedAt": "2026-09-02T01:03:00.000Z",
                "durationMs": 2000,
                "outcome": "checked",
                "httpStatus": 200,
                "sourceBindingStatus": "matched",
                "review": {"status": "review-required", "decision": None},
                "actions": [
                    {
                        "index": 0,
                        "action": "fill",
                        "outcome": "completed",
                        "durationMs": 12,
                    }
                ],
                "findings": [{"severity": "warning", "rule": "tiny-interactive-target"}],
                "screenshots": {
                    "viewport": {
                        "kind": "viewport",
                        "path": f"screenshots/{screenshot.name}",
                        "mime": "image/png",
                        "size": len(PNG),
                        "sha256": digest,
                        "width": 1,
                        "height": 1,
                        "capturedAt": "2026-09-02T01:03:00.000Z",
                    },
                    "fullPage": None,
                },
            }
        ],
    }
    (evidence / "journey-evidence.json").write_text(json.dumps(manifest), encoding="utf-8")
    yield SimpleNamespace(
        config=config,
        db=db,
        registry=registry,
        repo=repo,
        registration=registration,
        service=service,
        evidence=evidence,
        screenshot=screenshot,
        manifest=manifest,
    )
    db.close()


def test_schema15_visual_feedback_tables_exist(world):
    tables = {
        row["name"]
        for row in world.db.query("SELECT name FROM sqlite_master WHERE type='table'")
    }
    assert {"visual_feedback", "visual_feedback_comments", "visual_feedback_events"} <= tables
    assert (
        world.db.query("SELECT value FROM meta WHERE key='schema_version'")[0]["value"] == "15"
    )


def test_evidence_metadata_is_path_free_and_image_chunks_verify_integrity(world):
    result = world.service.get(world.repo, RUN_ID, caller())
    assert result["status"] == "available"
    assert result["image_count"] == 1
    assert result["issues"] == []
    cell = result["bundles"][0]["cells"][0]
    image = cell["screenshots"]["viewport"]
    assert image["status"] == "available"
    assert str(world.repo) not in json.dumps(result)
    chunk = world.service.image(
        world.repo,
        {
            "run_id": RUN_ID,
            "image_id": image["image_id"],
            "offset": 0,
            "max_bytes": 184320,
        },
        caller(),
    )
    assert base64.b64decode(chunk["base64"]) == PNG
    assert chunk["next_offset"] is None

    largest = {
        **chunk,
        "bytes": 184_320,
        "base64": base64.b64encode(b"x" * 184_320).decode("ascii"),
    }
    encoded = success_response("receipt", largest)
    assert len(encoded) < MAX_RESPONSE_BYTES and b"response exceeded" not in encoded

    world.screenshot.write_bytes(PNG[:-1] + b"x")
    with pytest.raises(ProtocolError) as caught:
        world.service.image(
            world.repo,
            {
                "run_id": RUN_ID,
                "image_id": image["image_id"],
            },
            caller(),
        )
    assert caught.value.code == "test_evidence_tampered"
    with pytest.raises(ProtocolError) as caught:
        world.service.create_feedback(
            world.repo,
            {
                "run_id": RUN_ID,
                "image_id": image["image_id"],
                "body": "This must not attach to changed evidence.",
                "marks": [
                    {
                        "id": "mark-1",
                        "type": "pin",
                        "color": "#ef4444",
                        "x": 0.5,
                        "y": 0.5,
                    }
                ],
            },
            caller(),
        )
    assert caught.value.code == "test_evidence_tampered"
    assert not world.db.query("SELECT 1 FROM tasks")


def test_multiple_formal_bundles_share_one_governed_check_without_overwrite(world):
    bundle = world.evidence / "formal-runs" / ("b" * 64)
    screenshots = bundle / "screenshots"
    screenshots.mkdir(parents=True)
    screenshot = screenshots / "second-viewport.png"
    screenshot.write_bytes(PNG)
    manifest = json.loads(json.dumps(world.manifest))
    manifest["runId"] = "formal-web-ui-second"
    manifest["cells"][0]["cellId"] = "cell-2"
    manifest["cells"][0]["targetName"] = "Account [base]"
    manifest["cells"][0]["screenshots"]["viewport"]["path"] = (
        "screenshots/second-viewport.png"
    )
    (bundle / "journey-evidence.json").write_text(
        json.dumps(manifest), encoding="utf-8"
    )

    result = world.service.get(world.repo, RUN_ID, caller())
    assert [item["formal_run_id"] for item in result["bundles"]] == [
        "formal-web-ui-example",
        "formal-web-ui-second",
    ]
    assert result["image_count"] == 2
    assert result["issues"] == []
    second = result["bundles"][1]["cells"][0]["screenshots"]["viewport"]
    chunk = world.service.image(
        world.repo,
        {"run_id": RUN_ID, "image_id": second["image_id"]},
        caller(),
    )
    assert base64.b64decode(chunk["base64"]) == PNG
    assert world.service.summary(world.repo, RUN_ID, caller()) == {
        "status": "available",
        "bundle_count": 2,
        "image_count": 2,
        "issue_count": 0,
        "issues_truncated": False,
    }


def test_nested_bundle_discovery_ignores_symlinks_and_unrecognized_names(world):
    outside = world.repo / "outside"
    outside.mkdir()
    (outside / "journey-evidence.json").write_text(
        json.dumps(world.manifest), encoding="utf-8"
    )
    formal_runs = world.evidence / "formal-runs"
    formal_runs.mkdir()
    (formal_runs / ("c" * 64)).symlink_to(outside, target_is_directory=True)
    unrecognized = formal_runs / "not-a-bundle"
    unrecognized.mkdir()
    (unrecognized / "journey-evidence.json").write_text(
        json.dumps(world.manifest), encoding="utf-8"
    )

    result = world.service.get(world.repo, RUN_ID, caller())
    assert len(result["bundles"]) == 1
    assert result["image_count"] == 1
    assert result["issues"] == []


def test_invalid_or_expired_evidence_is_truthful(world):
    world.manifest["governedRunId"] = "another-run"
    (world.evidence / "journey-evidence.json").write_text(
        json.dumps(world.manifest), encoding="utf-8"
    )
    result = world.service.get(world.repo, RUN_ID, caller())
    assert result["status"] == "unavailable"
    assert result["issues"][0]["code"] == "invalid_evidence"
    with pytest.raises(ProtocolError) as caught:
        world.service.get(world.repo, "t20260902T020304Z-fedcba", caller())
    assert caught.value.code == "test_evidence_expired"


def test_annotation_creates_plan_feedback_and_supports_discussion_lifecycle(world):
    evidence = world.service.get(world.repo, RUN_ID, caller())
    image_id = evidence["bundles"][0]["cells"][0]["screenshots"]["viewport"]["image_id"]
    created = world.service.create_feedback(
        world.repo,
        {
            "run_id": RUN_ID,
            "image_id": image_id,
            "body": "The sign-in button needs stronger contrast.",
            "marks": [
                {
                    "id": "mark-1",
                    "type": "rectangle",
                    "color": "#f59e0b",
                    "x": 0.2,
                    "y": 0.3,
                    "width": 0.4,
                    "height": 0.1,
                },
                {"id": "mark-2", "type": "pin", "color": "#4c8dff", "x": 0.5, "y": 0.5},
                {
                    "id": "mark-3",
                    "type": "arrow",
                    "color": "#ef4444",
                    "x1": 0.1,
                    "y1": 0.1,
                    "x2": 0.4,
                    "y2": 0.4,
                },
                {
                    "id": "mark-4",
                    "type": "freehand",
                    "color": "#22c55e",
                    "points": [{"x": 0.1, "y": 0.7}, {"x": 0.4, "y": 0.8}],
                },
                {
                    "id": "mark-5",
                    "type": "highlight",
                    "color": "#a855f7",
                    "points": [{"x": 0.2, "y": 0.6}, {"x": 0.7, "y": 0.6}],
                },
                {
                    "id": "mark-6",
                    "type": "text",
                    "color": "#f8fafc",
                    "x": 0.3,
                    "y": 0.3,
                    "text": "Needs more room",
                },
            ],
        },
        caller(),
    )
    task = dict(world.db.query("SELECT * FROM tasks WHERE task_id=?", (created["task_id"],))[0])
    assert task["kind"] == "user_feedback" and task["status"] == "planned"
    assert task["title"] == "Review: The sign-in button needs stronger contrast."
    feedback = created["feedback"]
    assert feedback["marks"][0]["type"] == "rectangle"
    assert {mark["type"] for mark in feedback["marks"]} == {
        "rectangle",
        "pin",
        "arrow",
        "freehand",
        "highlight",
        "text",
    }
    root_comment = feedback["comments"][0]["comment_id"]

    replied = world.service.reply(
        world.repo,
        {
            "run_id": RUN_ID,
            "feedback_id": feedback["feedback_id"],
            "body": "Please use the normal primary action treatment.",
        },
        caller(),
    )["feedback"]
    assert len(replied["comments"]) == 2

    edited = world.service.edit(
        world.repo,
        {
            "run_id": RUN_ID,
            "feedback_id": feedback["feedback_id"],
            "comment_id": root_comment,
            "body": "The sign-in button needs the standard primary contrast.",
        },
        caller(),
    )["feedback"]
    assert edited["comments"][0]["body"].endswith("contrast.")
    assert world.db.query("SELECT title FROM tasks WHERE task_id=?", (created["task_id"],))[0][
        "title"
    ].endswith("primary contrast.")

    resolved = world.service.set_state(
        world.repo,
        {
            "run_id": RUN_ID,
            "feedback_id": feedback["feedback_id"],
            "state": "resolved",
        },
        caller(),
    )["feedback"]
    assert resolved["state"] == "resolved" and resolved["task_status"] == "done"
    reopened = world.service.set_state(
        world.repo,
        {
            "run_id": RUN_ID,
            "feedback_id": feedback["feedback_id"],
            "state": "open",
        },
        caller(),
    )["feedback"]
    assert reopened["state"] == "open" and reopened["task_status"] == "planned"

    with pytest.raises(ProtocolError, match="author"):
        world.service.delete(
            world.repo,
            {
                "run_id": RUN_ID,
                "feedback_id": feedback["feedback_id"],
            },
            caller("other@example.test"),
        )
    deleted = world.service.delete(
        world.repo,
        {
            "run_id": RUN_ID,
            "feedback_id": feedback["feedback_id"],
        },
        caller(),
    )["feedback"]
    assert deleted["state"] == "deleted" and deleted["task_status"] == "dropped"
    assert (
        world.db.query("SELECT COUNT(*) AS count FROM visual_feedback_events")[0]["count"] >= 6
    )


def test_handlers_validate_exact_evidence_arguments(world):
    handlers = build_handlers(world.config, world.registry, test_evidence=world.service)
    expected = {
        "test.evidence.get",
        "test.evidence.image",
        "test.evidence.feedback.create",
        "test.evidence.feedback.reply",
        "test.evidence.feedback.edit",
        "test.evidence.feedback.state",
        "test.evidence.feedback.delete",
    }
    assert expected <= set(handlers)
    with pytest.raises(ProtocolError, match="required"):
        handlers["test.evidence.get"]({"path": str(world.repo)}, caller())
    with pytest.raises(ProtocolError, match="unknown"):
        handlers["test.evidence.get"](
            {
                "path": str(world.repo),
                "run_id": RUN_ID,
                "extra": True,
            },
            caller(),
        )


def test_test_list_includes_bounded_visual_evidence_availability(world):
    class Lifecycle:
        @staticmethod
        def list_current():
            return [
                {
                    "run_id": RUN_ID,
                    "worktree_path": str(world.repo),
                    "status": "passed",
                }
            ]

    handlers = build_handlers(
        world.config,
        world.registry,
        lifecycle=Lifecycle(),
        test_evidence=world.service,
    )
    listed = handlers["test.list"]({}, caller())["runs"][0]
    assert listed["visual_evidence"] == {
        "status": "available",
        "bundle_count": 1,
        "image_count": 1,
        "issue_count": 0,
        "issues_truncated": False,
    }
