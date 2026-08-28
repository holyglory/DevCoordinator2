"""Root-required end-to-end lifecycle tests against real systemd units."""

from __future__ import annotations

import json
import subprocess
import time
from pathlib import Path

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
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if json.loads(summary_path.read_text())["status"] == "interrupted":
            break
        time.sleep(0.3)
    doc = json.loads(summary_path.read_text())
    assert doc["status"] == "interrupted"
    assert _units() == []
    # Never resurrected: still interrupted after a grace period.
    time.sleep(2)
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
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline and _containers_with_label("run", second_run):
        time.sleep(0.5)
    assert _containers_with_label("run", second_run) == []
    assert _containers_with_label("instance", UNIT_PREFIX) == []
