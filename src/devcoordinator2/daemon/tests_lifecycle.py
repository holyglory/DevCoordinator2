"""Immediate test lifecycle: one current slot per worktree, latest-start-wins.

States: running → passed | failed | timed-out | cancelled | interrupted |
superseded. No queued state exists. All durable run state lives in
repository-local files; the daemon only holds live handles.
"""

from __future__ import annotations

import logging
import os
import threading
import time
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2 import ids
from devcoordinator2.daemon import (
    capture,
    docker_cli,
    events,
    securefs,
    summary,
    systemd_unit,
    test_postgres,
    tests_support,
)
from devcoordinator2.daemon.gitinfo import GitResolveError, resolve_worktree
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.repoconfig import ConfigError, load_test_spec
from devcoordinator2.daemon.server import Caller
from devcoordinator2.daemon.tests_support import (
    _remove_containers,
    _write_containers,
    _write_env_file,
)
from devcoordinator2.paths import InstanceConfig, test_dir
from devcoordinator2.protocol import ProtocolError

_START_LOCK_WAIT = 10.0
_LAUNCH_VERIFY_WAIT = 10.0
_ENV_FILE = tests_support.ENV_FILE
log = logging.getLogger("devcoordinator2.tests")


def _now_iso() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


class _RunHandle:
    def __init__(self, run_id: str, test: str, unit: str, worktree_root: Path,
                 caller_uid: int, client: str, proc, out: capture.Drainer,
                 err: capture.Drainer, cgroup: Path | None, started_at: str):
        self.run_id = run_id
        self.test = test
        self.unit = unit
        self.worktree_root = worktree_root
        self.caller_uid = caller_uid
        self.caller_gid = 0  # set by lifecycle right after construction
        self.client = client
        self.proc = proc
        self.out = out
        self.err = err
        self.cgroup = cgroup
        self.started_at = started_at
        self.started_mono = time.monotonic()
        self.stop_reason: str | None = None  # 'cancelled' | 'superseded'
        self.final_status: str | None = None
        self.lock = threading.Lock()
        # fd to this run's own test directory; the final summary is written
        # through it so a stale writer can never clobber a successor run.
        self.dir_fd: int | None = None
        self.finalized = threading.Event()  # set after the summary write
        self.containers: list[str] = []  # exact full IDs owned by this run
        self.repository_id: str | None = None

    def close_dir_fd(self) -> None:
        with self.lock:
            fd, self.dir_fd = self.dir_fd, None
        if fd is not None:
            try:
                os.close(fd)
            except OSError:
                pass

    @property
    def summary_path(self) -> Path:
        return test_dir(self.worktree_root) / "summary.json"


class TestLifecycle:
    def __init__(self, config: InstanceConfig, registry: Registry):
        self._config = config
        self._registry = registry
        self._locks: dict[str, threading.Lock] = {}
        self._locks_guard = threading.Lock()
        self._runs: dict[str, _RunHandle] = {}  # worktree_id -> live handle

    # -- public operations -------------------------------------------------

    def start(self, path: Path, test_name: str | None,
              caller: Caller) -> dict:
        if caller.uid == 0:
            raise ProtocolError("test_start_failed",
                                "repository code never runs as root; "
                                "call as a non-root account")
        try:
            reg = self._registry.register(path, caller_uid=caller.uid,
                                          caller_gid=caller.gid)
        except GitResolveError as exc:
            raise ProtocolError("repository_not_found", str(exc)) from exc
        worktree_root = Path(reg.worktree_path)
        try:
            spec = load_test_spec(worktree_root, test_name)
        except ConfigError as exc:
            raise ProtocolError("repository_config_invalid", str(exc)) from exc

        lock = self._worktree_lock(reg.worktree_id)
        if not lock.acquire(timeout=_START_LOCK_WAIT):
            raise ProtocolError("worktree_busy",
                                "another start for this worktree is in progress")
        try:
            self._supersede_prior(reg.worktree_id, worktree_root)
            securefs.remove_test_dir(worktree_root)
            try:
                current = securefs.create_test_dir(
                    worktree_root, caller.uid, caller.gid)
            except securefs.SecureFsError as exc:
                raise ProtocolError("test_start_failed", str(exc)) from exc
            run = ids.run_id()
            unit = ids.unit_name(self._config.unit_prefix, reg.worktree_id,
                                 run[1:])
            started_at = _now_iso()
            client = caller.client_kind
            dir_fd = os.open(current, os.O_RDONLY | os.O_DIRECTORY)
            try:
                initial = summary.build(run, spec.name, "running", started_at,
                                        caller.uid, client)
                summary.write_atomic_at(dir_fd, initial,
                                        owner=(caller.uid, caller.gid))
                env = dict(spec.env)
                containers: list[str] = []
                if spec.postgres is not None:
                    labels = docker_cli.managed_labels(
                        instance=self._config.unit_prefix,
                        repository_id=reg.repository_id,
                        worktree_id=reg.worktree_id, run_id=run,
                        purpose="test", caller_uid=caller.uid, client=client,
                        session=caller.client_session, created_at=started_at,
                        data_class="disposable")
                    try:
                        pg = test_postgres.provision(spec.postgres, run_id=run,
                                                     labels=labels)
                    except docker_cli.DockerError as exc:
                        raise ProtocolError(
                            "test_start_failed",
                            f"ephemeral postgres failed: {exc}") from exc
                    containers.append(pg.container_id)
                    _write_containers(dir_fd, containers, (caller.uid, caller.gid))
                    env.update(pg.env())
                env_file = None
                if env:
                    _write_env_file(dir_fd, env, (caller.uid, caller.gid))
                    env_file = current / _ENV_FILE
                argv = systemd_unit.build_systemd_run_argv(
                    unit=unit, slice_name=self._config.slice_name,
                    uid=caller.uid, gid=caller.gid,
                    timeout_seconds=spec.timeout_seconds, cwd=spec.cwd,
                    env_file=env_file, command=spec.command,
                    scratch_dir=current / "scratch",
                )
                try:
                    proc = systemd_unit.spawn(argv)
                except OSError as exc:
                    _remove_containers(containers)
                    raise ProtocolError(
                        "test_start_failed",
                        f"cannot spawn systemd-run: {exc}") from exc
                out = capture.Drainer(proc.stdout, current / "stdout.log",
                                      owner=(caller.uid, caller.gid))
                err = capture.Drainer(proc.stderr, current / "stderr.log",
                                      owner=(caller.uid, caller.gid))
                out.start()
                err.start()
                handle = _RunHandle(run, spec.name, unit, worktree_root,
                                    caller.uid, client, proc, out, err, None,
                                    started_at)
                handle.caller_gid = caller.gid
                handle.dir_fd = dir_fd
                handle.containers = containers
                handle.repository_id = reg.repository_id
            except (OSError, ProtocolError) as exc:
                os.close(dir_fd)
                if isinstance(exc, ProtocolError):
                    raise
                raise ProtocolError("test_start_failed", str(exc)) from exc
            try:
                self._verify_launch(handle)
            except ProtocolError:
                handle.finalized.wait(10)
                handle.close_dir_fd()
                raise
            self._runs[reg.worktree_id] = handle
            threading.Thread(target=self._reap, args=(reg.worktree_id, handle),
                             daemon=True).start()
            events.publish("test.started", run_id=run, test=spec.name,
                           repository_id=reg.repository_id, worktree_id=reg.worktree_id,
                           caller_uid=caller.uid, client=client)
            return {
                "run_id": run,
                "repository_id": reg.repository_id,
                "worktree_id": reg.worktree_id,
                "test": spec.name,
                "status": "running",
                "unit": unit,
                "summary_path": str(handle.summary_path),
            }
        finally:
            lock.release()

    def status(self, path: Path, caller: Caller) -> dict:
        worktree_root, wt_id = self._resolve(path, caller)
        doc = summary.read(test_dir(worktree_root) / "summary.json")
        if doc is None:
            raise ProtocolError("test_not_found",
                                "no current test run for this worktree")
        handle = self._runs.get(wt_id)
        if handle is not None and doc["status"] == "running" \
                and handle.final_status is None:
            doc["stdout_bytes_observed"] = handle.out.counts.observed
            doc["stderr_bytes_observed"] = handle.err.counts.observed
        doc["summary_path"] = str(test_dir(worktree_root) / "summary.json")
        return doc

    def output(self, path: Path, stream: str, tail_bytes: int,
               caller: Caller) -> dict:
        worktree_root, _ = self._resolve(path, caller)
        current = test_dir(worktree_root)
        doc = summary.read(current / "summary.json")
        if doc is None:
            raise ProtocolError("test_not_found",
                                "no current test run for this worktree")
        log_path = current / f"{stream}.log"
        tail, truncated_before = capture.tail_file(log_path, tail_bytes)
        return {
            "run_id": doc["run_id"],
            "stream": stream,
            "tail": tail.decode("utf-8", errors="replace"),
            "tail_bytes": len(tail),
            "truncated_before_tail": truncated_before,
            "log_path": str(log_path),
        }

    def stop(self, path: Path, caller: Caller) -> dict:
        worktree_root, wt_id = self._resolve(path, caller)
        doc = summary.read(test_dir(worktree_root) / "summary.json")
        if doc is None:
            raise ProtocolError("test_not_found",
                                "no current test run for this worktree")
        handle = self._runs.get(wt_id)
        if handle is None or handle.final_status is not None \
                or doc["status"] != "running":
            latest = summary.read(test_dir(worktree_root) / "summary.json") or doc
            return {"run_id": doc["run_id"], "status": latest["status"],
                    "already_finished": True}
        self._terminate(handle, "cancelled")
        final = summary.read(handle.summary_path)
        return {"run_id": handle.run_id,
                "status": final["status"] if final else "cancelled"}

    def list_current(self) -> list[dict]:
        return tests_support.list_current(self._registry._db, self._runs)

    def current_summary_ref(self, path: Path, caller: Caller) -> dict | None:
        try:
            worktree_root, _ = self._resolve(path, caller)
        except ProtocolError:
            return None
        doc = summary.read(test_dir(worktree_root) / "summary.json")
        if doc is None:
            return None
        return {"status": doc["status"],
                "run_id": doc["run_id"],
                "summary_path": str(test_dir(worktree_root) / "summary.json")}

    # -- restart recovery --------------------------------------------------

    def recover(self) -> None:
        """Stop leftover units and mark stale running summaries interrupted.
        Never resurrects, migrates, or retries unfinished work."""
        pattern = f"{self._config.unit_prefix}-*.service"
        for unit in systemd_unit.list_matching_units(pattern):
            cgroup = systemd_unit.control_group_path(unit)
            try:
                systemd_unit.stop_unit(unit)
            except systemd_unit.SystemdError:
                pass
            systemd_unit.prove_cgroup_empty(cgroup)
            systemd_unit.reset_failed(unit)
        # Every test container of this instance is ephemeral by definition:
        # unfinished runs are interrupted, finished runs already cleaned up.
        try:
            leftovers = docker_cli.list_ids_by_labels({
                f"{docker_cli.LABEL_PREFIX}.instance": self._config.unit_prefix,
                f"{docker_cli.LABEL_PREFIX}.purpose": "test",
            })
        except docker_cli.DockerError as exc:
            log.warning("recovery: cannot list test containers: %s", exc)
            leftovers = []
        _remove_containers(leftovers)
        for worktree_path in self._registry.registered_worktree_paths():
            summary_path = test_dir(worktree_path) / "summary.json"
            doc = summary.read(summary_path)
            if doc is not None and doc["status"] == "running":
                doc["status"] = "interrupted"
                doc["finished_at"] = _now_iso()
                try:
                    stat = summary_path.stat()
                    summary.write_atomic(summary_path, doc,
                                         owner=(stat.st_uid, stat.st_gid))
                    try:
                        tests_support.record_history(
                            worktree_path, doc, (stat.st_uid, stat.st_gid))
                    except securefs.SecureFsError as exc:
                        log.warning("recovery: cannot record test history for %s: %s",
                                    worktree_path, exc)
                except OSError:
                    pass

    # -- internals ---------------------------------------------------------

    def _worktree_lock(self, wt_id: str) -> threading.Lock:
        with self._locks_guard:
            return self._locks.setdefault(wt_id, threading.Lock())

    def _resolve(self, path: Path, caller: Caller) -> tuple[Path, str]:
        try:
            info = resolve_worktree(path, run_as=(caller.uid, caller.gid))
        except GitResolveError as exc:
            raise ProtocolError("repository_not_found", str(exc)) from exc
        return info.worktree_root, ids.worktree_id(info.worktree_root)

    def _supersede_prior(self, wt_id: str, worktree_root: Path) -> None:
        handle = self._runs.pop(wt_id, None)
        if handle is not None and handle.final_status is None:
            self._terminate(handle, "superseded")
        # Units from a previous daemon life (or leaks): exact-prefix match only.
        pattern = ids.unit_glob(self._config.unit_prefix, wt_id)
        for unit in systemd_unit.list_matching_units(pattern):
            cgroup = systemd_unit.control_group_path(unit)
            try:
                systemd_unit.stop_unit(unit)
            except systemd_unit.SystemdError as exc:
                raise ProtocolError("unit_stop_failed", str(exc)) from exc
            if not systemd_unit.prove_cgroup_empty(cgroup):
                raise ProtocolError(
                    "unit_stop_failed",
                    f"cgroup of {unit} still has processes after stop")
            systemd_unit.reset_failed(unit)
        try:
            stale = docker_cli.list_ids_by_labels({
                f"{docker_cli.LABEL_PREFIX}.instance": self._config.unit_prefix,
                f"{docker_cli.LABEL_PREFIX}.purpose": "test",
                f"{docker_cli.LABEL_PREFIX}.worktree": wt_id,
            })
        except docker_cli.DockerError:
            stale = []
        _remove_containers(stale)

    def _terminate(self, handle: _RunHandle, reason: str) -> None:
        with handle.lock:
            if handle.final_status is not None:
                return
            handle.stop_reason = reason
        cgroup = systemd_unit.control_group_path(handle.unit)
        try:
            systemd_unit.stop_unit(handle.unit)
        except systemd_unit.SystemdError as exc:
            raise ProtocolError("unit_stop_failed", str(exc)) from exc
        if not systemd_unit.prove_cgroup_empty(cgroup):
            raise ProtocolError(
                "unit_stop_failed",
                f"cgroup of {handle.unit} still has processes after stop")
        # systemd killed the tree. Finalize now (first caller past the lock
        # wins; the reaper's attempt becomes a no-op) and wait for the actual
        # summary write to complete before returning.
        self._finalize(handle)
        handle.finalized.wait(10)

    def _verify_launch(self, handle: _RunHandle) -> None:
        """Return only once the unit's process exists (or already ran)."""
        deadline = time.monotonic() + _LAUNCH_VERIFY_WAIT
        uid_mismatch: tuple | None = None
        while time.monotonic() < deadline:
            props = systemd_unit.show_unit(
                handle.unit, ["ActiveState", "MainPID", "ExecMainStatus"])
            main_pid = int(props.get("MainPID") or 0)
            if props.get("ActiveState") == "active" and main_pid > 0:
                uids = systemd_unit.process_uids(main_pid)
                if uids is not None and any(u != handle.caller_uid for u in uids):
                    # systemd's child drops credentials between fork and exec;
                    # a transient root probe is expected. Only a persistent
                    # mismatch is a failure.
                    uid_mismatch = uids
                else:
                    handle.cgroup = systemd_unit.control_group_path(handle.unit)
                    return
            rc = handle.proc.poll()
            if rc is not None:
                self._classify_exited_launch(handle, rc)
                return
            time.sleep(0.05)
        self._terminate(handle, "cancelled")
        if uid_mismatch is not None:
            raise ProtocolError(
                "test_start_failed",
                f"unit process uids {uid_mismatch} != caller {handle.caller_uid}")
        raise ProtocolError("test_start_failed",
                            "unit did not become active in time")

    def _classify_exited_launch(self, handle: _RunHandle, rc: int) -> None:
        """systemd-run exited during launch verification: either the command
        already finished (valid fast run) or the launch itself failed."""
        handle.out.join(2)
        handle.err.join(2)
        props = systemd_unit.show_unit(handle.unit, ["Result", "ExecMainStatus"])
        unit_ran = bool(props.get("Result"))
        exec_status = int(props.get("ExecMainStatus") or 0)
        if rc == 0 or (unit_ran and 0 < exec_status < 200):
            return  # completed run; the reaper writes the terminal summary
        detail_bytes, _ = capture.tail_file(
            handle.worktree_root / ".devcoordinator/test/current/stderr.log",
            2048)
        systemd_unit.reset_failed(handle.unit)
        self._finalize(handle)  # leaves an honest terminal summary behind
        raise ProtocolError(
            "test_start_failed",
            f"launch failed (systemd-run exit {rc})",
            detail_bytes.decode("utf-8", errors="replace"))

    def _reap(self, wt_id: str, handle: _RunHandle) -> None:
        handle.proc.wait()
        handle.out.join(30)
        handle.err.join(30)
        self._finalize(handle)
        handle.finalized.wait(15)
        handle.close_dir_fd()
        systemd_unit.reset_failed(handle.unit)
        if self._runs.get(wt_id) is handle:
            self._runs.pop(wt_id, None)

    def _finalize(self, handle: _RunHandle) -> None:
        with handle.lock:
            if handle.final_status is not None:
                return
            rc = handle.proc.poll()
            props = systemd_unit.show_unit(handle.unit,
                                           ["Result", "ExecMainStatus"])
            result = props.get("Result", "")
            if handle.stop_reason is not None:
                status, exit_code = handle.stop_reason, None
            elif result == "timeout":
                status, exit_code = "timed-out", None
            elif rc == 0:
                status, exit_code = "passed", 0
            else:
                status, exit_code = "failed", rc
            handle.final_status = status
        try:
            _remove_containers(handle.containers)
            finished_at = _now_iso()
            duration = round(time.monotonic() - handle.started_mono, 3)
            out, err = handle.out.counts, handle.err.counts
            doc = summary.build(
                handle.run_id, handle.test, status, handle.started_at,
                handle.caller_uid, handle.client, finished_at=finished_at,
                duration_seconds=duration, exit_code=exit_code,
                stdout_observed=out.observed, stdout_retained=out.retained,
                stderr_observed=err.observed, stderr_retained=err.retained,
            )
            try:
                # Written through the run's own directory fd: if this run was
                # superseded and its directory replaced, the write fails with
                # ENOENT instead of clobbering the successor's summary.
                if handle.dir_fd is not None:
                    summary.write_atomic_at(
                        handle.dir_fd, doc,
                        owner=(handle.caller_uid, handle.caller_gid))
            except OSError:
                pass  # directory superseded underneath us; successor owns the slot
            try:
                tests_support.record_history(
                    handle.worktree_root, doc,
                    (handle.caller_uid, handle.caller_gid))
            except securefs.SecureFsError as exc:
                log.warning("cannot record test history for %s: %s",
                            handle.worktree_root, exc)
            events.publish("test.finished", run_id=handle.run_id, test=handle.test,
                           status=status, exit_code=exit_code,
                           repository_id=handle.repository_id,
                           duration_seconds=duration, caller_uid=handle.caller_uid,
                           client=handle.client, worktree=str(handle.worktree_root))
        finally:
            handle.finalized.set()
