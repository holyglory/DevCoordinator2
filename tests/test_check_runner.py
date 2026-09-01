from __future__ import annotations

import asyncio
import os
import select
import subprocess
import sys
import threading
from pathlib import Path

from devcoordinator2.check_evidence import read_json_bounded, source_digest
from devcoordinator2.check_runner import CHECK_LOG_CAP_BYTES, Runner


def repository(tmp_path: Path) -> tuple[Path, Path]:
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".gitignore").write_text(".devcoordinator/\n")
    (repo / "source.txt").write_text("source\n")
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    subprocess.run(["git", "add", "."], cwd=repo, check=True)
    subprocess.run(
        ["git", "-c", "user.name=Test", "-c", "user.email=test@example.test",
         "commit", "-qm", "initial"], cwd=repo, check=True)
    current = repo / ".devcoordinator" / "test" / "current"
    for relative in ("artifacts", "scratch", "checks"):
        (current / relative).mkdir(parents=True, exist_ok=True)
    return repo, current


def check(name: str, code: str, *, after=(), requires=(), completion="process",
          on_failure="continue", produces=()) -> dict:
    return {
        "name": name,
        "command": [sys.executable, "-c", code],
        "cwd": None,
        "env": {},
        "after": list(after),
        "requires": list(requires),
        "completion": completion,
        "on_failure": on_failure,
        "produces": list(produces),
    }


def plan(repo: Path, current: Path, checks: list[dict]) -> dict:
    for row in checks:
        row["cwd"] = str(repo)
    return {
        "schema": 1,
        "run_id": "tparallel",
        "test": "complete",
        "proof": "complete",
        "selection": [],
        "origin_run_id": None,
        "worktree_root": str(repo),
        "current_dir": str(current),
        "source_digest": source_digest(repo),
        "config_digest": "a" * 64,
        "checks": checks,
        "reused": {},
    }


def test_all_independent_ready_checks_start_together(tmp_path):
    repo, current = repository(tmp_path)
    paths = {name: tmp_path / name for name in (
        "one-started", "two-started", "one-release", "two-release")}
    for path in paths.values():
        os.mkfifo(path)
    started_fds = {
        name: os.open(paths[f"{name}-started"], os.O_RDWR | os.O_NONBLOCK)
        for name in ("one", "two")
    }
    release_fds = {
        name: os.open(paths[f"{name}-release"], os.O_RDWR | os.O_NONBLOCK)
        for name in ("one", "two")
    }
    accepted = set()
    failure = []

    def release_both():
        try:
            poller = select.poll()
            by_fd = {fd: name for name, fd in started_fds.items()}
            for fd in by_fd:
                poller.register(fd, select.POLLIN)
            while len(accepted) < 2:
                events = poller.poll(10_000)
                assert events
                for fd, _event in events:
                    if os.read(fd, 1) == b"1":
                        accepted.add(by_fd[fd])
            assert accepted == {"one", "two"}
            for name in ("one", "two"):
                os.write(release_fds[name], b"1")
        except Exception as exc:  # containment failure is surfaced in the test thread
            failure.append(exc)
        finally:
            for fd in started_fds.values():
                os.close(fd)
            for fd in release_fds.values():
                os.close(fd)

    thread = threading.Thread(target=release_both)
    thread.start()
    clients = []
    for name in ("one", "two"):
        clients.append(check(name, (
            "import os; "
            f"w=os.open({str(paths[f'{name}-started'])!r},os.O_WRONLY);"
            "os.write(w,b'1');os.close(w);"
            f"r=os.open({str(paths[f'{name}-release'])!r},os.O_RDONLY);"
            "assert os.read(r,1)==b'1';os.close(r)")))
    result = asyncio.run(Runner(plan(repo, current, [
        *clients,
    ])).run())
    thread.join(10)
    assert not failure
    assert accepted == {"one", "two"}
    assert result == 0
    report = read_json_bounded(current / "check-report.json")
    assert report["counts"]["passed"] == 2


def test_failures_continue_and_success_dependencies_are_not_meaningful(tmp_path):
    repo, current = repository(tmp_path)
    checks = [
        check("fails", "raise SystemExit(7)"),
        check("independent", "pass"),
        check("after-failure", "pass", after=("fails",)),
        check("needs-success", "raise AssertionError('must not run')",
              requires=("fails",)),
    ]
    assert asyncio.run(Runner(plan(repo, current, checks)).run()) == 1
    report = read_json_bounded(current / "check-report.json")
    states = {row["name"]: row["status"] for row in report["checks"]}
    assert states == {
        "fails": "failed",
        "independent": "passed",
        "after-failure": "passed",
        "needs-success": "not_meaningful",
    }
    assert [row["check"] for row in report["failure_index"]] == [
        "fails", "needs-success"]


def test_identity_bound_event_keeps_setup_alive_for_dependents(tmp_path):
    repo, current = repository(tmp_path)
    artifact = ".devcoordinator/test/current/artifacts/browser-ready"
    event_code = (
        "import json,os,signal; "
        "payload={'run_id':os.environ['DEVCOORDINATOR_RUN_ID'],"
        "'check':os.environ['DEVCOORDINATOR_CHECK_NAME'],'status':'passed'}; "
        "os.write(int(os.environ['DEVCOORDINATOR_EVENT_FD']),"
        "(json.dumps(payload)+'\\n').encode()); signal.pause()"
    )
    dependent_code = (
        f"from pathlib import Path; Path({str(repo / artifact)!r}).write_text('ready')")
    checks = [
        check("service", event_code, completion="event"),
        check("browser", dependent_code, requires=("service",), produces=(artifact,)),
    ]
    assert asyncio.run(Runner(plan(repo, current, checks)).run()) == 0
    report = read_json_bounded(current / "check-report.json")
    states = {row["name"]: row for row in report["checks"]}
    assert states["service"]["status"] == "passed"
    assert states["browser"]["artifacts"][0]["path"] == artifact
    assert len(states["browser"]["artifacts"][0]["sha256"]) == 64


def test_wrong_identity_event_is_unsafe(tmp_path):
    repo, current = repository(tmp_path)
    event_code = (
        "import json,os; payload={'run_id':'stale','check':'event','status':'passed'}; "
        "os.write(int(os.environ['DEVCOORDINATOR_EVENT_FD']),"
        "(json.dumps(payload)+'\\n').encode())"
    )
    assert asyncio.run(Runner(plan(repo, current, [
        check("event", event_code, completion="event"),
    ])).run()) == 1
    report = read_json_bounded(current / "check-report.json")
    assert report["checks"][0]["status"] == "unsafe"
    assert "wrong run or check identity" in report["checks"][0]["reason"]


def test_source_change_invalidates_complete_proof(tmp_path):
    repo, current = repository(tmp_path)
    code = f"from pathlib import Path; Path({str(repo / 'source.txt')!r}).write_text('changed')"
    assert asyncio.run(Runner(plan(repo, current, [check("edit", code)])).run()) == 1
    report = read_json_bounded(current / "check-report.json")
    assert report["source_changed"] is True
    assert report["status"] == "failed"


def test_each_check_log_is_capped_while_the_stream_is_fully_drained(tmp_path):
    repo, current = repository(tmp_path)
    observed = CHECK_LOG_CAP_BYTES + 4096
    code = f"import sys;sys.stdout.buffer.write(b'x'*{observed});sys.stdout.flush()"
    assert asyncio.run(Runner(plan(repo, current, [check("noisy", code)])).run()) == 0
    report = read_json_bounded(current / "check-report.json")
    row = report["checks"][0]
    assert row["stdout_bytes_observed"] == observed
    assert row["stdout_bytes_retained"] == CHECK_LOG_CAP_BYTES
    assert row["stdout_truncated"] is True
    assert (current / "checks" / "noisy" / "stdout.log").stat().st_size == \
        CHECK_LOG_CAP_BYTES
