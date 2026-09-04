import json
import os
from pathlib import Path

from devcoordinator2.daemon import summary


def _running(run="t20260822T000000Z-abc123"):
    return summary.build(
        run_id=run, test="unit", status="running",
        started_at="2026-08-22T00:00:00Z", caller_uid=1000, client="codex",
    )


def test_build_and_roundtrip(tmp_path: Path):
    path = tmp_path / "summary.json"
    doc = _running()
    summary.write_atomic(path, doc)
    loaded = summary.read(path)
    assert loaded == doc
    assert loaded["schema_version"] == 2
    assert loaded["stdout_bytes_observed"] == 0


def test_complete_stream_counts():
    doc = summary.build(
        run_id="t1", test="unit", status="failed",
        started_at="2026-08-22T00:00:00Z", caller_uid=1000, client="other",
        finished_at="2026-08-22T00:01:00Z", duration_seconds=60.0,
        exit_code=1, stdout_observed=10_000_000, stderr_observed=10,
    )
    assert doc["stdout_bytes_observed"] == 10_000_000
    assert doc["stderr_bytes_observed"] == 10


def test_replacement_is_atomic(tmp_path: Path):
    path = tmp_path / "summary.json"
    summary.write_atomic(path, _running())
    # Simulate a crash between temp-write and rename: a partial temp file
    # must never affect the readable summary.
    (tmp_path / ".summary-partial").write_text('{"schema_version": 1, "run')
    loaded = summary.read(path)
    assert loaded is not None
    assert loaded["status"] == "running"
    # Replace with terminal state; reader sees old-complete or new-complete.
    done = dict(_running())
    done.update(status="passed", finished_at="2026-08-22T00:00:05Z",
                duration_seconds=5.0, exit_code=0)
    summary.write_atomic(path, done)
    assert summary.read(path)["status"] == "passed"
    # No stray temp files besides the one we planted.
    stray = [p.name for p in tmp_path.iterdir()
             if p.name.startswith(".summary-") and p.name != ".summary-partial"]
    assert stray == []


def test_read_rejects_corrupt(tmp_path: Path):
    path = tmp_path / "summary.json"
    path.write_text("{not json")
    assert summary.read(path) is None
    path.write_text(json.dumps({"schema_version": 1, "status": "running"}))
    assert summary.read(path) is None
    old = _running()
    old["schema_version"] = 1
    path.write_text(json.dumps(old))
    assert summary.read(path) is None
    removed = _running()
    removed["stdout_bytes_retained"] = 4 * 1024 * 1024
    removed["stdout_truncated"] = True
    path.write_text(json.dumps(removed))
    assert summary.read(path) is None
    incoherent = _running()
    incoherent.update(proof="selected", selection=[], readiness_eligible=False)
    path.write_text(json.dumps(incoherent))
    assert summary.read(path) is None
    assert summary.read(tmp_path / "missing.json") is None


def test_write_at_fd_and_stale_write_cannot_clobber_successor(tmp_path: Path):
    """A finalize racing a supersession writes through the old run's own
    directory fd; once that directory is replaced, the write fails with
    ENOENT instead of overwriting the successor's summary."""
    import pytest

    from devcoordinator2.daemon import securefs

    old_dir = securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    dir_fd = os.open(old_dir, os.O_RDONLY | os.O_DIRECTORY)
    try:
        summary.write_atomic_at(dir_fd, _running("t-old"))
        assert summary.read(old_dir / "summary.json")["run_id"] == "t-old"

        # Supersession: old directory removed, successor created in its place.
        securefs.remove_test_dir(tmp_path)
        new_dir = securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
        summary.write_atomic_at(
            os.open(new_dir, os.O_RDONLY | os.O_DIRECTORY), _running("t-new"))

        stale = dict(_running("t-old"))
        stale.update(status="superseded", finished_at="2026-08-22T00:00:09Z")
        with pytest.raises(OSError):
            summary.write_atomic_at(dir_fd, stale)
        assert summary.read(new_dir / "summary.json")["run_id"] == "t-new"
    finally:
        os.close(dir_fd)


def test_owner_applied_when_permitted(tmp_path: Path):
    path = tmp_path / "summary.json"
    summary.write_atomic(path, _running(), owner=(os.getuid(), os.getgid()))
    assert path.stat().st_uid == os.getuid()
