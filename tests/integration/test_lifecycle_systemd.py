"""Root-required end-to-end lifecycle tests against real systemd units."""

from __future__ import annotations

import importlib.util
import json
import os
import select
import subprocess
import threading
from pathlib import Path

from devcoordinator2.daemon import tests_support
from devcoordinator2.daemon.test_admission import (
    DirectoryEvents,
    begin_drain,
    end_drain,
    read_activity,
)
from integration.helpers import (
    ROOT_ONLY,
    UNIT_PREFIX,
    _call,
    _request,
    _units,
    _wait_status,
    _write_config,
    call_as,
)

pytestmark = ROOT_ONLY
_INSTALL_SPEC = importlib.util.spec_from_file_location(
    "devcoordinator2_install_for_integration",
    Path(__file__).resolve().parents[2] / "scripts" / "install.py")
assert _INSTALL_SPEC and _INSTALL_SPEC.loader
install = importlib.util.module_from_spec(_INSTALL_SPEC)
_INSTALL_SPEC.loader.exec_module(install)


def test_pass_uid_and_bounded_output(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["id"]\n'
                  'timeout_seconds = 60\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    assert resp["result"]["status"] == "running"
    final = _wait_status(world, world.repo, {"passed", "failed"})
    assert final["status"] == "passed"
    assert final["exit_code"] == 0
    assert final["caller_uid"] == world.caller.pw_uid
    # Successful status carries no log text.
    assert "tail" not in final and "stdout" not in final
    out = _call(world, "test.output",
                {"path": str(world.repo), "stream": "stdout"})
    assert f"uid={world.caller.pw_uid}" in out["result"]["tail"]
    # Summary file is owned by the caller and valid JSON.
    sp = Path(final["summary_path"])
    assert sp.stat().st_uid == world.caller.pw_uid
    assert json.loads(sp.read_text())["status"] == "passed"
    history = tests_support.read_history(world.repo)
    assert history[-1]["run_id"] == final["run_id"]
    assert history[-1]["status"] == "passed"


def test_broken_command_terminal_failure(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["/nonexistent/prog"]\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"] is False
    assert resp["error"]["code"] == "test_start_failed"
    assert "queued" not in json.dumps(resp).lower()


def test_timeout_kills_whole_cgroup(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\n'
                  'command = ["sleep", "120"]\ntimeout_seconds = 2\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    final = _wait_status(world, world.repo, {"timed-out"}, timeout=40)
    assert final["exit_code"] is None
    assert _units() == []


def test_cancel(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    stop = _call(world, "test.stop", {"path": str(world.repo)})
    assert stop["ok"], stop
    assert stop["result"]["status"] == "cancelled"
    assert _units() == []


def test_supersession_latest_start_wins(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n')
    first = _call(world, "test.start", {"path": str(world.repo)})
    assert first["ok"], first
    second = _call(world, "test.start", {"path": str(world.repo)})
    assert second["ok"], second
    assert second["result"]["run_id"] != first["result"]["run_id"]
    active = _units()
    assert len(active) == 1
    assert second["result"]["unit"] in active[0]
    status = _call(world, "test.status", {"path": str(world.repo)})
    assert status["result"]["run_id"] == second["result"]["run_id"]
    _call(world, "test.stop", {"path": str(world.repo)})


def test_flooder_capped_but_counted(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\n'
                  'command = ["dd", "if=/dev/zero", "bs=64k", "count=128",'
                  ' "status=none"]\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    final = _wait_status(world, world.repo, {"passed", "failed"}, timeout=60)
    assert final["status"] == "passed"
    assert final["stdout_bytes_observed"] == 64 * 1024 * 128
    assert final["stdout_bytes_retained"] == 4 * 1024 * 1024
    assert final["stdout_truncated"] is True


def test_daemon_restart_marks_interrupted(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n')
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    summary_path = Path(resp["result"]["summary_path"])
    world.daemon.kill_hard()
    assert json.loads(summary_path.read_text())["status"] == "running"
    world.daemon.start()
    doc = _wait_status(world, world.repo, {"interrupted"}, timeout=30)
    assert doc["status"] == "interrupted"
    assert _units() == []
    # No unit remains that could resurrect the interrupted run.
    assert json.loads(summary_path.read_text())["status"] == "interrupted"


def test_root_caller_rejected(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["id"]\n')
    resp = call_as(0, 0, world.daemon.socket_path,
                   _request("test.start", {"path": str(world.repo)}))
    assert resp["ok"] is False
    assert resp["error"]["code"] == "test_start_failed"
    assert "root" in resp["error"]["message"]


# -- Phase 2: test-scoped ephemeral PostgreSQL ---------------------------------

def _containers_with_label(key: str, value: str) -> list[str]:
    out = subprocess.run(
        ["docker", "ps", "--all", "--no-trunc", "--quiet",
         "--filter", f"label=devcoordinator2.{key}={value}"],
        capture_output=True, text=True).stdout
    return [ln.strip() for ln in out.splitlines() if ln.strip()]


PG_TOML = ('schema = 1\n[test.unit]\n'
           'command = ["psql", "-v", "ON_ERROR_STOP=1", "-c",'
           ' "create table t(x int); insert into t values (42); select x from t"]\n'
           'timeout_seconds = 120\n[test.unit.postgres]\n'
           'image = "postgres:16-alpine"\ndatabase = "app_test"\nuser = "app"\n')

POSTGIS_IMAGE = (
    "postgis/postgis@sha256:"
    "993c1a5fed969dab3974deaa8a5dcd768151725490be0579ef421333dccd6341")
POSTGIS_TOML = ('schema = 1\n[test.unit]\n'
                'command = ["psql", "-v", "ON_ERROR_STOP=1", "-c",'
                ' "create extension if not exists postgis; select postgis_version()"]\n'
                'timeout_seconds = 180\n[test.unit.postgres]\n'
                f'image = "{POSTGIS_IMAGE}"\ndatabase = "app_test"\nuser = "app"\n')


def test_postgres_real_query_labels_secrecy_and_cleanup(world):
    _write_config(world.repo, world.caller, PG_TOML)
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    run_id = resp["result"]["run_id"]
    unit = resp["result"]["unit"]
    # The container exists, carries the exact run identity and attribution.
    owned = _containers_with_label("run", run_id)
    assert len(owned) == 1, owned
    labels = json.loads(subprocess.run(
        ["docker", "inspect", "--format", "{{json .Config.Labels}}", owned[0]],
        capture_output=True, text=True).stdout)
    assert labels["devcoordinator2.purpose"] == "test"
    assert labels["devcoordinator2.caller_uid"] == str(world.caller.pw_uid)
    assert labels["devcoordinator2.data"] == "disposable"
    assert labels["devcoordinator2.instance"] == UNIT_PREFIX
    # The password never enters the unit's public Environment property.
    env_prop = subprocess.run(["systemctl", "show", unit, "-p", "Environment"],
                              capture_output=True, text=True).stdout
    assert "PGPASSWORD" not in env_prop
    env_file = world.repo / ".devcoordinator" / "test" / "current" / "env"
    st = env_file.stat()
    assert st.st_mode & 0o777 == 0o600 and st.st_uid == world.caller.pw_uid
    final = _wait_status(world, world.repo, {"passed", "failed"}, timeout=120)
    out = _call(world, "test.output", {"path": str(world.repo),
                                       "stream": "stdout"})["result"]["tail"]
    err = _call(world, "test.output", {"path": str(world.repo),
                                       "stream": "stderr"})["result"]["tail"]
    assert final["status"] == "passed", (final, out, err)
    assert "42" in out
    # Summary and status carry no credentials; container is gone.
    assert "PGPASSWORD" not in json.dumps(final)
    assert _containers_with_label("run", run_id) == []


def test_digest_pinned_postgis_fixture_is_pulled_injected_and_removed(world):
    _write_config(world.repo, world.caller, POSTGIS_TOML)
    resp = _call(world, "test.start", {"path": str(world.repo)})
    assert resp["ok"], resp
    run_id = resp["result"]["run_id"]
    final = _wait_status(world, world.repo, {"passed", "failed"}, timeout=180)
    out = _call(world, "test.output", {"path": str(world.repo),
                                       "stream": "stdout"})["result"]["tail"]
    err = _call(world, "test.output", {"path": str(world.repo),
                                       "stream": "stderr"})["result"]["tail"]
    assert final["status"] == "passed", (final, out, err)
    assert "postgis_version" in out and "USE_GEOS=1" in out
    assert _containers_with_label("run", run_id) == []


def test_postgres_removed_on_supersession_and_recovery(world):
    slow = ('schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n'
            '[test.unit.postgres]\nimage = "postgres:16-alpine"\n')
    _write_config(world.repo, world.caller, slow)
    first = _call(world, "test.start", {"path": str(world.repo)})
    assert first["ok"], first
    first_run = first["result"]["run_id"]
    assert len(_containers_with_label("run", first_run)) == 1
    second = _call(world, "test.start", {"path": str(world.repo)})
    assert second["ok"], second
    second_run = second["result"]["run_id"]
    assert _containers_with_label("run", first_run) == []
    assert len(_containers_with_label("run", second_run)) == 1
    # Daemon dies; restart recovery removes the orphaned test container.
    world.daemon.kill_hard()
    assert len(_containers_with_label("run", second_run)) == 1
    world.daemon.start()
    # Recovery removes test containers before the daemon publishes its socket.
    assert _containers_with_label("run", second_run) == []
    assert _containers_with_label("instance", UNIT_PREFIX) == []


def _check_toml(name: str, code: str, *, after=(), requires=(),
                completion="process", on_failure="continue", produces=()) -> str:
    rows = [
        "[[test.complete.check]]",
        f"name = {json.dumps(name)}",
        f"command = {json.dumps(['/usr/bin/python3', '-c', code])}",
    ]
    if after:
        rows.append(f"after = {json.dumps(list(after))}")
    if requires:
        rows.append(f"requires = {json.dumps(list(requires))}")
    if completion != "process":
        rows.append(f"completion = {json.dumps(completion)}")
    if on_failure != "continue":
        rows.append(f"on_failure = {json.dumps(on_failure)}")
    if produces:
        rows.append(f"produces = {json.dumps(list(produces))}")
    return "\n".join(rows) + "\n"


def test_governed_graph_runs_all_ready_checks_and_collects_safe_failures(world):
    paths = {name: world.base / name for name in (
        "one-started", "two-started", "one-release", "two-release")}
    for path in paths.values():
        os.mkfifo(path)
        os.chmod(path, 0o666)
    started_fds = {name: os.open(paths[f"{name}-started"], os.O_RDWR | os.O_NONBLOCK)
                   for name in ("one", "two")}
    release_fds = {name: os.open(paths[f"{name}-release"], os.O_RDWR | os.O_NONBLOCK)
                   for name in ("one", "two")}
    accepted = set()
    failure = []

    def barrier():
        try:
            poller = select.poll()
            by_fd = {fd: name for name, fd in started_fds.items()}
            for fd in by_fd:
                poller.register(fd, select.POLLIN)
            while len(accepted) < 2:
                events = poller.poll(30_000)
                assert events
                for fd, _event in events:
                    if os.read(fd, 1) == b"1":
                        accepted.add(by_fd[fd])
            for name in ("one", "two"):
                os.write(release_fds[name], b"1")
        except Exception as exc:
            failure.append(exc)
        finally:
            for fd in [*started_fds.values(), *release_fds.values()]:
                os.close(fd)

    thread = threading.Thread(target=barrier)
    thread.start()
    config = "schema = 1\n[test.complete]\ntimeout_seconds = 120\n"
    for name in ("one", "two"):
        code = (
            "import os;"
            f"w=os.open({str(paths[f'{name}-started'])!r},os.O_WRONLY);"
            "os.write(w,b'1');os.close(w);"
            f"r=os.open({str(paths[f'{name}-release'])!r},os.O_RDONLY);"
            "assert os.read(r,1)==b'1';os.close(r)")
        config += _check_toml(name, code)
    config += _check_toml(
        "fails", "import sys;print('exact-check-failure',file=sys.stderr);raise SystemExit(7)")
    config += _check_toml("after", "pass", after=("fails",))
    config += _check_toml(
        "needs", "raise AssertionError('must not run')", requires=("fails",))
    _write_config(world.repo, world.caller, config)

    started = _call(world, "test.start", {"path": str(world.repo)})
    assert started["ok"], started
    final = _wait_status(world, world.repo, {"failed"}, timeout=120)
    thread.join(30)
    assert not failure and accepted == {"one", "two"}
    states = {row["name"]: row["status"] for row in final["checks"]}
    assert states == {
        "one": "passed", "two": "passed", "fails": "failed",
        "after": "passed", "needs": "not_meaningful",
    }
    assert [row["check"] for row in final["failure_index"]] == ["fails", "needs"]
    assert final["proof"] == "complete"
    failed_output = _call(world, "test.output", {
        "path": str(world.repo), "stream": "stderr", "check": "fails"})
    assert failed_output["ok"], failed_output
    assert failed_output["result"]["check"] == "fails"
    assert "exact-check-failure" in failed_output["result"]["tail"]


def test_event_completed_setup_stays_alive_for_dependent_check(world):
    artifact = ".devcoordinator/test/current/artifacts/browser-ready"
    setup = (
        "import json,os,signal;"
        "payload={'run_id':os.environ['DEVCOORDINATOR_RUN_ID'],"
        "'check':os.environ['DEVCOORDINATOR_CHECK_NAME'],'status':'passed'};"
        "os.write(int(os.environ['DEVCOORDINATOR_EVENT_FD']),"
        "(json.dumps(payload)+'\\n').encode());signal.pause()")
    dependent = (
        "import os;from pathlib import Path;"
        f"Path({artifact!r}).write_text('ready')")
    config = "schema = 1\n[test.complete]\ntimeout_seconds = 120\n"
    config += _check_toml("service", setup, completion="event")
    config += _check_toml(
        "browser", dependent, requires=("service",), produces=(artifact,))
    _write_config(world.repo, world.caller, config)

    started = _call(world, "test.start", {"path": str(world.repo)})
    assert started["ok"], started
    final = _wait_status(world, world.repo, {"passed", "failed"}, timeout=120)
    assert final["status"] == "passed", final
    checks = {row["name"]: row for row in final["checks"]}
    assert checks["service"]["status"] == "passed"
    assert checks["browser"]["artifacts"][0]["path"] == artifact
    assert _units() == []


def test_selection_and_failed_check_retry_remain_diagnostic(world):
    ignore = world.repo / ".gitignore"
    ignore.write_text(".devcoordinator/\nbuild.bin\n")
    subprocess.run(["chown", f"{world.caller.pw_uid}:{world.caller.pw_gid}", ignore],
                   check=True)
    build = "from pathlib import Path;Path('build.bin').write_text('exact-build')"
    verify = "from pathlib import Path;raise SystemExit(0 if Path('fix.flag').exists() else 9)"
    unrelated = (
        "from pathlib import Path;import os;"
        "Path(os.environ['DEVCOORDINATOR_CHECK_SCRATCH'],'ran').write_text('yes')")
    config = "schema = 1\n[test.complete]\ntimeout_seconds = 120\n"
    config += _check_toml("build", build, produces=("build.bin",))
    config += _check_toml("verify", verify, requires=("build",))
    config += _check_toml("unrelated", unrelated)
    _write_config(world.repo, world.caller, config)

    first = _call(world, "test.start", {"path": str(world.repo)})
    assert first["ok"], first
    failed = _wait_status(world, world.repo, {"failed"}, timeout=120)
    origin = failed["run_id"]
    retried = _call(world, "test.retry", {
        "path": str(world.repo), "run_id": origin, "check": "verify"})
    assert retried["ok"], retried
    retry_final = _wait_status(world, world.repo, {"failed"}, timeout=120)
    retry_states = {row["name"]: row["status"] for row in retry_final["checks"]}
    assert retry_states == {"build": "reused", "verify": "failed"}
    assert retry_final["proof"] == "diagnostic"
    assert retry_final["origin_run_id"] == origin

    fix = world.repo / "fix.flag"
    fix.write_text("fixed")
    subprocess.run(["chown", f"{world.caller.pw_uid}:{world.caller.pw_gid}", fix],
                   check=True)
    stale = _call(world, "test.retry", {
        "path": str(world.repo), "run_id": origin, "check": "verify"})
    assert stale["ok"] is False
    assert "stale" in stale["error"]["message"]
    still_retry = _call(world, "test.status", {"path": str(world.repo)})
    assert still_retry["result"]["run_id"] == retry_final["run_id"]

    selected = _call(world, "test.start", {
        "path": str(world.repo), "checks": ["verify"]})
    assert selected["ok"], selected
    selected_final = _wait_status(world, world.repo, {"passed", "failed"}, timeout=120)
    assert selected_final["status"] == "passed", selected_final
    assert selected_final["proof"] == "diagnostic"
    assert selected_final["selection"] == ["verify"]
    assert {row["name"] for row in selected_final["checks"]} == {"build", "verify"}

    complete = _call(world, "test.start", {"path": str(world.repo)})
    assert complete["ok"], complete
    complete_final = _wait_status(world, world.repo, {"passed", "failed"}, timeout=120)
    assert complete_final["status"] == "passed", complete_final
    assert complete_final["proof"] == "complete"
    assert {row["name"] for row in complete_final["checks"]} == {
        "build", "verify", "unrelated"}


def test_upgrade_drain_rejects_new_starts_and_stop_reason_is_operational(world):
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["sleep", "120"]\n')
    started = _call(world, "test.start", {"path": str(world.repo)})
    assert started["ok"], started
    activity = read_activity(world.base)
    assert [row["run_id"] for row in activity["active"]] == [started["result"]["run_id"]]
    lease = begin_drain(world.base, "test upgrade")
    try:
        refused = _call(world, "test.start", {"path": str(world.repo)})
        assert refused["ok"] is False
        assert refused["error"]["code"] == "tests_draining"
        status = _call(world, "test.status", {"path": str(world.repo)})
        assert status["result"]["run_id"] == started["result"]["run_id"]
        stopped = _call(world, "test.stop", {
            "path": str(world.repo), "reason": "operator cancelled for emergency upgrade"})
        assert stopped["ok"] and stopped["result"]["status"] == "cancelled"
    finally:
        end_drain(lease)
    final = _call(world, "test.status", {"path": str(world.repo)})["result"]
    assert final["status"] == "cancelled"
    assert final["termination_reason"] == "operator cancelled for emergency upgrade"
    assert read_activity(world.base)["active"] == []
    assert _units() == []


def test_repository_installer_drain_waits_then_restarts_and_reconnects(world):
    release_fifo = world.base / "release-test"
    os.mkfifo(release_fifo)
    os.chmod(release_fifo, 0o666)
    command = (
        "import os;"
        f"fd=os.open({str(release_fifo)!r},os.O_RDONLY);"
        "assert os.read(fd,1)==b'1';os.close(fd)")
    _write_config(
        world.repo, world.caller,
        "schema = 1\n[test.unit]\n"
        f"command = {json.dumps(['/usr/bin/python3', '-c', command])}\n")
    started = _call(world, "test.start", {"path": str(world.repo)})
    assert started["ok"], started
    entered = threading.Event()
    completed = threading.Event()
    failures = []

    def drain():
        try:
            with install.drain_active_tests(
                    socket_path=world.daemon.socket_path,
                    runtime_dir=world.base, unit_prefix=UNIT_PREFIX,
                    daemon_running=True):
                world.daemon.stop()
                world.daemon.start()
                entered.set()
            completed.set()
        except Exception as exc:
            failures.append(exc)

    with DirectoryEvents(world.base) as events:
        thread = threading.Thread(target=drain)
        thread.start()
        if not (world.base / "test-drain.json").exists():
            events.wait(10)
    assert not entered.is_set()
    refused = _call(world, "test.start", {"path": str(world.repo)})
    assert refused["ok"] is False and refused["error"]["code"] == "tests_draining"
    writer = os.open(release_fifo, os.O_WRONLY)
    os.write(writer, b"1")
    os.close(writer)
    thread.join(120)
    assert not failures and entered.is_set() and completed.is_set()
    assert not (world.base / "test-drain.json").exists()
    summary_path = world.repo / ".devcoordinator" / "test" / "current" / "summary.json"
    final = json.loads(summary_path.read_text())
    assert final["status"] == "passed", final
    reconnected = _call(world, "test.status", {"path": str(world.repo)})
    assert reconnected["ok"] and reconnected["result"]["run_id"] == final["run_id"]
    _write_config(world.repo, world.caller,
                  'schema = 1\n[test.unit]\ncommand = ["true"]\n')
    after_upgrade = _call(world, "test.start", {"path": str(world.repo)})
    assert after_upgrade["ok"], after_upgrade
    final_after_upgrade = _wait_status(
        world, world.repo, {"passed", "failed"}, timeout=120)
    assert final_after_upgrade["status"] == "passed", final_after_upgrade
