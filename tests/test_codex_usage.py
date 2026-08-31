import json
import os
import sqlite3
import threading
import time
from pathlib import Path

from devcoordinator2.daemon.codex_usage import CodexUsage, SourceReport
from devcoordinator2.daemon.db import Database
from devcoordinator2.paths import CodexUsageSource, InstanceConfig


def _source_database(codex_home: Path, *, schema: int = 4) -> tuple[str, str, int]:
    usage = codex_home / "usage"
    usage.mkdir(parents=True)
    path = usage / "usage.sqlite3"
    conn = sqlite3.connect(path)
    conn.executescript("""
        CREATE TABLE _sqlx_migrations(version INTEGER);
        CREATE TABLE taxonomy_versions(version INTEGER);
        CREATE TABLE repository_merge_events(
          source_repository_id TEXT, target_repository_id TEXT);
        CREATE TABLE repositories(id TEXT PRIMARY KEY);
        CREATE TABLE operations(
          id TEXT PRIMARY KEY, operation_kind TEXT, agent_id TEXT,
          started_at_ms INTEGER, phase TEXT, activity TEXT, activity_state TEXT,
          attribution_provenance TEXT);
        CREATE TABLE operation_events(
          operation_id TEXT, terminal INTEGER, occurred_at_ms INTEGER, event_kind TEXT);
        CREATE TABLE tool_invocations(
          id TEXT PRIMARY KEY, operation_id TEXT, operation_family TEXT);
        CREATE TABLE model_requests(id TEXT PRIMARY KEY, operation_id TEXT);
        CREATE TABLE repository_attributions(operation_id TEXT, repository_id TEXT);
        CREATE TABLE token_observations(
          category_path TEXT, token_count INTEGER, coverage_state TEXT,
          observed_at_ms INTEGER, model_request_id TEXT, tool_invocation_id TEXT,
          repository_bucket TEXT, measurement_provenance TEXT);
        CREATE TABLE coverage_events(
          operation_id TEXT, coverage_state TEXT, occurred_at_ms INTEGER);
        CREATE TABLE activity_spans(
          id TEXT PRIMARY KEY, operation_id TEXT, started_at_ms INTEGER);
        CREATE TABLE activity_span_events(
          activity_span_id TEXT, event_kind TEXT, occurred_at_ms INTEGER);
        CREATE TABLE effective_classification_events(
          operation_id TEXT, phase TEXT, activity TEXT, activity_state TEXT,
          provenance TEXT);
    """)
    conn.execute("INSERT INTO _sqlx_migrations VALUES(?)", (schema,))
    conn.execute("INSERT INTO taxonomy_versions VALUES(1)")
    source = "a" * 64
    canonical = "b" * 64
    conn.execute("INSERT INTO repository_merge_events VALUES(?,?)", (source, canonical))
    conn.executemany("INSERT INTO repositories VALUES(?)", [(source,), (canonical,)])
    now_ms = 1_788_000_000_000
    model_start = now_ms - 60_000
    conn.execute("INSERT INTO operations VALUES(?,?,?,?,?,?,?,?)", (
        "model-op", "model_request", "agent-private", model_start,
        "implementation", "coding", "model_active", "agent_declared"))
    conn.execute("INSERT INTO operation_events VALUES(?,?,?,?)", (
        "model-op", 1, model_start + 10_000, "completed"))
    conn.execute("INSERT INTO model_requests VALUES(?,?)", ("request-private", "model-op"))
    conn.execute("INSERT INTO repository_attributions VALUES(?,?)", ("model-op", source))
    conn.execute("INSERT INTO effective_classification_events VALUES(?,?,?,?,?)", (
        "model-op", "implementation", "coding", "model_active", "agent_declared"))
    conn.execute("INSERT INTO operations VALUES(?,?,?,?,?,?,?,?)", (
        "tool-op", "local_tool", "agent-private", model_start + 4_000,
        "testing", "integration_testing", "tool_active", "agent_declared"))
    conn.execute("INSERT INTO operation_events VALUES(?,?,?,?)", (
        "tool-op", 1, model_start + 8_000, "completed"))
    conn.execute("INSERT INTO tool_invocations VALUES(?,?,?)", (
        "tool-private", "tool-op", "execution"))
    conn.execute("INSERT INTO repository_attributions VALUES(?,?)", ("tool-op", source))
    conn.execute("INSERT INTO effective_classification_events VALUES(?,?,?,?,?)", (
        "tool-op", "testing", "integration_testing", "tool_active", "agent_declared"))
    for category, count in [
        ("total_tokens", 100), ("input_tokens", 80),
        ("input_tokens_details.cached_tokens", 50), ("output_tokens", 20),
        ("output_tokens_details.reasoning_tokens", 10),
    ]:
        conn.execute("INSERT INTO token_observations VALUES(?,?,?,?,?,?,?,?)", (
            category, count, "complete", model_start + 10_000, "request-private", None,
            source, "provider_reported"))
    for operation in ("model-op", "tool-op"):
        conn.execute("INSERT INTO coverage_events VALUES(?,?,?)", (
            operation, "complete", model_start + 9_000))
    conn.execute("INSERT INTO activity_spans VALUES(?,?,?)", (
        "wait-private", "model-op", model_start + 2_000))
    conn.execute("INSERT INTO activity_span_events VALUES(?,?,?)", (
        "wait-private", "ended", model_start + 3_000))
    conn.commit()
    conn.close()
    return source, canonical, now_ms


def _world(tmp_path: Path, source: CodexUsageSource) -> tuple[CodexUsage, Database, dict]:
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock", state_dir=tmp_path / "state",
        unit_prefix="devcoordinator2-test", slice_name="tests.slice", client_group="",
        codex_usage_sources=(source,),
    )
    db = Database(config.database_path)
    repository = {
        "repository_id": "r0123456789abcdef", "display_name": "Example",
        "root_path": str(tmp_path / "repo"),
    }
    Path(repository["root_path"]).mkdir()
    with db.transaction() as conn:
        conn.execute("INSERT INTO repositories VALUES(?,?,?,?,?,?)", (
            repository["repository_id"], repository["root_path"],
            repository["display_name"], "t", os.getuid(), "t"))
    return CodexUsage(config, db), db, repository


def test_repository_report_uses_provider_total_and_merges_identity(tmp_path):
    codex_home = tmp_path / "codex-home"
    _, canonical, now_ms = _source_database(codex_home)
    source = CodexUsageSource(os.getuid(), codex_home, tmp_path / "codex")
    usage, db, repository = _world(tmp_path, source)
    with db.transaction() as conn:
        conn.execute(
            "INSERT INTO codex_usage_repository_links VALUES(?,?,?,?,?,?)",
            (os.getuid(), repository["repository_id"], canonical, 4, 1, "t"))

    report = usage.repository(repository, "24h", now_ms)

    assert report["coverage"]["state"] == "complete"
    assert report["totals"] == {
        "total_tokens": 100,
        "input_tokens": 80,
        "cached_input_tokens": 50,
        "output_tokens": 20,
        "reasoning_tokens": 10,
        "model_requests": 1,
        "tool_calls": 1,
        "operations": 2,
    }
    assert sum(point["phases"]["implementation"] for point in report["series"]) == 100
    coding = next(row for row in report["activities"] if row["activity"] == "coding")
    assert coding["total_tokens"] == 100 and coding["share"] == 1.0
    assert report["time"]["request_to_delivery"]["measured_ms"] == 10_000
    assert report["time"]["execution_wall"]["measured_ms"] == 10_000
    assert report["time"]["summed_agent_active"]["measured_ms"] == 9_000
    assert report["tools"]["outcomes"] == [{"outcome": "completed", "count": 1}]
    rendered = json.dumps(report)
    for prohibited in (str(codex_home), canonical, "agent-private", "request-private",
                       "tool-private", "wait-private"):
        assert prohibited not in rendered
    db.close()


def test_repository_report_supports_an_explicit_aligned_bucket_window(tmp_path):
    codex_home = tmp_path / "codex-home"
    _, canonical, now_ms = _source_database(codex_home)
    source = CodexUsageSource(os.getuid(), codex_home, tmp_path / "codex")
    usage, db, repository = _world(tmp_path, source)
    with db.transaction() as conn:
        conn.execute(
            "INSERT INTO codex_usage_repository_links VALUES(?,?,?,?,?,?)",
            (os.getuid(), repository["repository_id"], canonical, 4, 1, "t"))
    bucket_ms = 24 * 60 * 60 * 1000
    aligned_end = ((now_ms + bucket_ms - 1) // bucket_ms) * bucket_ms
    report = usage.repository_buckets(
        repository, "progress-day", bucket_ms, 14, now_ms,
        aligned_end_ms=aligned_end)
    assert report["range"] == "progress-day"
    assert len(report["series"]) == 14
    assert report["series"][0]["bucket_start_ms"] == aligned_end - 14 * bucket_ms
    assert sum(point["total_tokens"] for point in report["series"]) == 100
    db.close()


def test_unsupported_source_is_unavailable_not_zero(tmp_path):
    codex_home = tmp_path / "codex-home"
    _, canonical, now_ms = _source_database(codex_home, schema=5)
    source = CodexUsageSource(os.getuid(), codex_home, tmp_path / "codex")
    usage, db, repository = _world(tmp_path, source)
    with db.transaction() as conn:
        conn.execute("INSERT INTO codex_usage_repository_links VALUES(?,?,?,?,?,?)", (
            os.getuid(), repository["repository_id"], canonical, 5, 1, "t"))

    report = usage.repository(repository, "24h", now_ms)

    assert report["coverage"]["state"] == "unavailable"
    assert report["coverage"]["unavailable_reasons"] == {"schema_unsupported": 1}
    assert report["totals"]["total_tokens"] is None
    db.close()


def test_combining_sources_is_additive_for_tokens_and_union_for_wall_time(tmp_path):
    sources = (
        CodexUsageSource(1000, tmp_path / "one", tmp_path / "one-codex"),
        CodexUsageSource(1001, tmp_path / "two", tmp_path / "two-codex"),
    )
    config = InstanceConfig(
        socket_path=tmp_path / "s", state_dir=tmp_path / "state",
        unit_prefix="test", slice_name="test.slice", client_group="",
        codex_usage_sources=sources,
    )
    db = Database(config.database_path)
    usage = CodexUsage(config, db)
    first = SourceReport(4, 1, evidence=True, tokens={"total_tokens": 100},
                         token_observations={"complete": 1},
                         phase_series=[{"implementation": 100}],
                         bucket_coverage=["complete"],
                         execution_intervals=[(0, 10)], request_intervals=[(0, 10)])
    second = SourceReport(4, 1, evidence=True, tokens={"total_tokens": 50},
                          token_observations={"complete": 1},
                          phase_series=[{"testing": 50}], bucket_coverage=["complete"],
                          execution_intervals=[(5, 15)], request_intervals=[(5, 15)])

    report = usage._combine(
        {"repository_id": "r1", "display_name": "x"}, "24h", 15, 0, 100, 1,
        [(1000, first), (1001, second)], {},
    )

    assert report["coverage"]["state"] == "complete"
    assert report["totals"]["total_tokens"] == 150
    assert report["series"][0]["phases"]["implementation"] == 100
    assert report["series"][0]["phases"]["testing"] == 50
    assert report["time"]["execution_wall"]["measured_ms"] == 15
    db.close()


def test_repository_probe_uses_fixed_json_command(tmp_path):
    codex_home = tmp_path / "codex-home"
    codex_home.mkdir()
    executable = tmp_path / "codex"
    executable.write_text(
        "#!/usr/bin/env python3\n"
        "import json\n"
        "print(json.dumps({'schemaVersion': 1, 'kind': 'usageSummary',"
        " 'databaseSchemaVersion': 4, 'taxonomyVersion': 1,"
        " 'scope': {'type': 'repository', 'id': 'c' * 64}}))\n"
    )
    executable.chmod(0o700)
    source = CodexUsageSource(os.getuid(), codex_home, executable)

    result = CodexUsage._probe_repository(source, tmp_path, 1234)

    assert result == {"key": "c" * 64, "schema": 4, "taxonomy": 1}


def test_collection_reports_mapping_pending_without_blocking_probe(tmp_path):
    source = CodexUsageSource(
        os.getuid(), tmp_path / "missing-home", tmp_path / "missing-codex")
    usage, db, repository = _world(tmp_path, source)

    result = usage.repositories([repository], "24h", 1_788_000_000_000)

    coverage = result["repositories"][0]["coverage"]
    assert coverage["state"] == "unavailable"
    assert coverage["unavailable_reasons"] == {"mapping_pending": 1}
    db.close()


def test_on_demand_mapping_runs_one_heavy_probe_at_a_time(tmp_path, monkeypatch):
    sources = (
        CodexUsageSource(1000, tmp_path / "one", tmp_path / "one-codex"),
        CodexUsageSource(1001, tmp_path / "two", tmp_path / "two-codex"),
    )
    config = InstanceConfig(
        socket_path=tmp_path / "s", state_dir=tmp_path / "state",
        unit_prefix="test", slice_name="test.slice", client_group="",
        codex_usage_sources=sources,
    )
    db = Database(config.database_path)
    usage = CodexUsage(config, db)
    lock = threading.Lock()
    active = maximum = calls = 0

    def resolve(_source, _repository, _now_ms, *, resolve_missing=True):
        nonlocal active, maximum, calls
        assert resolve_missing is True
        with lock:
            active += 1
            calls += 1
            maximum = max(maximum, active)
        time.sleep(0.01)
        with lock:
            active -= 1
        return "a" * 64

    monkeypatch.setattr(usage, "_repository_key", resolve)
    def read(_source, _key, _start, _end, _bucket_ms, bucket_count):
        return SourceReport(
            4, 1, phase_series=[{} for _ in range(bucket_count)],
            bucket_coverage=["unobserved"] * bucket_count)

    monkeypatch.setattr(usage, "_read_source", read)
    usage.repository(
        {"repository_id": "r1", "root_path": "/one", "display_name": "one"},
        "24h", 1_788_000_000_000)

    assert calls == 2
    assert maximum == 1
    db.close()
