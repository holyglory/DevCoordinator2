"""Host-wide adaptive capacity broker for governed test leaves.

One Unix connection owns one permit.  The broker authenticates the physical
peer with SO_PEERCRED and matches the requested run to a daemon-registered
run identity.  A dropped connection therefore releases capacity without a
cleanup RPC or lease timeout.
"""

from __future__ import annotations

import json
import math
import os
import secrets
import socket
import stat
import struct
import threading
import time
from collections import defaultdict, deque
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2.daemon.db import Database

CAPACITY_PROTOCOL_SCHEMA = 1
CAP_MIN = 1
CAP_MAX = 65535
SAMPLE_INTERVAL_SECONDS = 15.0
MIN_EPOCH_SECONDS = 600.0
PRESSURE_PERCENT = 98.0
RECOVERY_PERCENT = 95.0
UNDERUSED_PERCENT = 90.0
_UCRED = struct.Struct("iII")


def _now_iso() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _percentile95(values: list[float]) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * 0.95) - 1)]


class ProcMetrics:
    """Content-free CPU and memory percentages from procfs."""

    def __init__(self, proc_root: Path = Path("/proc")):
        self._root = proc_root
        self._previous_cpu = self._cpu_totals()

    def _cpu_totals(self) -> tuple[int, int] | None:
        try:
            first = (self._root / "stat").read_text().splitlines()[0].split()
            if not first or first[0] != "cpu" or len(first) < 5:
                return None
            values = [int(value) for value in first[1:]]
        except (OSError, ValueError, IndexError):
            return None
        total = sum(values)
        idle = values[3] + (values[4] if len(values) > 4 else 0)
        return total, idle

    def _memory_percent(self) -> float | None:
        try:
            values = {}
            for line in (self._root / "meminfo").read_text().splitlines():
                key, separator, value = line.partition(":")
                if separator and key in ("MemTotal", "MemAvailable"):
                    values[key] = int(value.strip().split()[0])
            total, available = values["MemTotal"], values["MemAvailable"]
            if total <= 0 or not 0 <= available <= total:
                return None
            return 100.0 * (total - available) / total
        except (OSError, ValueError, KeyError, IndexError):
            return None

    def sample(self) -> tuple[float | None, float | None]:
        current = self._cpu_totals()
        cpu = None
        if current is not None and self._previous_cpu is not None:
            total_delta = current[0] - self._previous_cpu[0]
            idle_delta = current[1] - self._previous_cpu[1]
            if total_delta > 0 and 0 <= idle_delta <= total_delta:
                cpu = 100.0 * (total_delta - idle_delta) / total_delta
        self._previous_cpu = current
        return cpu, self._memory_percent()


@dataclass
class _Pending:
    connection: socket.socket
    run_id: str
    leaf_id: str
    granted: threading.Event = field(default_factory=threading.Event)
    permit_id: str | None = None
    learned_capacity: int | None = None
    effective_capacity: int | None = None
    waited: bool = False
    error: str | None = None


class CapacityBroker:
    """Fair host-wide permits plus conservative run-end learning."""

    def __init__(
        self,
        db: Database,
        socket_path: Path,
        *,
        logical_cpus: int | None = None,
        clock=time.monotonic,
        metrics: ProcMetrics | None = None,
        sample_interval: float = SAMPLE_INTERVAL_SECONDS,
        min_epoch_seconds: float = MIN_EPOCH_SECONDS,
    ):
        self._db = db
        self.socket_path = socket_path
        self._clock = clock
        self._metrics = metrics or ProcMetrics()
        self._sample_interval = sample_interval
        self._min_epoch_seconds = min_epoch_seconds
        self._condition = threading.Condition()
        self._stop = threading.Event()
        self._listener: socket.socket | None = None
        self._accept_thread: threading.Thread | None = None
        self._sample_thread: threading.Thread | None = None
        self._threads: set[threading.Thread] = set()
        self._registered_runs: dict[str, int] = {}
        self._queues: dict[str, deque[_Pending]] = defaultdict(deque)
        self._round_robin: deque[str] = deque()
        self._active: dict[str, _Pending] = {}
        self._paused = False
        self._pressure_streak = 0
        self._recovery_streak = 0
        self._pending_decrease = False
        self._epoch_started: float | None = None
        self._samples: list[tuple[float | None, float | None, bool]] = []
        self._run_started: dict[str, float] = {}
        self._run_durations: list[float] = []

        initial = max(1, 2 * (logical_cpus or os.cpu_count() or 1))
        rows = self._db.query(
            "SELECT learned_capacity, cap FROM test_capacity_state WHERE singleton=1")
        if not rows:
            with self._db.transaction() as conn:
                conn.execute(
                    "INSERT INTO test_capacity_state(singleton, learned_capacity, cap,"
                    " updated_at) VALUES(1,?,?,?)",
                    (initial, None, _now_iso()),
                )
            self._learned = initial
            self._cap = None
        else:
            self._learned = int(rows[0]["learned_capacity"])
            self._cap = int(rows[0]["cap"]) if rows[0]["cap"] is not None else None

    # -- lifecycle -------------------------------------------------------

    def start(self) -> None:
        self.socket_path.parent.mkdir(parents=True, exist_ok=True)
        try:
            details = self.socket_path.lstat()
        except FileNotFoundError:
            pass
        else:
            if not stat.S_ISSOCK(details.st_mode):
                raise RuntimeError(
                    f"capacity socket path is occupied by a non-socket: {self.socket_path}")
            self.socket_path.unlink()
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(self.socket_path))
        os.chmod(self.socket_path, 0o666)
        listener.listen(256)
        listener.settimeout(0.1)
        self._listener = listener
        self._accept_thread = threading.Thread(
            target=self._accept_loop, name="test-capacity-accept", daemon=True)
        self._sample_thread = threading.Thread(
            target=self._sample_loop, name="test-capacity-sampler", daemon=True)
        self._accept_thread.start()
        self._sample_thread.start()

    def shutdown(self) -> None:
        self._stop.set()
        if self._listener is not None:
            self._listener.close()
        with self._condition:
            for pending in self._all_pending_locked():
                pending.error = "broker shutting down"
                pending.granted.set()
            for lease in self._active.values():
                try:
                    lease.connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
            self._condition.notify_all()
        if self._accept_thread is not None:
            self._accept_thread.join(timeout=2)
        if self._sample_thread is not None:
            self._sample_thread.join(timeout=2)
        for thread in list(self._threads):
            thread.join(timeout=2)
        try:
            if stat.S_ISSOCK(self.socket_path.lstat().st_mode):
                self.socket_path.unlink()
        except FileNotFoundError:
            pass

    # -- run identities -------------------------------------------------

    def register_run(self, run_id: str, uid: int) -> None:
        with self._condition:
            existing = self._registered_runs.get(run_id)
            if existing is not None and existing != uid:
                raise RuntimeError(f"run {run_id} is already registered to another uid")
            self._registered_runs[run_id] = uid

    def unregister_run(self, run_id: str) -> None:
        with self._condition:
            self._registered_runs.pop(run_id, None)
            queue = self._queues.pop(run_id, deque())
            self._round_robin = deque(item for item in self._round_robin
                                      if item != run_id)
            for pending in queue:
                pending.error = "run is no longer active"
                pending.granted.set()
            for permit_id, lease in list(self._active.items()):
                if lease.run_id != run_id:
                    continue
                self._active.pop(permit_id, None)
                try:
                    lease.connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
            self._finish_run_locked(run_id)
            self._grant_ready_locked()
            self._maybe_finish_epoch_locked()

    # -- public administration -----------------------------------------

    def snapshot(self) -> dict:
        with self._condition:
            return self._snapshot_locked()

    def set_cap(self, cap: int | None, actor: str) -> dict:
        if cap is not None and (
                not isinstance(cap, int) or isinstance(cap, bool)
                or not CAP_MIN <= cap <= CAP_MAX):
            raise ValueError(f"cap must be null or an integer in {CAP_MIN}..{CAP_MAX}")
        with self._condition:
            previous_effective = self._effective_locked()
            self._cap = cap
            new_effective = self._effective_locked()
            self._persist_adjustment_locked(
                actor=actor,
                reason="administrator_cap_changed",
                previous=previous_effective,
                new=new_effective,
                cpu=None,
                memory=None,
                saturation=None,
                duration=None,
            )
            self._grant_ready_locked()
            return self._snapshot_locked()

    # -- deterministic learning surface --------------------------------

    def record_sample(self, cpu_percent: float | None,
                      memory_percent: float | None) -> None:
        """Record one 15-second-equivalent host sample.

        This is public to the daemon sampler and deliberately deterministic for
        tests; invalid or missing measurements remain neutral evidence.
        """
        cpu = self._bounded_percent(cpu_percent)
        memory = self._bounded_percent(memory_percent)
        with self._condition:
            if self._epoch_started is None:
                return
            saturated = self._waiting_locked() > 0 or (
                bool(self._active) and len(self._active) >= self._effective_locked())
            self._samples.append((cpu, memory, saturated))
            pressure = (cpu is not None and cpu >= PRESSURE_PERCENT) or (
                memory is not None and memory >= PRESSURE_PERCENT)
            if pressure:
                self._pressure_streak += 1
            else:
                self._pressure_streak = 0
            if self._pressure_streak >= 4:
                self._paused = True
                self._pending_decrease = True
            if self._paused and cpu is not None and memory is not None \
                    and cpu < RECOVERY_PERCENT and memory < RECOVERY_PERCENT:
                self._recovery_streak += 1
            elif self._paused:
                self._recovery_streak = 0
            if self._paused and self._recovery_streak >= 2:
                self._paused = False
                self._pressure_streak = 0
                self._recovery_streak = 0
                self._grant_ready_locked()

    @staticmethod
    def _bounded_percent(value: float | None) -> float | None:
        if isinstance(value, bool) or not isinstance(value, int | float):
            return None
        value = float(value)
        return value if 0.0 <= value <= 100.0 else None

    # -- socket protocol -------------------------------------------------

    def _accept_loop(self) -> None:
        assert self._listener is not None
        while not self._stop.is_set():
            try:
                connection, _ = self._listener.accept()
            except TimeoutError:
                continue
            except OSError:
                break
            thread = threading.Thread(
                target=self._serve_connection, args=(connection,), daemon=True)
            self._threads.add(thread)
            thread.start()
            self._threads = {item for item in self._threads if item.is_alive()}

    def _serve_connection(self, connection: socket.socket) -> None:
        pending: _Pending | None = None
        try:
            _pid, uid, _gid = _UCRED.unpack(connection.getsockopt(
                socket.SOL_SOCKET, socket.SO_PEERCRED, _UCRED.size))
            connection.settimeout(5.0)
            payload = self._read_line(connection)
            if set(payload) != {"schema", "action", "run_id", "leaf_id"} \
                    or payload.get("schema") != CAPACITY_PROTOCOL_SCHEMA \
                    or payload.get("action") != "acquire" \
                    or not isinstance(payload.get("run_id"), str) \
                    or not isinstance(payload.get("leaf_id"), str) \
                    or not payload["run_id"] or len(payload["run_id"]) > 128 \
                    or not payload["leaf_id"] or len(payload["leaf_id"]) > 512:
                self._send_denied(connection, "invalid acquire request")
                return
            pending = _Pending(connection, payload["run_id"], payload["leaf_id"])
            with self._condition:
                if self._registered_runs.get(pending.run_id) != uid:
                    self._send_denied(connection, "run identity is unavailable")
                    return
                self._enqueue_locked(pending)
                self._grant_ready_locked()
                if pending.permit_id is None:
                    pending.waited = True
            connection.settimeout(0.1)
            while not pending.granted.wait(0.1):
                if self._stop.is_set() or self._peer_disconnected(connection):
                    self._cancel_pending(pending)
                    return
            if pending.error is not None:
                self._send_denied(connection, pending.error)
                return
            assert pending.permit_id is not None
            connection.sendall((json.dumps({
                "schema": CAPACITY_PROTOCOL_SCHEMA,
                "status": "granted",
                "permit_id": pending.permit_id,
                "learned_capacity": pending.learned_capacity,
                "effective_capacity": pending.effective_capacity,
                "waited": pending.waited,
            }, separators=(",", ":")) + "\n").encode())
            while not self._stop.is_set():
                try:
                    chunk = connection.recv(4096)
                except TimeoutError:
                    continue
                if not chunk:
                    break
                # An explicit release message is advisory; closing the
                # connection remains the authoritative release.
                if b'"action":"release"' in chunk.replace(b" ", b""):
                    break
        except (OSError, ValueError, json.JSONDecodeError, UnicodeDecodeError):
            pass
        finally:
            if pending is not None:
                if pending.permit_id is None:
                    self._cancel_pending(pending)
                else:
                    self._release(pending.permit_id)
            connection.close()

    @staticmethod
    def _read_line(connection: socket.socket) -> dict:
        data = bytearray()
        while not data.endswith(b"\n"):
            chunk = connection.recv(1024)
            if not chunk:
                raise ValueError("connection closed before request")
            data.extend(chunk)
            if len(data) > 4096:
                raise ValueError("request too large")
        value = json.loads(data)
        if not isinstance(value, dict):
            raise ValueError("request must be an object")
        return value

    @staticmethod
    def _send_denied(connection: socket.socket, error: str) -> None:
        try:
            connection.sendall((json.dumps({
                "schema": CAPACITY_PROTOCOL_SCHEMA,
                "status": "denied",
                "error": error,
            }, separators=(",", ":")) + "\n").encode())
        except OSError:
            pass

    @staticmethod
    def _peer_disconnected(connection: socket.socket) -> bool:
        try:
            return connection.recv(1, socket.MSG_PEEK) == b""
        except (TimeoutError, BlockingIOError):
            return False
        except OSError:
            return True

    # -- queue and epoch mechanics -------------------------------------

    def _enqueue_locked(self, pending: _Pending) -> None:
        queue = self._queues[pending.run_id]
        if not queue:
            self._round_robin.append(pending.run_id)
        queue.append(pending)
        now = self._clock()
        if self._epoch_started is None:
            self._epoch_started = now
            self._samples = []
            self._run_durations = []
            self._pending_decrease = False
        self._run_started.setdefault(pending.run_id, now)

    def _grant_ready_locked(self) -> None:
        while not self._paused and len(self._active) < self._effective_locked() \
                and self._round_robin:
            run_id = self._round_robin.popleft()
            queue = self._queues.get(run_id)
            if not queue:
                continue
            pending = queue.popleft()
            if queue:
                self._round_robin.append(run_id)
            else:
                self._queues.pop(run_id, None)
            if self._registered_runs.get(run_id) is None:
                pending.error = "run is no longer active"
                pending.granted.set()
                continue
            permit_id = "c" + secrets.token_hex(8)
            pending.permit_id = permit_id
            pending.learned_capacity = self._learned
            pending.effective_capacity = self._effective_locked()
            self._active[permit_id] = pending
            pending.granted.set()

    def _cancel_pending(self, pending: _Pending) -> None:
        with self._condition:
            queue = self._queues.get(pending.run_id)
            if queue and pending in queue:
                queue.remove(pending)
                if not queue:
                    self._queues.pop(pending.run_id, None)
                    self._round_robin = deque(
                        run for run in self._round_robin if run != pending.run_id)
            self._finish_run_locked(pending.run_id)
            self._maybe_finish_epoch_locked()

    def _release(self, permit_id: str) -> None:
        with self._condition:
            lease = self._active.pop(permit_id, None)
            if lease is None:
                return
            self._finish_run_locked(lease.run_id)
            self._grant_ready_locked()
            self._maybe_finish_epoch_locked()

    def _finish_run_locked(self, run_id: str) -> None:
        if run_id not in self._run_started:
            return
        if self._queues.get(run_id) or any(
                lease.run_id == run_id for lease in self._active.values()):
            return
        self._run_durations.append(self._clock() - self._run_started.pop(run_id))

    def _maybe_finish_epoch_locked(self) -> None:
        if self._epoch_started is None or self._active or self._waiting_locked():
            return
        finished = self._clock()
        epoch_duration = finished - self._epoch_started
        longest_run = max(self._run_durations, default=0.0)
        cpu_values = [cpu for cpu, _memory, _saturated in self._samples
                      if cpu is not None]
        memory_values = [memory for _cpu, memory, _saturated in self._samples
                         if memory is not None]
        complete_metrics = bool(self._samples) and len(cpu_values) == len(self._samples) \
            and len(memory_values) == len(self._samples)
        cpu = _percentile95(cpu_values) if complete_metrics else None
        memory = _percentile95(memory_values) if complete_metrics else None
        saturation = (sum(1 for _cpu, _memory, value in self._samples if value)
                      / len(self._samples)) if self._samples else None

        previous = self._learned
        new = previous
        reason = None
        if longest_run >= self._min_epoch_seconds and complete_metrics:
            if self._pending_decrease and previous > 1:
                new = max(1, min(previous - 1, math.floor(previous * 0.75)))
                reason = "sustained_pressure"
            elif self._cap is None or self._cap > previous:
                if saturation is not None and saturation >= 0.5 \
                        and cpu is not None and cpu < UNDERUSED_PERCENT \
                        and memory is not None and memory < UNDERUSED_PERCENT:
                    new = min(CAP_MAX, max(previous + 1,
                                          math.ceil(previous * 1.25)))
                    reason = "underused_saturated_epoch"
        if reason is not None and new != previous:
            self._learned = new
            self._persist_adjustment_locked(
                actor="system:auto", reason=reason, previous=previous, new=new,
                cpu=cpu, memory=memory, saturation=saturation,
                duration=epoch_duration)

        self._epoch_started = None
        self._samples = []
        self._run_started = {}
        self._run_durations = []
        self._pending_decrease = False
        self._pressure_streak = 0
        self._recovery_streak = 0
        self._paused = False

    def _all_pending_locked(self) -> list[_Pending]:
        return [pending for queue in self._queues.values() for pending in queue]

    def _waiting_locked(self) -> int:
        return sum(len(queue) for queue in self._queues.values())

    def _effective_locked(self) -> int:
        return min(self._learned, self._cap) if self._cap is not None else self._learned

    def _snapshot_locked(self) -> dict:
        rows = self._db.query(
            "SELECT * FROM test_capacity_events ORDER BY event_id DESC LIMIT 1")
        return {
            "learned_capacity": self._learned,
            "effective_capacity": self._effective_locked(),
            "cap": self._cap,
            "active": len(self._active),
            "waiting": self._waiting_locked(),
            "paused": self._paused,
            "last_adjustment": dict(rows[0]) if rows else None,
        }

    def _persist_adjustment_locked(
        self, *, actor: str, reason: str, previous: int, new: int,
        cpu: float | None, memory: float | None,
        saturation: float | None, duration: float | None,
    ) -> None:
        now = _now_iso()
        with self._db.transaction() as conn:
            conn.execute(
                "UPDATE test_capacity_state SET learned_capacity=?, cap=?, updated_at=?"
                " WHERE singleton=1",
                (self._learned, self._cap, now),
            )
            conn.execute(
                "INSERT INTO test_capacity_events(at,actor,reason,previous_capacity,"
                " new_capacity,cap,p95_cpu_percent,p95_memory_percent,"
                " saturation_fraction,epoch_seconds) VALUES(?,?,?,?,?,?,?,?,?,?)",
                (now, actor, reason, previous, new, self._cap, cpu, memory,
                 saturation, duration),
            )

    def _sample_loop(self) -> None:
        while not self._stop.wait(self._sample_interval):
            cpu, memory = self._metrics.sample()
            self.record_sample(cpu, memory)
