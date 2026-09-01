"""Non-root governed-check graph runner.

All ready checks start concurrently. Dependencies, process completion, and a
dedicated inherited event descriptor are the only normal progression signals.
"""

from __future__ import annotations

import asyncio
import json
import os
import signal
import sys
import time
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2.check_evidence import (
    EvidenceError,
    artifact_receipts,
    read_json_bounded,
    source_digest,
    write_json_atomic,
)

PLAN_SCHEMA = 1
REPORT_SCHEMA = 1
EVENT_MAX_BYTES = 4096
CHECK_LOG_CAP_BYTES = 4 * 1024 * 1024
TERMINAL = frozenset({
    "passed", "failed", "not_meaningful", "cancelled", "unsafe", "reused",
})
SUCCESS = frozenset({"passed", "reused"})


def _iso_now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


class RunnerError(Exception):
    pass


class Runner:
    def __init__(self, plan: dict):
        self.plan = plan
        self.worktree = Path(plan["worktree_root"])
        self.current = Path(plan["current_dir"])
        self.report_path = self.current / "check-report.json"
        self.checks = {row["name"]: row for row in plan["checks"]}
        self.order = [row["name"] for row in plan["checks"]]
        self.states = {
            name: {
                "name": name,
                "status": "reused" if name in plan.get("reused", {}) else "pending",
                "started_at": None,
                "finished_at": None,
                "duration_seconds": 0.0 if name in plan.get("reused", {}) else None,
                "exit_code": None,
                "reason": "matching artifact evidence reused"
                if name in plan.get("reused", {}) else None,
                "artifacts": plan.get("reused", {}).get(name, []),
                "output_ref": f"checks/{name}",
                "stdout_bytes_observed": 0 if name in plan.get("reused", {}) else None,
                "stdout_bytes_retained": 0 if name in plan.get("reused", {}) else None,
                "stdout_truncated": False if name in plan.get("reused", {}) else None,
                "stderr_bytes_observed": 0 if name in plan.get("reused", {}) else None,
                "stderr_bytes_retained": 0 if name in plan.get("reused", {}) else None,
                "stderr_truncated": False if name in plan.get("reused", {}) else None,
            }
            for name in self.order
        }
        self.started_at = _iso_now()
        self.started_mono = time.monotonic()
        self.running: dict[str, asyncio.Task] = {}
        self.processes: dict[str, asyncio.subprocess.Process] = {}
        self.services: dict[str, dict] = {}
        self.aggregate_lock = asyncio.Lock()
        self.stopping = False
        self.unsafe_reason: str | None = None

    def _document(self, *, terminal: bool = False) -> dict:
        rows = [self.states[name] for name in self.order]
        failures = [{
            "check": row["name"], "status": row["status"],
            "reason": row["reason"], "output_ref": row["output_ref"],
        } for row in rows if row["status"] in {
            "failed", "not_meaningful", "cancelled", "unsafe",
        }]
        counts = {status: sum(row["status"] == status for row in rows)
                  for status in (*sorted(TERMINAL), "pending", "running")}
        complete = all(row["status"] in TERMINAL for row in rows)
        passed = complete and all(row["status"] in SUCCESS for row in rows)
        return {
            "schema": REPORT_SCHEMA,
            "run_id": self.plan["run_id"],
            "test": self.plan["test"],
            "proof": self.plan["proof"],
            "selection": self.plan.get("selection", []),
            "status": "passed" if terminal and passed else
                      "failed" if terminal else "running",
            "started_at": self.started_at,
            "finished_at": _iso_now() if terminal else None,
            "duration_seconds": round(time.monotonic() - self.started_mono, 3),
            "source_digest": self.plan["source_digest"],
            "config_digest": self.plan["config_digest"],
            "source_changed": False,
            "unsafe_reason": self.unsafe_reason,
            "counts": counts,
            "checks": rows,
            "failure_index": failures[:128],
            "failure_index_truncated": len(failures) > 128,
        }

    def write_report(self, *, terminal: bool = False) -> dict:
        document = self._document(terminal=terminal)
        write_json_atomic(self.report_path, document)
        return document

    async def run(self) -> int:
        if source_digest(self.worktree) != self.plan["source_digest"]:
            raise RunnerError("repository source changed before checks started")
        self.write_report()
        try:
            while True:
                self._mark_not_meaningful()
                ready = self._ready_names()
                for name in ready:
                    self.states[name].update(status="running", started_at=_iso_now())
                    self.write_report()
                    self.running[name] = asyncio.create_task(self._execute(name))
                if not self.running:
                    if all(self.states[name]["status"] in TERMINAL for name in self.order):
                        break
                    raise RunnerError("governed-check graph made no progress")

                waiters = set(self.running.values())
                service_waiters = {service["monitor"] for service in self.services.values()}
                done, _ = await asyncio.wait(
                    waiters | service_waiters, return_when=asyncio.FIRST_COMPLETED)
                for name, service in list(self.services.items()):
                    if service["monitor"] in done and not self.stopping:
                        rc = service["monitor"].result()
                        self.states[name].update(
                            status="unsafe", exit_code=rc,
                            finished_at=_iso_now(),
                            reason="long-lived check exited after its completion event",
                        )
                        self.unsafe_reason = f"required long-lived check {name} exited"
                        await self._cancel_running(except_name=name)
                        self._cancel_pending(self.unsafe_reason)
                for name, task in list(self.running.items()):
                    if task not in done:
                        continue
                    del self.running[name]
                    try:
                        result = task.result()
                    except Exception as exc:
                        result = self._result(
                            "unsafe", self.started_mono, None,
                            f"governed check failed internally: {type(exc).__name__}")
                    self.states[name].update(result)
                    self.write_report()
                    if result["status"] == "unsafe" \
                            or (result["status"] == "failed"
                                and self.checks[name]["on_failure"] == "stop"):
                        self.unsafe_reason = result["reason"] or f"{name} stopped the run"
                        await self._cancel_running(except_name=name)
                        self._cancel_pending(self.unsafe_reason)
            await self._stop_services()
            try:
                final_digest = source_digest(self.worktree)
            except EvidenceError as exc:
                final_digest = ""
                self.unsafe_reason = f"cannot verify final source: {exc}"
            document = self._document(terminal=True)
            if final_digest != self.plan["source_digest"]:
                document["source_changed"] = True
                document["status"] = "failed"
                document["failure_index"].append({
                    "check": None, "status": "failed",
                    "reason": "repository source changed during the run",
                    "output_ref": None,
                })
            write_json_atomic(self.report_path, document)
            return 0 if document["status"] == "passed" else 1
        except asyncio.CancelledError:
            await self._cancel_running()
            self._cancel_pending("run cancelled")
            await self._stop_services()
            self.write_report(terminal=True)
            raise

    def _dependencies_terminal(self, check: dict) -> bool:
        names = [*check["after"], *check["requires"]]
        return all(self.states[name]["status"] in TERMINAL for name in names)

    def _ready_names(self) -> list[str]:
        return [name for name in self.order
                if self.states[name]["status"] == "pending"
                and self._dependencies_terminal(self.checks[name])
                and all(self.states[dep]["status"] in SUCCESS
                        for dep in self.checks[name]["requires"])]

    def _mark_not_meaningful(self) -> None:
        changed = False
        for name in self.order:
            check = self.checks[name]
            if self.states[name]["status"] != "pending" \
                    or not self._dependencies_terminal(check):
                continue
            failed = [dep for dep in check["requires"]
                      if self.states[dep]["status"] not in SUCCESS]
            if not failed:
                continue
            self.states[name].update(
                status="not_meaningful", finished_at=_iso_now(),
                reason=f"required checks did not pass: {', '.join(failed)}",
            )
            changed = True
        if changed:
            self.write_report()

    def _cancel_pending(self, reason: str) -> None:
        for name in self.order:
            if self.states[name]["status"] == "pending":
                self.states[name].update(
                    status="cancelled", finished_at=_iso_now(), reason=reason)
        self.write_report()

    async def _execute(self, name: str) -> dict:
        check = self.checks[name]
        started = time.monotonic()
        check_dir = self.current / "checks" / name
        scratch = self.current / "scratch" / name
        check_dir.mkdir(parents=True, exist_ok=True)
        scratch.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ)
        env.update(check["env"])
        env.update({
            "DEVCOORDINATOR_RUN_ID": self.plan["run_id"],
            "DEVCOORDINATOR_CHECK_NAME": name,
            "DEVCOORDINATOR_CHECK_SCRATCH": str(scratch),
            "DEVCOORDINATOR_SHARED_ARTIFACTS": str(self.current / "artifacts"),
        })
        read_fd = write_fd = None
        if check["completion"] == "event":
            read_fd, write_fd = os.pipe2(os.O_CLOEXEC)
            os.set_inheritable(write_fd, True)
            env["DEVCOORDINATOR_EVENT_FD"] = str(write_fd)
        try:
            proc = await asyncio.create_subprocess_exec(
                *check["command"], cwd=check["cwd"], env=env,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                pass_fds=() if write_fd is None else (write_fd,),
                start_new_session=True,
            )
        except OSError as exc:
            if read_fd is not None:
                os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
            return self._result("failed", started, None, f"cannot start check: {exc}")
        self.processes[name] = proc
        if write_fd is not None:
            os.close(write_fd)
        pumps = [
            asyncio.create_task(self._pump(proc.stdout, check_dir / "stdout.log", 1)),
            asyncio.create_task(self._pump(proc.stderr, check_dir / "stderr.log", 2)),
        ]
        try:
            if check["completion"] == "process":
                rc = await proc.wait()
                output = await self._finish_pumps(pumps)
                status = "passed" if rc == 0 else "failed"
                reason = None if rc == 0 else f"process exited {rc}"
                artifacts = artifact_receipts(
                    self.worktree, check["produces"]) if rc == 0 else []
                return self._result(status, started, rc, reason, artifacts, output)

            event_task = asyncio.create_task(self._read_event(read_fd))
            exit_task = asyncio.create_task(proc.wait())
            done, _ = await asyncio.wait(
                {event_task, exit_task}, return_when=asyncio.FIRST_COMPLETED)
            if exit_task in done and event_task not in done:
                event_task.cancel()
                rc = exit_task.result()
                output = await self._finish_pumps(pumps)
                return self._result(
                    "failed", started, rc,
                    "process exited before its required completion event",
                    output=output)
            event = event_task.result()
            if event["run_id"] != self.plan["run_id"] or event["check"] != name:
                await self._terminate(proc)
                output = await self._finish_pumps(pumps)
                return self._result(
                    "unsafe", started, proc.returncode,
                    "completion event carried the wrong run or check identity",
                    output=output)
            status = event["status"]
            if status not in ("passed", "failed", "unsafe"):
                await self._terminate(proc)
                output = await self._finish_pumps(pumps)
                return self._result("unsafe", started, proc.returncode,
                                    "completion event carried an invalid status",
                                    output=output)
            if status != "passed":
                await self._terminate(proc)
                output = await self._finish_pumps(pumps)
                return self._result(status, started, proc.returncode,
                                    "completion event reported failure", output=output)
            if exit_task.done():
                rc = exit_task.result()
                output = await self._finish_pumps(pumps)
                if rc != 0:
                    return self._result("failed", started, rc,
                                        "event process exited nonzero", output=output)
                artifacts = artifact_receipts(self.worktree, check["produces"])
                return self._result("passed", started, 0, None, artifacts, output)
            artifacts = artifact_receipts(self.worktree, check["produces"])
            self.services[name] = {
                "name": name, "process": proc, "monitor": exit_task, "pumps": pumps,
            }
            return self._result("passed", started, None, None, artifacts)
        except (EvidenceError, RunnerError, json.JSONDecodeError,
                KeyError, TypeError, ValueError) as exc:
            await self._terminate(proc)
            output = await self._finish_pumps(pumps)
            return self._result(
                "failed", started, proc.returncode, str(exc), output=output)
        except asyncio.CancelledError:
            await self._terminate(proc)
            self.states[name].update(await self._finish_pumps(pumps))
            raise
        finally:
            self.processes.pop(name, None)
            if read_fd is not None:
                try:
                    os.close(read_fd)
                except OSError:
                    pass

    def _result(self, status: str, started: float, exit_code: int | None,
                reason: str | None, artifacts: list[dict] | None = None,
                output: dict | None = None) -> dict:
        result = {
            "status": status,
            "finished_at": _iso_now(),
            "duration_seconds": round(time.monotonic() - started, 3),
            "exit_code": exit_code,
            "reason": reason,
            "artifacts": artifacts or [],
        }
        if output is not None:
            result.update(output)
        return result

    async def _pump(self, stream: asyncio.StreamReader | None, path: Path,
                    aggregate_fd: int) -> dict:
        if stream is None:
            return {"observed": 0, "retained": 0, "truncated": False}
        observed = retained = 0
        with path.open("wb") as output:
            while True:
                block = await stream.read(65536)
                if not block:
                    return {"observed": observed, "retained": retained,
                            "truncated": observed > retained}
                observed += len(block)
                room = CHECK_LOG_CAP_BYTES - retained
                if room > 0:
                    keep = block[:room]
                    output.write(keep)
                    output.flush()
                    retained += len(keep)
                async with self.aggregate_lock:
                    written = 0
                    while written < len(block):
                        written += os.write(aggregate_fd, block[written:])

    async def _finish_pumps(self, pumps: list[asyncio.Task]) -> dict:
        values = await asyncio.gather(*pumps, return_exceptions=True)
        normalized = [value if isinstance(value, dict) else {
            "observed": 0, "retained": 0, "truncated": False,
        } for value in values]
        while len(normalized) < 2:
            normalized.append({"observed": 0, "retained": 0, "truncated": False})
        return {
            "stdout_bytes_observed": normalized[0]["observed"],
            "stdout_bytes_retained": normalized[0]["retained"],
            "stdout_truncated": normalized[0]["truncated"],
            "stderr_bytes_observed": normalized[1]["observed"],
            "stderr_bytes_retained": normalized[1]["retained"],
            "stderr_truncated": normalized[1]["truncated"],
        }

    async def _read_event(self, fd: int) -> dict:
        loop = asyncio.get_running_loop()
        future = loop.create_future()
        buffer = bytearray()

        def readable() -> None:
            try:
                block = os.read(fd, EVENT_MAX_BYTES + 1 - len(buffer))
                if not block:
                    raise RunnerError("completion event descriptor closed without an event")
                buffer.extend(block)
                if len(buffer) > EVENT_MAX_BYTES:
                    raise RunnerError("completion event exceeds 4096 bytes")
                if b"\n" not in buffer:
                    return
                line, trailing = bytes(buffer).split(b"\n", 1)
                if trailing:
                    raise RunnerError("completion event contains trailing data")
                value = json.loads(line)
                if not isinstance(value, dict):
                    raise RunnerError("completion event is not an object")
                loop.remove_reader(fd)
                future.set_result(value)
            except Exception as exc:
                loop.remove_reader(fd)
                if not future.done():
                    future.set_exception(exc)

        loop.add_reader(fd, readable)
        try:
            return await future
        finally:
            loop.remove_reader(fd)

    async def _terminate(self, proc: asyncio.subprocess.Process) -> None:
        if proc.returncode is not None:
            return
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        try:
            await asyncio.wait_for(proc.wait(), 10)
        except TimeoutError:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            await proc.wait()

    async def _cancel_running(self, except_name: str | None = None) -> None:
        tasks = []
        for name, task in list(self.running.items()):
            if name == except_name or task.done():
                continue
            task.cancel()
            tasks.append(task)
            self.states[name].update(
                status="cancelled", finished_at=_iso_now(),
                reason=self.unsafe_reason or "run cancelled")
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        self.running = {name: task for name, task in self.running.items()
                        if name == except_name and not task.done()}

    async def _stop_services(self) -> None:
        self.stopping = True
        for service in self.services.values():
            await self._terminate(service["process"])
        for service in self.services.values():
            output = await self._finish_pumps(service["pumps"])
            self.states[service["name"]].update(output)
            if not service["monitor"].done():
                await service["monitor"]
        self.services.clear()


def _load_plan(path: Path) -> dict:
    plan = read_json_bounded(path)
    if plan is None or plan.get("schema") != PLAN_SCHEMA:
        raise RunnerError("invalid governed-check plan")
    required = {
        "run_id", "test", "proof", "worktree_root", "current_dir",
        "source_digest", "config_digest", "checks",
    }
    if not required <= set(plan) or not isinstance(plan["checks"], list):
        raise RunnerError("governed-check plan is incomplete")
    return plan


async def _main(path: Path) -> int:
    runner = Runner(_load_plan(path))
    loop = asyncio.get_running_loop()
    task = asyncio.current_task()
    for signum in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(signum, task.cancel)
    try:
        return await runner.run()
    except (RunnerError, EvidenceError, KeyError, TypeError, ValueError) as exc:
        print(f"governed-check runner failed: {exc}", file=sys.stderr)
        return 2


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: governed-check-runner PLAN.json", file=sys.stderr)
        return 2
    return asyncio.run(_main(Path(sys.argv[1])))


if __name__ == "__main__":
    raise SystemExit(main())
