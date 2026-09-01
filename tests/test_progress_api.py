import os
from datetime import UTC, datetime
from pathlib import Path

import pytest

from devcoordinator2.daemon import securefs, summary, tests_support
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.plan_api import build_plan_handlers
from devcoordinator2.daemon.progress_api import (
    PERIODS,
    _window,
    build_progress_handlers,
    repository_report,
)
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

CALLER = Caller(pid=0, uid=1000, gid=1000, client_kind="other", client_session=None)
NOW = int(datetime(2026, 8, 30, 12, tzinfo=UTC).timestamp() * 1000)


class FakeUsage:
    def __init__(self, observed=True):
        self.observed = observed

    def repository_buckets(self, repository, label, bucket_ms, bucket_count,
                           now_ms, *, aligned_end_ms, resolve_missing=True):
        assert label.startswith("progress-")
        assert bucket_ms in {item["bucket_ms"] for item in PERIODS.values()}
        coverage = "complete" if self.observed else "unobserved"
        return {
            "coverage": {
                "state": coverage, "has_gaps": not self.observed,
                "configured_collectors": 1, "available_collectors": 1,
                "contributing_collectors": int(self.observed),
                "freshest_at_ms": now_ms if self.observed else None,
                "unavailable_reasons": {},
            },
            "series": [{
                "bucket_start_ms": aligned_end_ms - (bucket_count - index) * bucket_ms,
                "bucket_end_ms": aligned_end_ms - (bucket_count - index - 1) * bucket_ms,
                "coverage": coverage, "phases": {},
                "total_tokens": 1000 + index if self.observed else 0,
            } for index in range(bucket_count)],
        }


@pytest.fixture
def world(tmp_path: Path):
    repo = tmp_path / "repo"
    repo.mkdir()
    config = InstanceConfig(socket_path=tmp_path / "s", state_dir=tmp_path / "state",
                            unit_prefix="devcoordinator2-dev", slice_name="x.slice",
                            client_group="", base_domain="example.test",
                            admin_emails=("owner@example.test",))
    db = Database(config.database_path)
    with db.transaction() as conn:
        conn.execute("INSERT INTO repositories VALUES('r1',?,'repo-one','t',1,'t')",
                     (str(repo),))
        conn.execute("INSERT INTO worktrees VALUES('w1','r1',?,'t','t')",
                     (str(repo),))
    plan = build_plan_handlers(config, db, Registry(db))
    yield type("World", (), {"db": db, "repo": repo, "config": config,
                              "plan": plan})
    db.close()


def call(world, command, **args):
    return world.plan[command](args, CALLER)


def _terminal(run_id, status, finished):
    return summary.build(
        run_id, "unit", status, "2026-08-30T09:00:00Z", os.getuid(), "codex",
        finished_at=finished, duration_seconds=60,
        exit_code=0 if status == "passed" else 1)


def _populated(world):
    release = call(world, "release.create", repository_id="r1",
                   name="Release one", kind="release")
    done_one = call(world, "task.create", repository_id="r1",
                    title="People can create an account", kind="goal",
                    release_id=release["release_id"], estimated_loc=100)
    done_two = call(world, "task.create", repository_id="r1",
                    title="People can invite a teammate", kind="goal",
                    release_id=release["release_id"], estimated_loc=200)
    open_known = call(world, "task.create", repository_id="r1",
                      title="Billing changes save reliably", kind="goal",
                      release_id=release["release_id"], estimated_loc=300,
                      outcome="Billing changes save without errors for the account owner.")
    open_unknown = call(world, "task.create", repository_id="r1",
                        title="Search results arrive quickly", kind="user_feedback",
                        release_id=release["release_id"],
                        outcome="People can find the right result without waiting.")
    call(world, "task.update", task_id=done_one["task_id"], status="done")
    call(world, "task.update", task_id=done_two["task_id"], status="done")
    call(world, "task.update", task_id=open_known["task_id"], status="done")
    call(world, "task.update", task_id=open_known["task_id"], status="in_progress",
         note="Final release receipts are not yet attached.")
    with world.db.transaction() as conn:
        conn.execute(
            "UPDATE plan_events SET at='2026-08-25T12:00:00Z'"
            " WHERE subject_id=? AND event='status' AND to_value='done'",
            (done_one["task_id"],))
        conn.execute(
            "UPDATE plan_events SET at='2026-08-28T12:00:00Z'"
            " WHERE subject_id=? AND event='status' AND to_value='done'",
            (done_two["task_id"],))
        conn.execute(
            "UPDATE plan_events SET at='2026-08-20T12:00:00Z'"
            " WHERE subject_id=? AND event='status'",
            (open_known["task_id"],))
    securefs.create_test_dir(world.repo, os.getuid(), os.getgid())
    owner = (os.getuid(), os.getgid())
    tests_support.record_history(
        world.repo, _terminal("t-one", "passed", "2026-08-26T10:00:00Z"), owner)
    tests_support.record_history(
        world.repo, _terminal("t-two", "failed", "2026-08-29T10:00:00Z"), owner)
    return release, open_known, open_unknown


def test_day_report_combines_measured_sources_and_explainable_forecast(world):
    release, open_known, open_unknown = _populated(world)
    repository = Registry(world.db).list_repositories()[0]
    report = repository_report(world.db, repository, FakeUsage(), "day", NOW)
    assert report["period"] == "day" and len(report["series"]) == 7
    assert report["comparison"]["current"]["tasks_completed"] == 2
    assert report["comparison"]["current"]["planned_lines_completed"] == 300
    assert report["comparison"]["current"]["test_pass_rate"] == 0.5
    assert report["comparison"]["current"]["total_tokens"] is not None
    assert report["forecast"]["state"] == "available"
    assert report["forecast"]["release"]["release_id"] == release["release_id"]
    assert report["forecast"]["target_date_recorded"] is False
    assert report["forecast"]["earliest_at_ms"] <= \
        report["forecast"]["likely_at_ms"] <= report["forecast"]["latest_at_ms"]
    assert "No target date" in report["forecast"]["explanation"]
    assert [row["task_id"] for row in report["release_work"]] == [
        open_known["task_id"], open_unknown["task_id"]]
    known = report["release_work"][0]
    assert known["title"] == "Billing changes save reliably"
    assert known["status"] == "in_progress" and known["reopened"] is True
    assert known["reopen_note"] == "Final release receipts are not yet attached."
    assert "outcome" not in known and "priorities" not in report
    assert report["coverage"]["state"] == "partial"  # history starts mid-window
    assert report["semantics"]["lines"].startswith("current planned")


def test_no_release_or_history_returns_honest_unavailable_states(world):
    call(world, "task.create", repository_id="r1",
         title="People can export a report", kind="goal", estimated_loc=50)
    repository = Registry(world.db).list_repositories()[0]
    report = repository_report(world.db, repository, FakeUsage(False), "hour", NOW)
    assert report["forecast"]["state"] == "unavailable"
    assert report["forecast"]["reason"] == "no_planned_release"
    assert report["coverage"]["tests"]["state"] == "unobserved"
    assert report["coverage"]["tokens"]["state"] == "unobserved"
    assert all(point["total_tokens"] is None for point in report["series"])


def test_planned_release_without_completed_work_does_not_invent_a_date(world):
    release = call(world, "release.create", repository_id="r1",
                   name="First release", kind="release")
    call(world, "task.create", repository_id="r1",
         title="People can export a report", kind="goal",
         release_id=release["release_id"], estimated_loc=50)
    repository = Registry(world.db).list_repositories()[0]
    report = repository_report(world.db, repository, FakeUsage(), "day", NOW)
    assert report["forecast"]["reason"] == "insufficient_completion_history"
    assert report["forecast"].get("likely_at_ms") is None


def test_release_work_follows_nested_plan_order_and_keeps_unknown_size(world):
    release = call(world, "release.create", repository_id="r1",
                   name="First release", kind="release")
    parent = call(world, "task.create", repository_id="r1",
                  title="Check the release", kind="goal",
                  release_id=release["release_id"])
    later = call(world, "task.create", repository_id="r1",
                 title="Check recovery", kind="improvement",
                 parent_task_id=parent["task_id"], release_id=release["release_id"],
                 estimated_loc=90)
    first = call(world, "task.create", repository_id="r1",
                 title="Check sign in", kind="improvement",
                 parent_task_id=parent["task_id"], release_id=release["release_id"])
    call(world, "task.update", task_id=first["task_id"], position=0)
    repository = Registry(world.db).list_repositories()[0]
    report = repository_report(world.db, repository, FakeUsage(), "day", NOW)
    assert [row["task_id"] for row in report["release_work"]] == [
        first["task_id"], later["task_id"]]
    assert report["release_work"][0]["estimated_loc"] is None
    assert parent["task_id"] not in {
        row["task_id"] for row in report["release_work"]}


def test_week_windows_align_to_monday_and_handlers_validate_scope(world):
    window = _window("week", NOW)
    assert datetime.fromtimestamp(window["aligned_end_ms"] / 1000, UTC).weekday() == 0
    registry = Registry(world.db)
    handlers = build_progress_handlers(world.db, registry, FakeUsage())
    picker = handlers["progress.repositories"]({"_repository_ids": ["r1"]}, CALLER)
    assert picker["repositories"][0]["repository_id"] == "r1"
    with pytest.raises(ProtocolError, match="hour, day, or week"):
        handlers["progress.repository"](
            {"repository_id": "r1", "period": "month"}, CALLER)
    with pytest.raises(ProtocolError, match="unknown args"):
        handlers["progress.repositories"]({"private": True}, CALLER)
