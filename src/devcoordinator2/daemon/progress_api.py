"""Repository delivery progress, factual release work, and explainable forecasts.

Every number is derived from an existing authority: permanent plan events,
bounded repository-local test summaries, and the read-only Codex usage
collector. Missing evidence stays missing; the forecast is a deterministic
range and never a promised delivery date.
"""

from __future__ import annotations

import math
import statistics
import time
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from devcoordinator2.daemon import securefs, summary, tests_support
from devcoordinator2.daemon.codex_usage import CodexUsage
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller, Handler
from devcoordinator2.paths import test_dir
from devcoordinator2.protocol import ProtocolError

HOUR_MS = 60 * 60 * 1000
DAY_MS = 24 * HOUR_MS
WEEK_MS = 7 * DAY_MS
MONDAY_EPOCH_MS = int(datetime(1970, 1, 5, tzinfo=UTC).timestamp() * 1000)
FORECAST_LOOKBACK_MS = 28 * DAY_MS
QUALITY_TEST_STATUSES = frozenset({"passed", "failed", "timed-out", "interrupted"})
PERIODS = {
    "hour": {"bucket_ms": HOUR_MS, "current_buckets": 24},
    "day": {"bucket_ms": DAY_MS, "current_buckets": 7},
    "week": {"bucket_ms": WEEK_MS, "current_buckets": 8},
}


def _parse_ms(value: str | None) -> int | None:
    if not value:
        return None
    try:
        return int(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
                   * 1000)
    except ValueError:
        return None


def _iso(ms: int) -> str:
    return datetime.fromtimestamp(ms / 1000, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _window(period: str, now_ms: int) -> dict[str, int]:
    spec = PERIODS[period]
    bucket_ms = spec["bucket_ms"]
    current = spec["current_buckets"]
    if period == "week":
        aligned_end = (math.ceil((now_ms - MONDAY_EPOCH_MS) / bucket_ms)
                       * bucket_ms + MONDAY_EPOCH_MS)
    else:
        aligned_end = math.ceil(now_ms / bucket_ms) * bucket_ms
    total = current * 2
    return {
        "bucket_ms": bucket_ms,
        "current_buckets": current,
        "total_buckets": total,
        "aligned_end_ms": aligned_end,
        "start_ms": aligned_end - total * bucket_ms,
        "current_start_ms": aligned_end - current * bucket_ms,
        "end_ms": now_ms,
    }


def _bucket_index(timestamp_ms: int | None, window: dict[str, int]) -> int | None:
    if timestamp_ms is None:
        return None
    index = (timestamp_ms - window["start_ms"]) // window["bucket_ms"]
    return int(index) if 0 <= index < window["total_buckets"] else None


def _number(value: str | None) -> int:
    if value in (None, "", "None"):
        return 0
    try:
        return int(value)
    except ValueError:
        return 0


def _empty_buckets(window: dict[str, int]) -> list[dict[str, Any]]:
    return [{
        "bucket_start_ms": window["start_ms"] + i * window["bucket_ms"],
        "bucket_end_ms": window["start_ms"] + (i + 1) * window["bucket_ms"],
        "tasks_completed": 0,
        "tasks_created": 0,
        "tasks_reopened": 0,
        "planned_lines_completed": 0,
        "scope_lines_changed": 0,
        "test_runs": 0,
        "tests_passed": 0,
        "test_pass_rate": None,
        "total_tokens": None,
        "token_coverage": "unobserved",
    } for i in range(window["total_buckets"])]


def _task_rows(db: Database, repository_id: str) -> tuple[list[dict], set[str]]:
    tasks = [dict(row) for row in db.query(
        "SELECT * FROM tasks WHERE repository_id=? ORDER BY seq",
        (repository_id,))]
    parents = {row["parent_task_id"] for row in tasks if row["parent_task_id"]}
    return tasks, parents


def _plan_series(db: Database, repository_id: str, window: dict[str, int],
                 buckets: list[dict[str, Any]]) -> dict[str, Any]:
    tasks, parents = _task_rows(db, repository_id)
    by_id = {task["task_id"]: task for task in tasks}
    events = db.query(
        "SELECT subject_id, event, from_value, to_value, at FROM plan_events"
        " WHERE repository_id=? AND subject_kind='task' AND at>=? AND at<?"
        " ORDER BY event_id",
        (repository_id, _iso(window["start_ms"]), _iso(window["aligned_end_ms"])))
    estimated_completions = 0
    total_completions = 0
    for event in events:
        task = by_id.get(event["subject_id"])
        if task is None or task["task_id"] in parents:
            continue
        index = _bucket_index(_parse_ms(event["at"]), window)
        if index is None:
            continue
        bucket = buckets[index]
        estimated = task["estimated_loc"] or 0
        if event["event"] == "created":
            bucket["tasks_created"] += 1
            bucket["scope_lines_changed"] += estimated
        elif event["event"] == "status" and event["to_value"] == "done":
            bucket["tasks_completed"] += 1
            bucket["planned_lines_completed"] += estimated
            total_completions += 1
            estimated_completions += int(bool(estimated))
        elif event["event"] == "status" and event["from_value"] == "done" \
                and event["to_value"] == "in_progress":
            bucket["tasks_reopened"] += 1
        elif event["event"] == "status" and event["to_value"] == "dropped":
            bucket["scope_lines_changed"] -= estimated
        elif event["event"] == "status" and event["from_value"] == "dropped":
            bucket["scope_lines_changed"] += estimated
        elif event["event"] == "estimate":
            bucket["scope_lines_changed"] += (
                _number(event["to_value"]) - _number(event["from_value"]))
    active = [task for task in tasks if task["status"] != "dropped"]
    leaves = [task for task in active if task["task_id"] not in parents]
    return {
        "tasks": tasks,
        "parents": parents,
        "leaves": leaves,
        "events": [dict(event) for event in events],
        "estimate_coverage": {
            "completed_with_estimate": estimated_completions,
            "completed_total": total_completions,
        },
    }


def _repository_test_history(repository: dict) -> tuple[list[dict], dict[str, Any]]:
    runs: dict[str, dict] = {}
    errors = 0
    sources = 0
    for worktree in repository.get("worktrees", []):
        root = Path(worktree["worktree_path"])
        try:
            recorded = tests_support.read_history(root)
            sources += int(bool(recorded))
            for run in recorded:
                runs[run["run_id"]] = run
        except securefs.SecureFsError:
            errors += 1
        current = summary.read(test_dir(root) / "summary.json")
        if current and current["status"] in summary.TERMINAL_STATUSES:
            runs.setdefault(current["run_id"], tests_support.history_entry(current))
    ordered = sorted(runs.values(), key=lambda run: run.get("finished_at") or "")
    state = "unavailable" if errors and not ordered else (
        "partial" if errors or (ordered and not sources)
        else "complete" if ordered else "unobserved")
    return ordered, {
        "state": state,
        "recorded_runs": len(ordered),
        "history_sources": sources,
        "unavailable_sources": errors,
        "earliest_at": ordered[0].get("finished_at") if ordered else None,
    }


def _add_test_series(runs: list[dict], window: dict[str, int],
                     buckets: list[dict[str, Any]]) -> None:
    for run in runs:
        if run["status"] not in QUALITY_TEST_STATUSES:
            continue
        index = _bucket_index(_parse_ms(run.get("finished_at")), window)
        if index is None:
            continue
        buckets[index]["test_runs"] += 1
        buckets[index]["tests_passed"] += int(run["status"] == "passed")
    for bucket in buckets:
        if bucket["test_runs"]:
            bucket["test_pass_rate"] = (
                bucket["tests_passed"] / bucket["test_runs"])


def _add_usage_series(report: dict, buckets: list[dict[str, Any]]) -> None:
    for bucket, point in zip(buckets, report["series"], strict=True):
        bucket["token_coverage"] = point["coverage"]
        bucket["total_tokens"] = (
            None if point["coverage"] == "unobserved" else point["total_tokens"])


def _totals(buckets: list[dict[str, Any]], duration_days: float) -> dict[str, Any]:
    tests = sum(bucket["test_runs"] for bucket in buckets)
    passed = sum(bucket["tests_passed"] for bucket in buckets)
    observed_tokens = [bucket["total_tokens"] for bucket in buckets
                       if bucket["total_tokens"] is not None]
    lines = sum(bucket["planned_lines_completed"] for bucket in buckets)
    completed = sum(bucket["tasks_completed"] for bucket in buckets)
    created = sum(bucket["tasks_created"] for bucket in buckets)
    return {
        "tasks_completed": completed,
        "tasks_created": created,
        "tasks_reopened": sum(bucket["tasks_reopened"] for bucket in buckets),
        "planned_lines_completed": lines,
        "scope_lines_changed": sum(bucket["scope_lines_changed"] for bucket in buckets),
        "test_runs": tests,
        "tests_passed": passed,
        "test_pass_rate": passed / tests if tests else None,
        "total_tokens": sum(observed_tokens) if observed_tokens else None,
        "tokens_per_completed_task": (
            sum(observed_tokens) / completed if observed_tokens and completed else None),
        "tokens_per_planned_line": (
            sum(observed_tokens) / lines if observed_tokens and lines else None),
        "tasks_completed_per_day": completed / duration_days,
        "tasks_created_per_day": created / duration_days,
        "tests_per_completed_task": tests / completed if completed else None,
    }


def _completion_evidence(db: Database, repository_id: str, plan: dict,
                         now_ms: int) -> dict[str, Any]:
    by_id = {task["task_id"]: task for task in plan["tasks"]}
    parents = plan["parents"]
    cutoff = now_ms - FORECAST_LOOKBACK_MS
    completed = []
    events = db.query(
        "SELECT subject_id, event, to_value, at FROM plan_events"
        " WHERE repository_id=? AND subject_kind='task' AND event='status'"
        " AND to_value='done' AND at>=? AND at<? ORDER BY event_id",
        (repository_id, _iso(cutoff), _iso(now_ms + 1000)))
    for event in events:
        at_ms = _parse_ms(event["at"])
        task = by_id.get(event["subject_id"])
        if event["event"] != "status" or event["to_value"] != "done" \
                or at_ms is None or at_ms < cutoff or task is None \
                or task["task_id"] in parents:
            continue
        completed.append((at_ms, task["estimated_loc"]))
    if not completed:
        return {"tasks": 0, "planned_lines": 0, "lookback_days": 28,
                "tasks_per_day": 0.0, "planned_lines_per_day": 0.0}
    earliest = min(at for at, _ in completed)
    days = max(7.0, min(28.0, (now_ms - earliest) / DAY_MS + 1))
    lines = sum(value or 0 for _, value in completed)
    return {
        "tasks": len(completed), "planned_lines": lines,
        "lookback_days": round(days, 2),
        "tasks_per_day": len(completed) / days,
        "planned_lines_per_day": lines / days,
    }


def _forecast(scope: list[dict], evidence: dict[str, Any], now_ms: int,
              test_pass_rate: float | None, scope_change: int,
              release: dict | None) -> dict[str, Any]:
    open_tasks = [task for task in scope if task["status"] != "done"]
    known_sizes = [task["estimated_loc"] for task in scope if task["estimated_loc"]]
    remaining_known = sum(task["estimated_loc"] or 0 for task in open_tasks)
    unknown = sum(task["estimated_loc"] is None for task in open_tasks)
    base = {
        "release": ({"release_id": release["release_id"], "name": release["name"],
                     "status": release["status"]} if release else None),
        "remaining_tasks": len(open_tasks),
        "remaining_planned_lines": remaining_known,
        "unestimated_tasks": unknown,
        "velocity": evidence,
        "target_date_recorded": False,
        "as_of_ms": now_ms,
    }
    if release is None:
        return {**base, "state": "unavailable", "reason": "no_planned_release",
                "explanation": "Plan a release before estimating its delivery range."}
    if not open_tasks:
        return {**base, "state": "ready", "likely_at_ms": now_ms,
                "earliest_at_ms": now_ms, "latest_at_ms": now_ms,
                "confidence_percent": 90, "confidence": "high",
                "explanation": "All recorded work for this release is complete."
                " No target date is recorded."}
    median_size = statistics.median(known_sizes) if known_sizes else None
    equivalent_lines = remaining_known + (unknown * median_size if median_size else 0)
    if equivalent_lines and evidence["planned_lines_per_day"] > 0:
        days = equivalent_lines / evidence["planned_lines_per_day"]
    elif evidence["tasks_per_day"] > 0:
        days = len(open_tasks) / evidence["tasks_per_day"]
    else:
        return {**base, "state": "unavailable",
                "reason": "insufficient_completion_history",
                "explanation": "More completed work is needed before pace can support"
                " a release forecast."}
    drivers = []
    risk = 1.0
    if unknown:
        risk += min(0.35, unknown / max(1, len(open_tasks)) * 0.35)
        drivers.append(f"{unknown} remaining task{'s are' if unknown != 1 else ' is'}"
                       " not estimated")
    if test_pass_rate is not None and test_pass_rate < 0.8:
        risk += 0.15
        drivers.append("recent test stability is below 80%")
    if scope_change > 0:
        risk += 0.1
        drivers.append("measured scope grew during this period")
    likely_days = max(1.0, days * risk)
    confidence = 88
    if evidence["tasks"] < 5:
        confidence -= 15
    if evidence["tasks"] < 2:
        confidence -= 15
    confidence -= min(30, round(unknown / max(1, len(open_tasks)) * 30))
    if test_pass_rate is None:
        confidence -= 8
    elif test_pass_rate < 0.8:
        confidence -= 12
    if scope_change > 0:
        confidence -= 8
    confidence = max(20, min(90, confidence))
    spread = max(1, math.ceil(likely_days * ((100 - confidence) / 100 + 0.1)))
    likely = now_ms + math.ceil(likely_days) * DAY_MS
    earliest = now_ms + max(1, math.floor(likely_days) - spread) * DAY_MS
    latest = now_ms + (math.ceil(likely_days) + spread) * DAY_MS
    label = "high" if confidence >= 80 else "medium" if confidence >= 55 else "low"
    explanation = (drivers[0].capitalize() + "." if drivers else
                   "The range is based on recent measured completion pace.")
    explanation += " No target date is recorded."
    return {**base, "state": "available", "likely_at_ms": likely,
            "earliest_at_ms": earliest, "latest_at_ms": latest,
            "confidence_percent": confidence, "confidence": label,
            "drivers": drivers, "explanation": explanation,
            "assumptions": [
                "Current task estimates are used as planned size, not measured Git changes.",
                "Unestimated work uses the median recorded task size when available.",
                "Ordering alone does not change total scope or the central forecast.",
            ]}


def _release_scope(db: Database, repository_id: str, tasks: list[dict],
                   parents: set[str]) \
        -> tuple[dict | None, list[dict]]:
    releases = [dict(row) for row in db.query(
        "SELECT * FROM releases WHERE repository_id=?"
        " AND status IN ('planned','requested') ORDER BY seq",
        (repository_id,))]
    release = releases[0] if releases else None
    release_id = release["release_id"] if release else None
    grouped = [task for task in tasks if task["status"] != "dropped"
               and task["release_id"] == release_id]
    grouped_by_id = {task["task_id"]: task for task in grouped}
    children: dict[str, list[dict]] = {}
    for task in grouped:
        if task["parent_task_id"] in grouped_by_id:
            children.setdefault(task["parent_task_id"], []).append(task)
    def order(task: dict) -> tuple[int, int]:
        return task["position"], task["seq"]
    for rows in children.values():
        rows.sort(key=order)
    scope: list[dict] = []

    def walk(task: dict) -> None:
        nested = children.get(task["task_id"], [])
        if not nested:
            if task["task_id"] not in parents:
                scope.append(task)
            return
        for child in nested:
            walk(child)

    roots = [task for task in grouped
             if task["parent_task_id"] not in grouped_by_id]
    for task in sorted(roots, key=order):
        walk(task)
    return release, scope


def _release_work(db: Database, repository_id: str,
                  scope: list[dict]) -> list[dict[str, Any]]:
    reopened: dict[str, str | None] = {}
    for event in db.query(
            "SELECT subject_id, note FROM plan_events"
            " WHERE repository_id=? AND subject_kind='task' AND event='status'"
            " AND from_value='done' AND to_value!='done' ORDER BY event_id",
            (repository_id,)):
        reopened[event["subject_id"]] = event["note"]
    return [{
        "task_id": task["task_id"],
        "title": task["title"],
        "status": task["status"],
        "kind": task["kind"],
        "estimated_loc": task["estimated_loc"],
        "elaboration_needed": bool(task.get("elaboration_needed")),
        "unblock_condition": task.get("unblock_condition"),
        "reopened": task["task_id"] in reopened,
        "reopen_note": reopened.get(task["task_id"]),
    } for task in scope if task["status"] != "done"]


def _coverage(plan: dict, tests: dict, usage: dict) -> dict[str, Any]:
    states = [tests["state"], usage["state"]]
    state = "complete" if all(value == "complete" for value in states) else (
        "unavailable" if all(value in ("unavailable", "unobserved") for value in states)
        else "partial")
    return {
        "state": state,
        "plan": {"state": "complete", **plan["estimate_coverage"]},
        "tests": tests,
        "tokens": usage,
    }


def repository_report(db: Database, repository: dict, usage: CodexUsage,
                      period: str, now_ms: int | None = None) -> dict[str, Any]:
    now_ms = now_ms or int(time.time() * 1000)
    window = _window(period, now_ms)
    buckets = _empty_buckets(window)
    plan = _plan_series(db, repository["repository_id"], window, buckets)
    test_runs, test_coverage = _repository_test_history(repository)
    if test_coverage["state"] == "complete" \
            and (_parse_ms(test_coverage["earliest_at"]) or now_ms) > window["start_ms"]:
        test_coverage["state"] = "partial"
    _add_test_series(test_runs, window, buckets)
    usage_report = usage.repository_buckets(
        repository, f"progress-{period}", window["bucket_ms"],
        window["total_buckets"], now_ms,
        aligned_end_ms=window["aligned_end_ms"])
    _add_usage_series(usage_report, buckets)
    split = window["current_buckets"]
    previous_buckets, current_buckets = buckets[:split], buckets[split:]
    duration_days = split * window["bucket_ms"] / DAY_MS
    current = _totals(current_buckets, duration_days)
    previous = _totals(previous_buckets, duration_days)
    release, scope = _release_scope(
        db, repository["repository_id"], plan["tasks"], plan["parents"])
    evidence = _completion_evidence(
        db, repository["repository_id"], plan, now_ms)
    forecast = _forecast(scope, evidence, now_ms, current["test_pass_rate"],
                         current["scope_lines_changed"], release)
    release_work = _release_work(db, repository["repository_id"], scope)
    leaves = [task for task in plan["leaves"] if task["status"] != "dropped"]
    coverage = _coverage(
        plan, test_coverage,
        {key: usage_report["coverage"][key] for key in (
            "state", "has_gaps", "configured_collectors", "available_collectors",
            "contributing_collectors", "freshest_at_ms", "unavailable_reasons")})
    return {
        "repository_id": repository["repository_id"],
        "display_name": repository["display_name"],
        "period": period,
        "generated_at_ms": now_ms,
        "window": {
            "bucket_ms": window["bucket_ms"],
            "start_ms": window["current_start_ms"],
            "end_ms": now_ms,
            "comparison_start_ms": window["start_ms"],
            "timezone": "UTC",
        },
        "scope": {
            "tasks_total": len(leaves),
            "tasks_done": sum(task["status"] == "done" for task in leaves),
            "planned_lines_total": sum(task["estimated_loc"] or 0 for task in leaves),
            "planned_lines_done": sum((task["estimated_loc"] or 0)
                                      for task in leaves if task["status"] == "done"),
            "unestimated_open_tasks": sum(task["status"] != "done"
                                          and task["estimated_loc"] is None
                                          for task in leaves),
        },
        "series": current_buckets,
        "comparison": {"current": current, "previous": previous},
        "forecast": forecast,
        "release_work": release_work,
        "coverage": coverage,
        "semantics": {
            "tasks": "terminal task status events in the permanent plan ledger",
            "lines": "current planned task estimates completed; not measured Git changes",
            "tests": "bounded repository-local terminal test summaries",
            "tokens": "provider total_tokens; missing collector coverage stays missing",
            "forecast": ("provisional range from recent completion pace, current"
                         " estimates, scope movement, and test stability; when work"
                         " has no estimate, the median recorded task size is used"
                         " when available"),
        },
    }


def repositories_report(db: Database, repositories: list[dict]) -> dict[str, Any]:
    rows = []
    for repository in repositories:
        tasks, parents = _task_rows(db, repository["repository_id"])
        leaves = [task for task in tasks if task["status"] != "dropped"
                  and task["task_id"] not in parents]
        releases = db.query(
            "SELECT release_id, name, status FROM releases WHERE repository_id=?"
            " AND status IN ('planned','requested') ORDER BY seq LIMIT 1",
            (repository["repository_id"],))
        rows.append({
            "repository_id": repository["repository_id"],
            "display_name": repository["display_name"],
            "open_tasks": sum(task["status"] != "done" for task in leaves),
            "tasks_done": sum(task["status"] == "done" for task in leaves),
            "planned_lines_done": sum((task["estimated_loc"] or 0)
                                      for task in leaves if task["status"] == "done"),
            "planned_lines_total": sum(task["estimated_loc"] or 0 for task in leaves),
            "next_release": (dict(releases[0]) if releases else None),
        })
    return {"repositories": rows}


def build_progress_handlers(db: Database, registry: Registry,
                            usage: CodexUsage) -> dict[str, Handler]:
    def repositories(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        unknown = set(args) - {"_repository_ids"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        repository_ids = args.get("_repository_ids")
        if repository_ids is not None and (not isinstance(repository_ids, list)
                                           or not all(isinstance(item, str)
                                                      for item in repository_ids)):
            raise ProtocolError("args_invalid", "repository scope is invalid")
        records = registry.list_repositories()
        if repository_ids is not None:
            allowed = set(repository_ids)
            records = [record for record in records
                       if record["repository_id"] in allowed]
        return repositories_report(db, records)

    def repository(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        unknown = set(args) - {"repository_id", "period"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        repository_id = args.get("repository_id")
        period = args.get("period", "day")
        if not isinstance(repository_id, str):
            raise ProtocolError("args_invalid", "'repository_id' is required")
        if period not in PERIODS:
            raise ProtocolError("args_invalid", "'period' must be hour, day, or week")
        record = next((item for item in registry.list_repositories()
                       if item["repository_id"] == repository_id), None)
        if record is None:
            raise ProtocolError("repository_not_found", "no registered repository")
        return repository_report(db, record, usage, period)

    return {"progress.repositories": repositories,
            "progress.repository": repository}
