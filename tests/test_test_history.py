import os
from pathlib import Path

import pytest

from devcoordinator2.daemon import securefs, summary, tests_support


def _result(run_id: str, status: str = "passed") -> dict:
    return summary.build(
        run_id, "unit", status, "2026-08-30T10:00:00Z", os.getuid(), "codex",
        finished_at="2026-08-30T10:01:00Z", duration_seconds=60,
        exit_code=0 if status == "passed" else 1)


def test_history_records_terminal_runs_once_and_keeps_only_safe_fields(
        tmp_path: Path):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    owner = (os.getuid(), os.getgid())
    tests_support.record_history(tmp_path, _result("t-one"), owner)
    tests_support.record_history(tmp_path, _result("t-one"), owner)
    tests_support.record_history(tmp_path, _result("t-two", "failed"), owner)
    runs = tests_support.read_history(tmp_path)
    assert [run["run_id"] for run in runs] == ["t-one", "t-two"]
    assert set(runs[0]) == set(tests_support.HISTORY_FIELDS)
    assert "caller_uid" not in runs[0] and "client" not in runs[0]


def test_history_ignores_running_and_enforces_retention(
        tmp_path: Path, monkeypatch):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    owner = (os.getuid(), os.getgid())
    monkeypatch.setattr(tests_support, "HISTORY_CAP", 2)
    running = _result("t-running")
    running["status"] = "running"
    tests_support.record_history(tmp_path, running, owner)
    for run_id in ("t-one", "t-two", "t-three"):
        tests_support.record_history(tmp_path, _result(run_id), owner)
    assert [run["run_id"] for run in tests_support.read_history(tmp_path)] == [
        "t-two", "t-three"]


def test_history_refuses_malformed_existing_data(tmp_path: Path):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    securefs.write_test_history(
        tmp_path, b'{"schema":1,"runs":[{"run_id":"broken"}]}',
        (os.getuid(), os.getgid()))
    with pytest.raises(securefs.SecureFsError, match="invalid runs"):
        tests_support.read_history(tmp_path)
