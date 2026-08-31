"""Read-only, privacy-preserving aggregation of configured Codex collectors.

Usage facts remain canonical in each user's private Codex database.  This
module resolves one opaque repository key per configured source, queries only
allowlisted categorical columns, and combines measurements before anything is
returned to the Console.
"""

from __future__ import annotations

import json
import os
import pwd
import re
import sqlite3
import subprocess
import time
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from urllib.parse import quote

from devcoordinator2.daemon.db import Database
from devcoordinator2.paths import CodexUsageSource, InstanceConfig

SUPPORTED_DATABASE_SCHEMA = 4
SUPPORTED_TAXONOMY = 1
SOURCE_TIMEOUT_SECONDS = 15
SOURCE_OUTPUT_BYTES = 262_144
QUERY_TIMEOUT_SECONDS = 2.0
REPOSITORY_KEY = re.compile(r"[0-9a-f]{64}$")
PHASES = ("planning", "implementation", "testing", "deployment", "reporting",
          "unattributed")
TOKEN_CATEGORIES = (
    "total_tokens", "input_tokens", "input_tokens_details.cached_tokens",
    "output_tokens", "output_tokens_details.reasoning_tokens",
)
WINDOWS = {
    "24h": (60 * 60 * 1000, 24),
    "7d": (6 * 60 * 60 * 1000, 28),
    "30d": (24 * 60 * 60 * 1000, 30),
}


class SourceUnavailable(Exception):
    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason


@dataclass
class SourceReport:
    database_schema: int
    taxonomy_version: int
    evidence: bool = False
    freshest_at_ms: int | None = None
    tokens: dict[str, int] = field(default_factory=dict)
    token_observations: Counter = field(default_factory=Counter)
    phase_series: list[Counter] = field(default_factory=list)
    bucket_coverage: list[str] = field(default_factory=list)
    activities: Counter = field(default_factory=Counter)
    activity_operations: Counter = field(default_factory=Counter)
    activity_provenance: Counter = field(default_factory=Counter)
    operation_count: int = 0
    model_request_count: int = 0
    tool_count: int = 0
    tool_outcomes: Counter = field(default_factory=Counter)
    tool_families: Counter = field(default_factory=Counter)
    coverage_events: Counter = field(default_factory=Counter)
    request_intervals: list[tuple[int, int]] = field(default_factory=list)
    execution_intervals: list[tuple[int, int]] = field(default_factory=list)
    agent_intervals: dict[str, list[tuple[int, int]]] = field(
        default_factory=lambda: defaultdict(list))
    phase_intervals: dict[str, list[tuple[int, int]]] = field(
        default_factory=lambda: defaultdict(list))
    request_unknown: int = 0
    execution_unknown: int = 0
    agent_unknown: int = 0
    phase_unknown: Counter = field(default_factory=Counter)


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _window(range_key: str, now_ms: int) -> tuple[int, int, int, int]:
    bucket_ms, count = WINDOWS[range_key]
    aligned_end = ((now_ms + bucket_ms - 1) // bucket_ms) * bucket_ms
    return aligned_end - bucket_ms * count, now_ms, bucket_ms, count


def _bucket_index(timestamp_ms: int, start_ms: int, bucket_ms: int,
                  bucket_count: int) -> int | None:
    index = (timestamp_ms - start_ms) // bucket_ms
    return int(index) if 0 <= index < bucket_count else None


def _clip_interval(start: int, end: int | None,
                   range_start: int, range_end: int) -> tuple[int, int] | None:
    if end is None or end < start:
        return None
    clipped = (max(start, range_start), min(end, range_end))
    return clipped if clipped[0] <= clipped[1] else None


def _union_ms(intervals: list[tuple[int, int]]) -> int:
    if not intervals:
        return 0
    ordered = sorted(intervals)
    total = 0
    start, end = ordered[0]
    for next_start, next_end in ordered[1:]:
        if next_start <= end:
            end = max(end, next_end)
        else:
            total += end - start
            start, end = next_start, next_end
    return total + end - start


def _subtract(base: tuple[int, int], exclusions: list[tuple[int, int]]) \
        -> list[tuple[int, int]]:
    relevant = sorted((max(base[0], start), min(base[1], end))
                      for start, end in exclusions
                      if max(base[0], start) <= min(base[1], end))
    result = []
    cursor = base[0]
    for start, end in relevant:
        if start > cursor:
            result.append((cursor, start))
        cursor = max(cursor, end)
    if cursor < base[1]:
        result.append((cursor, base[1]))
    return result


def _tool_outcome(value: str | None) -> str:
    if value == "completed":
        return "completed"
    if value == "failed":
        return "failed"
    if value == "denied":
        return "rejected"
    if value in ("cancelled", "superseded", "interrupted"):
        return "interrupted"
    return "unknown"


class CodexUsage:
    def __init__(self, config: InstanceConfig, db: Database):
        self._config = config
        self._db = db

    def repositories(self, repositories: list[dict], range_key: str,
                     now_ms: int | None = None) -> dict[str, Any]:
        now_ms = now_ms or int(time.time() * 1000)
        rows = []
        for repository in repositories:
            report = self.repository(repository, range_key, now_ms,
                                     resolve_missing=False)
            rows.append({
                "repository_id": report["repository_id"],
                "display_name": report["display_name"],
                "range": report["range"],
                "coverage": report["coverage"],
                "total_tokens": report["totals"]["total_tokens"],
                "model_requests": report["totals"]["model_requests"],
                "tool_calls": report["totals"]["tool_calls"],
                "execution_wall_ms": report["time"]["execution_wall"]["measured_ms"],
            })
        return {"range": range_key, "generated_at_ms": now_ms, "repositories": rows}

    def repository(self, repository: dict, range_key: str,
                   now_ms: int | None = None, *,
                   resolve_missing: bool = True) -> dict[str, Any]:
        now_ms = now_ms or int(time.time() * 1000)
        start_ms, end_ms, bucket_ms, bucket_count = _window(range_key, now_ms)
        return self._repository_window(
            repository, range_key, now_ms, start_ms, end_ms, bucket_ms,
            bucket_count, resolve_missing=resolve_missing)

    def repository_buckets(self, repository: dict, label: str,
                           bucket_ms: int, bucket_count: int,
                           now_ms: int | None = None, *,
                           aligned_end_ms: int | None = None,
                           resolve_missing: bool = True) -> dict[str, Any]:
        """Read an explicit bounded window for another repository dashboard."""
        if bucket_ms < 60_000 or not (1 <= bucket_count <= 400):
            raise ValueError("invalid Codex usage bucket window")
        now_ms = now_ms or int(time.time() * 1000)
        aligned_end = aligned_end_ms or (
            (now_ms + bucket_ms - 1) // bucket_ms) * bucket_ms
        if aligned_end < now_ms or aligned_end - now_ms >= bucket_ms:
            raise ValueError("invalid aligned Codex usage window end")
        start_ms = aligned_end - bucket_ms * bucket_count
        return self._repository_window(
            repository, label, now_ms, start_ms, now_ms, bucket_ms,
            bucket_count, resolve_missing=resolve_missing)

    def _repository_window(self, repository: dict, range_key: str, now_ms: int,
                           start_ms: int, end_ms: int, bucket_ms: int,
                           bucket_count: int, *,
                           resolve_missing: bool) -> dict[str, Any]:
        reports = []
        failures: Counter = Counter()
        def read(source: CodexUsageSource) -> tuple[int | None, SourceReport | None,
                                                    str | None]:
            try:
                key = self._repository_key(
                    source, repository, now_ms, resolve_missing=resolve_missing)
                try:
                    report = self._read_source(
                        source, key, start_ms, end_ms, bucket_ms, bucket_count)
                except SourceUnavailable as exc:
                    if exc.reason != "mapping_unavailable":
                        raise
                    with self._db.transaction() as conn:
                        conn.execute(
                            "DELETE FROM codex_usage_repository_links"
                            " WHERE source_uid=? AND repository_id=?",
                            (source.uid, repository["repository_id"]),
                        )
                    key = self._repository_key(
                        source, repository, now_ms, resolve_missing=resolve_missing)
                    report = self._read_source(
                        source, key, start_ms, end_ms, bucket_ms, bucket_count)
                return source.uid, report, None
            except SourceUnavailable as exc:
                return None, None, exc.reason
        sources = self._config.codex_usage_sources
        if sources:
            # A Codex process may carry a large model/tool catalog. Resolve and
            # read sources one at a time so a cold repository mapping cannot
            # multiply that transient footprint across local accounts.
            with ThreadPoolExecutor(max_workers=1,
                                    thread_name_prefix="codex-usage-read") as pool:
                for uid, report, reason in pool.map(read, sources):
                    if report is not None and uid is not None:
                        reports.append((uid, report))
                    elif reason is not None:
                        failures[reason] += 1
        return self._combine(repository, range_key, now_ms, start_ms, bucket_ms,
                             bucket_count, reports, failures)

    def _repository_key(self, source: CodexUsageSource, repository: dict,
                        now_ms: int, *, resolve_missing: bool = True) -> str:
        rows = self._db.query(
            "SELECT codex_repository_id FROM codex_usage_repository_links"
            " WHERE source_uid=? AND repository_id=?",
            (source.uid, repository["repository_id"]),
        )
        if rows:
            return rows[0]["codex_repository_id"]
        if not resolve_missing:
            raise SourceUnavailable("mapping_pending")
        result = self._probe_repository(source, Path(repository["root_path"]), now_ms)
        with self._db.transaction() as conn:
            conn.execute(
                "INSERT OR REPLACE INTO codex_usage_repository_links(source_uid,"
                " repository_id, codex_repository_id, source_schema, taxonomy_version,"
                " resolved_at) VALUES(?,?,?,?,?,?)",
                (source.uid, repository["repository_id"], result["key"],
                 result["schema"], result["taxonomy"], _now()),
            )
        return result["key"]

    @staticmethod
    def _probe_repository(source: CodexUsageSource, root: Path, now_ms: int) -> dict:
        try:
            account = pwd.getpwuid(source.uid)
        except KeyError as exc:
            raise SourceUnavailable("source_unavailable") from exc
        if not source.executable.is_file() or not source.codex_home.is_dir():
            raise SourceUnavailable("source_unavailable")
        env = {
            "PATH": "/usr/bin:/bin",
            "HOME": account.pw_dir,
            "USER": account.pw_name,
            "LOGNAME": account.pw_name,
            "CODEX_HOME": str(source.codex_home),
            "LANG": "C.UTF-8",
        }
        argv = [str(source.executable), "usage", "--json", "--since", str(now_ms),
                "repo", "current"]
        if os.geteuid() == 0 and source.uid != 0:
            argv = ["setpriv", f"--reuid={source.uid}",
                    f"--regid={account.pw_gid}", "--init-groups", "--", *argv]
        elif source.uid != os.geteuid():
            raise SourceUnavailable("source_unavailable")
        try:
            proc = subprocess.run(argv, cwd=root, env=env, capture_output=True,
                                  timeout=SOURCE_TIMEOUT_SECONDS, check=False)
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise SourceUnavailable("source_unavailable") from exc
        if proc.returncode != 0 or len(proc.stdout) > SOURCE_OUTPUT_BYTES:
            raise SourceUnavailable("source_unavailable")
        try:
            payload = json.loads(proc.stdout.decode("utf-8"))
            key = payload["scope"]["id"]
            schema = int(payload["databaseSchemaVersion"])
            taxonomy = int(payload["taxonomyVersion"])
        except (UnicodeDecodeError, ValueError, KeyError, TypeError,
                json.JSONDecodeError) as exc:
            raise SourceUnavailable("source_unavailable") from exc
        if payload.get("schemaVersion") != 1 or payload.get("kind") != "usageSummary" \
                or payload.get("scope", {}).get("type") != "repository" \
                or not isinstance(key, str) or not REPOSITORY_KEY.fullmatch(key):
            raise SourceUnavailable("source_unavailable")
        return {"key": key, "schema": schema, "taxonomy": taxonomy}

    @staticmethod
    def _open_database(source: CodexUsageSource) -> sqlite3.Connection:
        path = source.codex_home / "usage" / "usage.sqlite3"
        try:
            info = path.lstat()
        except OSError as exc:
            raise SourceUnavailable("source_unavailable") from exc
        if path.is_symlink() or not path.is_file() or info.st_uid != source.uid:
            raise SourceUnavailable("source_unavailable")
        uri = f"file:{quote(str(path), safe='/')}?mode=ro"
        try:
            conn = sqlite3.connect(uri, uri=True, timeout=0.25)
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA query_only=ON")
            conn.execute("PRAGMA busy_timeout=250")
            deadline = time.monotonic() + QUERY_TIMEOUT_SECONDS
            conn.set_progress_handler(lambda: int(time.monotonic() > deadline), 10_000)
            return conn
        except sqlite3.Error as exc:
            raise SourceUnavailable("source_unavailable") from exc

    def _read_source(self, source: CodexUsageSource, repository_key: str,
                     start_ms: int, end_ms: int, bucket_ms: int,
                     bucket_count: int) -> SourceReport:
        conn = self._open_database(source)
        try:
            schema = int(conn.execute(
                "SELECT COALESCE(MAX(version),0) FROM _sqlx_migrations").fetchone()[0])
            taxonomy = int(conn.execute(
                "SELECT COALESCE(MAX(version),0) FROM taxonomy_versions").fetchone()[0])
            if schema != SUPPORTED_DATABASE_SCHEMA or taxonomy != SUPPORTED_TAXONOMY:
                raise SourceUnavailable("schema_unsupported")
            canonical = self._canonical_repository(conn, repository_key)
            family = self._repository_family(conn, canonical)
            placeholders = ",".join("?" for _ in family)
            if conn.execute(
                    f"SELECT 1 FROM repositories WHERE id IN ({placeholders}) LIMIT 1",
                    tuple(family)).fetchone() is None:
                raise SourceUnavailable("mapping_unavailable")
            return self._source_report(conn, family, schema, taxonomy,
                                       start_ms, end_ms, bucket_ms, bucket_count)
        except sqlite3.Error as exc:
            raise SourceUnavailable("source_unavailable") from exc
        finally:
            conn.close()

    @staticmethod
    def _canonical_repository(conn: sqlite3.Connection, key: str) -> str:
        current = key
        seen = set()
        while current not in seen and len(seen) < 64:
            seen.add(current)
            row = conn.execute(
                "SELECT target_repository_id FROM repository_merge_events"
                " WHERE source_repository_id=?", (current,)).fetchone()
            if row is None:
                return current
            current = row[0]
        raise SourceUnavailable("source_unavailable")

    @staticmethod
    def _repository_family(conn: sqlite3.Connection, canonical: str) -> set[str]:
        rows = conn.execute(
            "WITH RECURSIVE family(id) AS (SELECT ? UNION ALL"
            " SELECT merge.source_repository_id FROM repository_merge_events merge"
            " JOIN family ON merge.target_repository_id=family.id) SELECT id FROM family",
            (canonical,),
        ).fetchall()
        return {row[0] for row in rows}

    def _source_report(self, conn: sqlite3.Connection, family: set[str], schema: int,
                       taxonomy: int, start_ms: int, end_ms: int, bucket_ms: int,
                       bucket_count: int) -> SourceReport:
        report = SourceReport(schema, taxonomy,
                              phase_series=[Counter() for _ in range(bucket_count)],
                              bucket_coverage=["unobserved"] * bucket_count)
        placeholders = ",".join("?" for _ in family)
        params = [*family, end_ms, start_ms]
        effective = {row[0]: tuple(row[1:]) for row in conn.execute(
            "SELECT operation_id,phase,activity,activity_state,provenance"
            " FROM effective_classification_events")}
        sql = (
            "SELECT operation.id,operation.operation_kind,operation.agent_id,"
            " operation.started_at_ms,operation.phase,operation.activity,"
            " operation.activity_state,operation.attribution_provenance,"
            " terminal.occurred_at_ms,terminal.event_kind,tool.operation_family"
            " FROM operations operation LEFT JOIN operation_events terminal"
            " ON terminal.operation_id=operation.id AND terminal.terminal=1"
            " LEFT JOIN tool_invocations tool ON tool.operation_id=operation.id"
            f" WHERE EXISTS(SELECT 1 FROM repository_attributions attribution"
            f" WHERE attribution.operation_id=operation.id AND attribution.repository_id IN"
            f" ({placeholders})) AND operation.started_at_ms<?"
            " AND (terminal.occurred_at_ms IS NULL OR terminal.occurred_at_ms>?)"
        )
        operations = []
        by_id = {}
        for row in conn.execute(sql, params):
            phase, activity, state, provenance = effective.get(row["id"], (
                row["phase"], row["activity"], row["activity_state"],
                row["attribution_provenance"]))
            operation = dict(row)
            operation.update(phase=phase, activity=activity, activity_state=state,
                             provenance=provenance)
            operations.append(operation)
            by_id[row["id"]] = operation
            report.activity_operations[(phase, activity)] += 1
            report.activity_provenance[(phase, activity, provenance)] += 1
        report.operation_count = len(operations)
        report.model_request_count = sum(
            operation["operation_kind"] == "model_request" for operation in operations)
        report.tool_count = sum(operation["operation_kind"] in
                                ("local_tool", "hosted_tool", "activity_control")
                                for operation in operations)
        operation_ids = set(by_id)
        report.evidence = bool(operations)
        for operation in operations:
            started = operation["started_at_ms"]
            report.freshest_at_ms = max(report.freshest_at_ms or started, started)
            kind = operation["operation_kind"]
            if kind in ("local_tool", "hosted_tool", "activity_control"):
                report.tool_outcomes[_tool_outcome(operation["event_kind"])] += 1
                report.tool_families[operation["operation_family"] or "unknown"] += 1
        self._add_tokens(conn, report, by_id, family, start_ms, end_ms,
                         bucket_ms, bucket_count)
        self._add_coverage(conn, report, operation_ids, start_ms, end_ms,
                           bucket_ms, bucket_count)
        self._add_intervals(conn, report, operations, operation_ids,
                            start_ms, end_ms)
        return report

    @staticmethod
    def _add_tokens(conn: sqlite3.Connection, report: SourceReport,
                    by_id: dict[str, dict], family: set[str], start_ms: int,
                    end_ms: int, bucket_ms: int, bucket_count: int) -> None:
        placeholders = ",".join("?" for _ in family)
        categories = ",".join("?" for _ in TOKEN_CATEGORIES)
        rows = conn.execute(
            "SELECT token.category_path,token.token_count,token.coverage_state,"
            " token.observed_at_ms,COALESCE(request.operation_id,tool.operation_id) op"
            " FROM token_observations token LEFT JOIN model_requests request"
            " ON request.id=token.model_request_id LEFT JOIN tool_invocations tool"
            " ON tool.id=token.tool_invocation_id"
            f" WHERE token.repository_bucket IN ({placeholders})"
            f" AND token.category_path IN ({categories})"
            " AND token.measurement_provenance='provider_reported'"
            " AND token.observed_at_ms>=? AND token.observed_at_ms<?",
            (*family, *TOKEN_CATEGORIES, start_ms, end_ms),
        )
        for row in rows:
            operation = by_id.get(row["op"])
            if operation is None:
                continue
            report.evidence = True
            report.token_observations[row["coverage_state"]] += 1
            report.freshest_at_ms = max(report.freshest_at_ms or row["observed_at_ms"],
                                        row["observed_at_ms"])
            if row["token_count"] is None:
                continue
            report.tokens[row["category_path"]] = (
                report.tokens.get(row["category_path"], 0) + row["token_count"])
            if row["category_path"] != "total_tokens":
                continue
            index = _bucket_index(row["observed_at_ms"], start_ms, bucket_ms,
                                  bucket_count)
            if index is not None:
                report.phase_series[index][operation["phase"]] += row["token_count"]
                report.bucket_coverage[index] = (
                    "complete" if row["coverage_state"] == "complete"
                    and report.bucket_coverage[index] != "partial" else "partial")
            report.activities[(operation["phase"], operation["activity"])] += \
                row["token_count"]

    @staticmethod
    def _add_coverage(conn: sqlite3.Connection, report: SourceReport,
                      operation_ids: set[str], start_ms: int, end_ms: int,
                      bucket_ms: int, bucket_count: int) -> None:
        for row in conn.execute(
                "SELECT operation_id,coverage_state,occurred_at_ms FROM coverage_events"
                " WHERE occurred_at_ms>=? AND occurred_at_ms<?", (start_ms, end_ms)):
            if row["operation_id"] not in operation_ids:
                continue
            report.coverage_events[row["coverage_state"]] += 1
            report.evidence = True
            index = _bucket_index(row["occurred_at_ms"], start_ms, bucket_ms,
                                  bucket_count)
            if index is not None and row["coverage_state"] != "complete":
                report.bucket_coverage[index] = "partial"

    @staticmethod
    def _add_intervals(conn: sqlite3.Connection, report: SourceReport,
                       operations: list[dict], operation_ids: set[str],
                       start_ms: int, end_ms: int) -> None:
        waits: dict[str, list[tuple[int, int]]] = defaultdict(list)
        unknown_waits = set()
        for row in conn.execute(
                "SELECT span.operation_id,span.started_at_ms,ended.occurred_at_ms"
                " FROM activity_spans span LEFT JOIN activity_span_events ended"
                " ON ended.activity_span_id=span.id AND ended.event_kind='ended'"):
            if row["operation_id"] not in operation_ids:
                continue
            interval = _clip_interval(row["started_at_ms"], row["occurred_at_ms"],
                                      start_ms, end_ms)
            if interval is None:
                unknown_waits.add(row["operation_id"])
            else:
                waits[row["operation_id"]].append(interval)
        for operation in operations:
            interval = _clip_interval(operation["started_at_ms"],
                                      operation["occurred_at_ms"], start_ms, end_ms)
            active = operation["activity_state"] not in (
                "external_wait", "user_wait", "blocked_wait")
            if interval is None:
                report.execution_unknown += 1
                report.phase_unknown[operation["phase"]] += 1
                if operation["operation_kind"] == "model_request":
                    report.request_unknown += 1
                if active:
                    report.agent_unknown += 1
                continue
            report.execution_intervals.append(interval)
            report.phase_intervals[operation["phase"]].append(interval)
            if operation["operation_kind"] == "model_request":
                report.request_intervals.append(interval)
            if not active:
                continue
            if operation["id"] in unknown_waits or operation["agent_id"] is None:
                report.agent_unknown += 1
            else:
                report.agent_intervals[operation["agent_id"]].extend(
                    _subtract(interval, waits[operation["id"]]))

    def _combine(self, repository: dict, range_key: str, now_ms: int, start_ms: int,
                 bucket_ms: int, bucket_count: int,
                 reports: list[tuple[int, SourceReport]], failures: Counter) -> dict[str, Any]:
        tokens = Counter()
        token_coverage = Counter()
        coverage_events = Counter()
        series = [Counter() for _ in range(bucket_count)]
        bucket_coverage = ["unobserved"] * bucket_count
        activities = Counter()
        activity_operations = Counter()
        activity_provenance = Counter()
        outcomes = Counter()
        families = Counter()
        request_intervals = []
        execution_intervals = []
        agent_intervals: dict[str, list[tuple[int, int]]] = defaultdict(list)
        phase_intervals: dict[str, list[tuple[int, int]]] = defaultdict(list)
        request_unknown = execution_unknown = agent_unknown = 0
        phase_unknown = Counter()
        operation_count = model_requests = tool_calls = 0
        freshest = None
        contributed = 0
        for uid, report in reports:
            tokens.update(report.tokens)
            token_coverage.update(report.token_observations)
            coverage_events.update(report.coverage_events)
            for index, values in enumerate(report.phase_series):
                series[index].update(values)
                if report.bucket_coverage[index] == "partial":
                    bucket_coverage[index] = "partial"
                elif report.bucket_coverage[index] == "complete" \
                        and bucket_coverage[index] == "unobserved":
                    bucket_coverage[index] = "complete"
            activities.update(report.activities)
            activity_operations.update(report.activity_operations)
            activity_provenance.update(report.activity_provenance)
            outcomes.update(report.tool_outcomes)
            families.update(report.tool_families)
            request_intervals.extend(report.request_intervals)
            execution_intervals.extend(report.execution_intervals)
            for agent, intervals in report.agent_intervals.items():
                agent_intervals[f"{uid}:{agent}"].extend(intervals)
            for phase, intervals in report.phase_intervals.items():
                phase_intervals[phase].extend(intervals)
            request_unknown += report.request_unknown
            execution_unknown += report.execution_unknown
            agent_unknown += report.agent_unknown
            phase_unknown.update(report.phase_unknown)
            operation_count += report.operation_count
            model_requests += report.model_request_count
            tool_calls += report.tool_count
            freshest = max(filter(None, (freshest, report.freshest_at_ms)), default=None)
            contributed += int(report.evidence)
        configured = len(self._config.codex_usage_sources)
        available = len(reports)
        any_evidence = contributed > 0
        has_gaps = bool(failures) or any(state != "complete" for state in token_coverage) \
            or any(state != "complete" for state in coverage_events) \
            or any(report.evidence and (not report.token_observations
                                        or report.execution_unknown > 0
                                        or report.request_unknown > 0
                                        or report.agent_unknown > 0)
                   for _, report in reports)
        if available == 0:
            coverage_state = "unavailable"
        elif not any_evidence:
            coverage_state = "unobserved"
        elif available == configured and not has_gaps:
            coverage_state = "complete"
        else:
            coverage_state = "partial"
        if coverage_state != "complete":
            bucket_coverage = ["partial" if value != "unobserved" or failures else value
                               for value in bucket_coverage]
        total_tokens = tokens.get("total_tokens") if token_coverage else None
        total_for_share = total_tokens or 0
        activity_rows = []
        for phase, activity in sorted(set(activities) | set(activity_operations),
                                      key=lambda key: (-activities[key], key)):
            value = activities[(phase, activity)]
            activity_rows.append({
                "phase": phase, "activity": activity, "total_tokens": value,
                "share": value / total_for_share if total_for_share else None,
                "operations": activity_operations[(phase, activity)],
                "provenance": {provenance: count for (p, a, provenance), count
                               in activity_provenance.items() if p == phase and a == activity},
            })
        request_ms = (
            max(end for _, end in request_intervals)
            - min(start for start, _ in request_intervals)
            if request_intervals else 0
        )
        if any_evidence and not request_intervals and request_unknown == 0:
            request_unknown = 1
        execution_ms = _union_ms(execution_intervals)
        agent_ms = sum(_union_ms(intervals) for intervals in agent_intervals.values())
        phase_rows = [{
            "phase": phase,
            "measured_ms": _union_ms(phase_intervals[phase]),
            "unknown_intervals": phase_unknown[phase],
        } for phase in sorted(set(phase_intervals) | set(phase_unknown))]
        points = []
        for index, values in enumerate(series):
            point = {"bucket_start_ms": start_ms + index * bucket_ms,
                     "bucket_end_ms": start_ms + (index + 1) * bucket_ms,
                     "coverage": bucket_coverage[index],
                     "phases": {phase: values.get(phase, 0) for phase in PHASES}}
            point["total_tokens"] = sum(point["phases"].values())
            points.append(point)
        return {
            "repository_id": repository["repository_id"],
            "display_name": repository["display_name"],
            "range": range_key,
            "generated_at_ms": now_ms,
            "coverage": {
                "state": coverage_state, "has_gaps": coverage_state != "complete",
                "configured_collectors": configured, "available_collectors": available,
                "contributing_collectors": contributed,
                "freshest_at_ms": freshest,
                "events": dict(sorted(coverage_events.items())),
                "token_observations": dict(sorted(token_coverage.items())),
                "unavailable_reasons": dict(sorted(failures.items())),
                "database_schemas": sorted({report.database_schema for _, report in reports}),
                "taxonomy_versions": sorted({report.taxonomy_version for _, report in reports}),
            },
            "totals": {
                "total_tokens": total_tokens,
                "input_tokens": tokens.get("input_tokens"),
                "cached_input_tokens": tokens.get("input_tokens_details.cached_tokens"),
                "output_tokens": tokens.get("output_tokens"),
                "reasoning_tokens": tokens.get("output_tokens_details.reasoning_tokens"),
                "model_requests": model_requests, "tool_calls": tool_calls,
                "operations": operation_count,
            },
            "series": points,
            "activities": activity_rows,
            "time": {
                "request_to_delivery": {"measured_ms": request_ms,
                                         "unknown_intervals": request_unknown},
                "execution_wall": {"measured_ms": execution_ms,
                                   "unknown_intervals": execution_unknown},
                "summed_agent_active": {"measured_ms": agent_ms,
                                        "unknown_intervals": agent_unknown},
                "phases": phase_rows,
            },
            "tools": {
                "outcomes": [{"outcome": name, "count": count}
                             for name, count in sorted(outcomes.items())],
                "families": [{"family": name, "count": count}
                             for name, count in families.most_common(12)],
            },
            "semantics": {
                "tokens": "provider total_tokens only; cached input and reasoning are subsets",
                "time": "wall, execution, phase, agent, and tool durations are separate",
                "coverage": (
                    "partial and unavailable collectors never contribute synthetic zeroes"
                ),
            },
        }
