"""Immediate test lifecycle: one current slot per worktree, latest-start-wins.

States: running → passed | failed | timed-out | cancelled | interrupted |
superseded. No queued state exists. All durable run state lives in
repository-local files; the daemon only holds live handles.
"""

from __future__ import annotations

import logging
import os
import subprocess
import threading
import time
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2 import ids
from devcoordinator2.check_evidence import EvidenceError, receipts_match, source_digest
from devcoordinator2.daemon import (
    capture,
    docker_cli,
    events,
    securefs,
    summary,
    systemd_unit,
    test_admission,
    test_postgres,
    tests_support,
)
from devcoordinator2.daemon.gitinfo import GitResolveError, resolve_worktree
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.repoconfig import (
    VALIDATION_TIERS,
    CheckSpec,
    ConfigError,
    TestSpec,
    load_test_spec,
)
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
_PLAN_FILE = tests_support.PLAN_FILE
_EXECUTOR_BINARY = (Path(__file__).resolve().parents[3] / "target" / "release"
                    / "devcoordinator2-executor")
_CHECK_PATH = "/usr/local/bin:/usr/bin:/bin"
log = logging.getLogger("devcoordinator2.tests")


def _now_iso() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


class _RunHandle:
    def __init__(self, run_id: str, test: str, unit: str, worktree_root: Path,
                 caller_uid: int, client: str, proc, out: capture.Drainer,
                 err: capture.Drainer, cgroup: Path | None, started_at: str,
                 *, proof: str, selection: tuple[str, ...],
                 origin_run_id: str | None, requested_tier: str,
                 readiness_eligible: bool):
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
        self.proof = proof
        self.selection = selection
        self.origin_run_id = origin_run_id
        self.requested_tier = requested_tier
        self.readiness_eligible = readiness_eligible
        self.stop_detail: str | None = None

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
    def __init__(self, config: InstanceConfig, registry: Registry, capacity=None,
                 test_logs=None):
        self._config = config
        self._registry = registry
        self._locks: dict[str, threading.Lock] = {}
        self._locks_guard = threading.Lock()
        self._runs: dict[str, _RunHandle] = {}  # worktree_id -> live handle
        self._admission = test_admission.TestAdmission(config.socket_path.parent)
        self._capacity = capacity
        self._test_logs = test_logs

    # -- public operations -------------------------------------------------

    def start(self, path: Path, test_name: str | None,
              caller: Caller, *, checks: tuple[str, ...] = (),
              tier: str = "release",
              retry_run_id: str | None = None,
              retry_check: str | None = None) -> dict:
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
        if (retry_run_id is None) != (retry_check is None):
            raise ProtocolError(
                "args_invalid", "retry_run_id and retry_check must be supplied together")
        if retry_check is not None and checks:
            raise ProtocolError(
                "args_invalid", "retry and explicit check selection are separate modes")
        if tier not in VALIDATION_TIERS:
            raise ProtocolError(
                "args_invalid", "tier must be development, pre-merge, or release")
        configured = self._configured_checks(spec)
        requested = (retry_check,) if retry_check is not None else checks
        try:
            source_fingerprint = source_digest(
                worktree_root, (caller.uid, caller.gid))
        except EvidenceError as exc:
            raise ProtocolError("test_start_failed", str(exc)) from exc
        origin = None
        if retry_run_id is not None:
            try:
                origin = tests_support.find_evidence(worktree_root, retry_run_id)
            except securefs.SecureFsError as exc:
                raise ProtocolError("test_start_failed", str(exc)) from exc
            self._validate_retry(
                origin, retry_run_id, retry_check, spec,
                source_fingerprint, configured)
            tier = origin["requested_tier"]
        selected = self._selected_closure(configured, requested, tier)
        self._validate_executables(configured, selected, spec.env,
                                   caller.uid, caller.gid)
        if not _EXECUTOR_BINARY.is_file() or not os.access(_EXECUTOR_BINARY, os.X_OK):
            raise ProtocolError(
                "test_start_failed",
                "Rust governed-test executor is unavailable; build the locked release"
                " target before starting tests")

        lock = self._worktree_lock(reg.worktree_id)
        if not lock.acquire(timeout=_START_LOCK_WAIT):
            raise ProtocolError("worktree_busy",
                                "another start for this worktree is in progress")
        admission_context = self._admission.start_guard()
        admission_entered = False
        try:
            try:
                admission = admission_context.__enter__()
                admission_entered = True
            except test_admission.TestsDraining as exc:
                raise ProtocolError("tests_draining", str(exc)) from exc
            self._supersede_prior(reg.worktree_id, worktree_root)
            securefs.remove_test_dir(worktree_root)
            try:
                current = securefs.create_test_dir(
                    worktree_root, caller.uid, caller.gid)
            except securefs.SecureFsError as exc:
                raise ProtocolError("test_start_failed", str(exc)) from exc
            run = ids.run_id()
            try:
                log_dir = securefs.create_test_log_run_dir(
                    worktree_root, run, caller.uid, caller.gid)
                executor_stdout, executor_stderr = (
                    securefs.create_test_executor_log_files(
                        worktree_root, run, caller.uid, caller.gid)
                )
            except securefs.SecureFsError as exc:
                securefs.remove_test_dir(worktree_root)
                try:
                    securefs.remove_test_log_run_dir(worktree_root, run)
                except securefs.SecureFsError:
                    pass
                raise ProtocolError("test_start_failed", str(exc)) from exc
            unit = ids.unit_name(self._config.unit_prefix, reg.worktree_id,
                                 run[1:])
            started_at = _now_iso()
            client = caller.client_kind
            dir_fd = os.open(current, os.O_RDONLY | os.O_DIRECTORY)
            admitted = False
            capacity_registered = False
            proc = None
            containers: list[str] = []
            try:
                proof = "retry" if retry_run_id is not None \
                    else ("selected" if requested else "complete")
                readiness_eligible = proof == "complete" and tier == "release"
                initial = summary.build(
                    run, spec.name, "running", started_at, caller.uid, client,
                    proof=proof, selection=requested,
                    origin_run_id=retry_run_id, requested_tier=tier)
                summary.write_atomic_at(dir_fd, initial,
                                        owner=(caller.uid, caller.gid))
                admission.started(run, unit)
                admitted = True
                admission_context.__exit__(None, None, None)
                admission_entered = False
                env = dict(spec.env)
                env.setdefault("PATH", _CHECK_PATH)
                if self._capacity is not None:
                    self._capacity.register_run(run, caller.uid)
                    capacity_registered = True
                    env["DEVCOORDINATOR_CAPACITY_SOCKET"] = str(
                        self._capacity.socket_path)
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
                plan = self._build_plan(
                    spec, configured, selected, run, worktree_root, current, log_dir,
                    source_fingerprint, requested, origin, retry_run_id, tier)
                tests_support.write_check_plan(
                    dir_fd, plan, (caller.uid, caller.gid))
                argv = systemd_unit.build_systemd_run_argv(
                    unit=unit, slice_name=self._config.slice_name,
                    uid=caller.uid, gid=caller.gid,
                    timeout_seconds=spec.timeout_seconds, cwd=worktree_root,
                    env_file=env_file,
                    command=(str(_EXECUTOR_BINARY), "run",
                             str(current / _PLAN_FILE)),
                    scratch_dir=current / "scratch",
                )
                try:
                    proc = systemd_unit.spawn(argv)
                except OSError as exc:
                    _remove_containers(containers)
                    raise ProtocolError(
                        "test_start_failed",
                        f"cannot spawn systemd-run: {exc}") from exc
                capture_failed = threading.Event()

                def stop_on_capture_failure() -> None:
                    if capture_failed.is_set():
                        return
                    capture_failed.set()
                    if proc is not None and proc.poll() is None:
                        try:
                            systemd_unit.stop_unit(unit)
                        except systemd_unit.SystemdError:
                            proc.terminate()

                out = capture.Drainer(
                    proc.stdout, executor_stdout, stop_on_capture_failure)
                err = capture.Drainer(
                    proc.stderr, executor_stderr, stop_on_capture_failure)
                out.start()
                err.start()
                handle = _RunHandle(run, spec.name, unit, worktree_root,
                                    caller.uid, client, proc, out, err, None,
                                    started_at, proof=proof,
                                    selection=requested,
                                    origin_run_id=retry_run_id,
                                    requested_tier=tier,
                                    readiness_eligible=readiness_eligible)
                handle.caller_gid = caller.gid
                handle.dir_fd = dir_fd
                handle.containers = containers
                handle.repository_id = reg.repository_id
            except (OSError, ProtocolError) as exc:
                if proc is not None and proc.poll() is None:
                    try:
                        systemd_unit.stop_unit(unit)
                    except systemd_unit.SystemdError:
                        proc.terminate()
                if admitted:
                    self._admission.finished(run)
                if capacity_registered and self._capacity is not None:
                    self._capacity.unregister_run(run)
                _remove_containers(containers)
                for stream_file in (executor_stdout, executor_stderr):
                    if not stream_file.closed:
                        stream_file.close()
                os.close(dir_fd)
                if proc is None:
                    try:
                        securefs.remove_test_log_run_dir(worktree_root, run)
                    except securefs.SecureFsError:
                        pass
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
                "proof": proof,
                "selection": list(requested),
                "origin_run_id": retry_run_id,
                "requested_tier": tier,
                "readiness_eligible": readiness_eligible,
                "unit": unit,
                "summary_ref": "summary.json",
            }
        finally:
            if admission_entered:
                admission_context.__exit__(None, None, None)
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
            if handle.dir_fd is not None:
                report = tests_support.read_check_report(handle.dir_fd)
                if report is not None:
                    doc.update(self._report_projection(report))
        self._sanitize_public_result(doc)
        if self._capacity is not None:
            doc["capacity"] = self._capacity.snapshot()
        return doc

    def stop(self, path: Path, caller: Caller,
             reason: str | None = None) -> dict:
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
        handle.stop_detail = reason
        self._terminate(handle, "cancelled")
        final = summary.read(handle.summary_path)
        return {"run_id": handle.run_id,
                "status": final["status"] if final else "cancelled"}

    def list_current(self) -> list[dict]:
        rows = tests_support.list_current(self._registry._db, self._runs)
        for row in rows:
            handle = self._runs.get(row["worktree_id"])
            if handle is not None and handle.final_status is None \
                    and handle.dir_fd is not None and row.get("status") == "running":
                report = tests_support.read_check_report(handle.dir_fd)
                if report is not None:
                    row.update(self._report_projection(report))
            self._sanitize_public_result(row)
        if self._capacity is not None:
            capacity = self._capacity.snapshot()
            for row in rows:
                row["capacity"] = capacity
        return rows

    def current_summary_ref(self, path: Path, caller: Caller) -> dict | None:
        try:
            worktree_root, _ = self._resolve(path, caller)
        except ProtocolError:
            return None
        doc = summary.read(test_dir(worktree_root) / "summary.json")
        if doc is None:
            return None
        return {"status": doc["status"], "run_id": doc["run_id"]}

    @staticmethod
    def _configured_checks(spec: TestSpec) -> tuple[CheckSpec, ...]:
        return spec.checks

    @staticmethod
    def _selected_closure(configured: tuple[CheckSpec, ...],
                          requested: tuple[str, ...],
                          tier: str) -> tuple[CheckSpec, ...]:
        by_name = {check.name: check for check in configured}
        if len(requested) != len(set(requested)):
            raise ProtocolError("args_invalid", "check selection contains duplicates")
        missing = [name for name in requested if name not in by_name]
        if missing:
            raise ProtocolError(
                "args_invalid", f"unknown checks: {', '.join(sorted(missing))}")
        tier_rank = VALIDATION_TIERS.index(tier)
        excluded = [name for name in requested
                    if VALIDATION_TIERS.index(by_name[name].tier) > tier_rank]
        if excluded:
            raise ProtocolError(
                "args_invalid",
                f"checks are outside requested {tier} tier: "
                f"{', '.join(sorted(excluded))}")
        if not requested:
            selected = tuple(check for check in configured
                             if VALIDATION_TIERS.index(check.tier) <= tier_rank)
            if not selected:
                raise ProtocolError(
                    "args_invalid", f"no checks are configured for the {tier} tier")
            return selected
        included = set(requested)
        pending = list(requested)
        while pending:
            check = by_name[pending.pop()]
            for dependency in (*check.after, *check.requires):
                if dependency not in included:
                    included.add(dependency)
                    pending.append(dependency)
        return tuple(check for check in configured if check.name in included
                     and VALIDATION_TIERS.index(check.tier) <= tier_rank)

    @staticmethod
    def _validate_executables(configured: tuple[CheckSpec, ...],
                              selected: tuple[CheckSpec, ...],
                              global_env: dict[str, str], uid: int,
                              gid: int) -> None:
        selected_names = {check.name for check in selected}
        for check in configured:
            if check.name not in selected_names:
                continue
            commands = tuple(command for command in (
                check.command, check.discover, check.case_command) if command is not None)
            for command in commands:
                executable = command[0]
                path_value = check.env.get("PATH", global_env.get("PATH", _CHECK_PATH))
                if "/" in executable:
                    candidates = [Path(executable) if Path(executable).is_absolute()
                                  else check.cwd / executable]
                else:
                    candidates = [Path(part) / executable
                                  for part in path_value.split(os.pathsep) if part]
                found = False
                for candidate in candidates:
                    argv = ["/usr/bin/test", "-x", str(candidate)]
                    if os.geteuid() == 0 and uid != 0:
                        argv = ["setpriv", f"--reuid={uid}", f"--regid={gid}",
                                "--init-groups", "--", *argv]
                    try:
                        proc = subprocess.run(
                            argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, timeout=5, check=False,
                            env={"PATH": _CHECK_PATH},
                        )
                    except (OSError, subprocess.TimeoutExpired):
                        continue
                    if proc.returncode == 0:
                        found = True
                        break
                if not found:
                    raise ProtocolError(
                        "test_start_failed",
                        f"check {check.name!r} executable {executable!r} is unavailable")

    @staticmethod
    def _validate_retry(origin: dict | None, origin_run_id: str,
                        retry_check: str | None, spec: TestSpec,
                        source_fingerprint: str,
                        configured: tuple[CheckSpec, ...]) -> None:
        if origin is None:
            raise ProtocolError(
                "test_start_failed", f"no completed evidence for run {origin_run_id}")
        if origin.get("proof") != "complete" or origin.get("selection"):
            raise ProtocolError(
                "test_start_failed", "a retry requires an original complete run")
        if origin.get("requested_tier") not in VALIDATION_TIERS:
            raise ProtocolError(
                "test_start_failed", "retry evidence has no valid validation tier")
        if origin.get("test") != spec.name:
            raise ProtocolError(
                "test_start_failed", "retry evidence belongs to another test")
        if origin.get("source_digest") != source_fingerprint \
                or origin.get("config_digest") != spec.config_digest:
            raise ProtocolError(
                "test_start_failed", "retry evidence is stale for current source or config")
        if retry_check not in {check.name for check in configured}:
            raise ProtocolError("args_invalid", f"unknown check: {retry_check}")
        previous = next((row for row in origin.get("checks", [])
                         if row.get("name") == retry_check), None)
        if previous is None or previous.get("status") != "failed":
            raise ProtocolError(
                "test_start_failed", "only a failed check from that complete run can retry")

    @staticmethod
    def _build_plan(spec: TestSpec, configured: tuple[CheckSpec, ...],
                    selected: tuple[CheckSpec, ...], run_id: str,
                    worktree_root: Path, current: Path, log_dir: Path,
                    source_fingerprint: str, requested: tuple[str, ...],
                    origin: dict | None, origin_run_id: str | None,
                    requested_tier: str) -> dict:
        selected_names = {check.name for check in selected}
        target_names = set(requested)
        previous = {row.get("name"): row for row in (origin or {}).get("checks", [])}
        reused: dict[str, list[dict]] = {}
        if origin is not None:
            for check in selected:
                row = previous.get(check.name)
                artifacts = row.get("artifacts", []) if isinstance(row, dict) else []
                expected_paths = [artifact.get("path") for artifact in artifacts
                                  if isinstance(artifact, dict)]
                if check.name in target_names or check.completion != "process" \
                        or check.command is None \
                        or not artifacts or tuple(expected_paths) != check.produces \
                        or row.get("status") not in ("passed", "reused") \
                        or not receipts_match(worktree_root, artifacts):
                    continue
                reused[check.name] = artifacts
        rows = []
        for check in configured:
            if check.name not in selected_names:
                continue
            row = {
                "name": check.name,
                "tier": check.tier,
                "role": check.role,
                "cwd": check.cwd.relative_to(worktree_root).as_posix(),
                "env": check.env,
                "after": list(check.after),
                "requires": list(check.requires),
                "invalidates": [name for name in check.invalidates
                                if name in selected_names],
                "completion": check.completion,
                "on_failure": check.on_failure,
                "produces": list(check.produces),
                "retained_artifacts": [
                    {
                        "name": artifact.name,
                        "path": artifact.path,
                        "max_bytes": artifact.max_bytes,
                    }
                    for artifact in check.retained_artifacts
                ],
                "diagnostic_sources": [
                    {"format": source.format, "path": source.path}
                    for source in check.diagnostic_sources
                ],
                "timeout_seconds": check.timeout_seconds,
            }
            if check.command is not None:
                row["command"] = list(check.command)
            elif check.discover is not None:
                row["discover"] = list(check.discover)
                row["case_command"] = list(check.case_command or ())
            else:
                row["cases"] = [
                    {"id": case.id, "args": list(case.args)} for case in check.cases]
                row["case_command"] = list(check.case_command or ())
            rows.append(row)
        proof = "retry" if origin_run_id is not None \
            else ("selected" if requested else "complete")
        return {
            "schema": 2,
            "run_id": run_id,
            "test": spec.name,
            "requested_tier": requested_tier,
            "readiness_eligible": proof == "complete" and requested_tier == "release",
            "proof": proof,
            "selection": list(requested),
            "origin_run_id": origin_run_id,
            "worktree_root": str(worktree_root),
            "current_dir": str(current),
            "log_dir": str(log_dir),
            "source_digest": source_fingerprint,
            "config_digest": spec.config_digest,
            "checks": rows,
            "reused": reused,
        }

    @staticmethod
    def _report_projection(report: dict) -> dict:
        checks = []
        all_checks = report.get("checks", [])
        if not isinstance(all_checks, list):
            all_checks = []
        for row in all_checks[:64]:
            if not isinstance(row, dict):
                continue
            projected = {key: row.get(key) for key in (
                "name", "tier", "role", "status", "started_at", "finished_at",
                "duration_seconds", "exit", "streams", "case_count",
            )}
            artifacts = row.get("artifacts", [])
            projected["artifacts"] = artifacts[:8] if isinstance(artifacts, list) else []
            projected["artifacts_truncated"] = isinstance(artifacts, list) \
                and len(artifacts) > 8
            retained = row.get("retained_artifacts", [])
            projected["retained_artifacts"] = (
                retained[:8] if isinstance(retained, list) else [])
            projected["retained_artifacts_truncated"] = (
                isinstance(retained, list) and len(retained) > 8)
            cases = row.get("cases", [])
            projected["cases"] = cases[:32] if isinstance(cases, list) else []
            projected["cases_truncated"] = bool(row.get("cases_truncated")) \
                or (isinstance(cases, list) and len(cases) > 32)
            checks.append(projected)
        failures = []
        all_failures = report.get("failure_index", [])
        if not isinstance(all_failures, list):
            all_failures = []
        for row in all_failures[:16]:
            if isinstance(row, dict):
                failures.append({key: row.get(key) for key in (
                    "check", "case", "status", "exit", "termination_reason", "source",
                    "error_category", "expected", "actual", "fingerprint", "occurrences",
                    "log_refs", "origin",
                )})
        return {
            "requested_tier": report.get("requested_tier"),
            "readiness_eligible": bool(report.get("readiness_eligible")),
            "proof": report.get("proof"),
            "selection": report.get("selection", []),
            "check_summary": report.get("counts", {}),
            "checks": checks,
            "checks_truncated": len(all_checks) > 64,
            "failure_index": failures,
            "failure_index_truncated": (
                bool(report.get("failure_index_truncated"))
                or len(all_failures) > len(failures)
            ),
            "source_changed": bool(report.get("source_changed")),
            "execution_capacity": report.get("capacity"),
            "capacity_wait_count": (
                report.get("capacity", {}).get("capacity_wait_count", 0)
                if isinstance(report.get("capacity"), dict) else 0),
            **TestLifecycle._aggregate_output_projection(all_checks),
        }

    @staticmethod
    def _sanitize_public_result(document: dict) -> None:
        """Remove legacy private paths and unclassified prose before a status reply."""
        for private_field in ("summary_path", "check_report_path", "unsafe_reason"):
            document.pop(private_field, None)
        for stream in ("stdout", "stderr"):
            document.pop(f"{stream}_bytes_retained", None)
            document.pop(f"{stream}_truncated", None)
        if document.get("termination_reason") not in {
                None, "operator_cancelled", "superseded", "timed_out", "interrupted"}:
            document.pop("termination_reason", None)
        for collection in ("checks", "failure_index"):
            rows = document.get(collection)
            if isinstance(rows, list):
                for row in rows:
                    if isinstance(row, dict):
                        row.pop("reason", None)
                        for stream in ("stdout", "stderr"):
                            row.pop(f"{stream}_bytes_retained", None)
                            row.pop(f"{stream}_truncated", None)

    @staticmethod
    def _aggregate_output_projection(checks: list) -> dict:
        result = {"stdout_bytes_observed": 0, "stderr_bytes_observed": 0}
        streams = []
        for row in checks:
            if not isinstance(row, dict):
                continue
            if isinstance(row.get("streams"), list):
                streams.extend(row["streams"])
            for case in row.get("cases", []) if isinstance(row.get("cases"), list) else []:
                if isinstance(case, dict) and isinstance(case.get("streams"), list):
                    streams.extend(case["streams"])
        for stream in ("stdout", "stderr"):
            result[f"{stream}_bytes_observed"] = sum(
                row.get("bytes", 0)
                for row in streams
                if isinstance(row, dict)
                and isinstance(row.get("log_ref"), dict)
                and row["log_ref"].get("stream") == stream
                and isinstance(row.get("bytes"), int)
            )
        return result

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
        self._admission.reset()

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
            handle.worktree_root / ".devcoordinator/test/current/executor-stderr.log",
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
            report = tests_support.read_check_report(handle.dir_fd) \
                if handle.dir_fd is not None else None
            if handle.stop_reason is not None:
                status, exit_code = handle.stop_reason, None
            elif result == "timeout":
                status, exit_code = "timed-out", None
            elif rc == 0 and report is not None and report.get("status") == "passed":
                status, exit_code = "passed", 0
            else:
                status, exit_code = "failed", rc
            wrapper_out, wrapper_err = handle.out.counts, handle.err.counts
            if wrapper_out.retained != wrapper_out.observed \
                    or wrapper_err.retained != wrapper_err.observed:
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
                stdout_observed=out.observed, stderr_observed=err.observed,
                proof=handle.proof, selection=handle.selection,
                origin_run_id=handle.origin_run_id,
                requested_tier=handle.requested_tier,
            )
            doc.update(
                termination_reason=("operator_cancelled" if handle.stop_detail else None),
                check_report_ref=tests_support.REPORT_FILE,
            )
            if report is not None:
                doc.update(self._report_projection(report))
                doc["check_report_ref"] = tests_support.REPORT_FILE
                try:
                    # Retry evidence must be durable before the terminal summary
                    # becomes observable to a new caller.
                    tests_support.record_evidence(
                        handle.worktree_root, report,
                        (handle.caller_uid, handle.caller_gid))
                except securefs.SecureFsError as exc:
                    log.warning("cannot record check evidence for %s: %s",
                                handle.worktree_root, exc)
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
            if self._test_logs is not None:
                self._test_logs.notify_run_finished(
                    handle.worktree_root, handle.run_id)
        finally:
            if self._capacity is not None:
                self._capacity.unregister_run(handle.run_id)
            self._admission.finished(handle.run_id)
            handle.finalized.set()
