"""Atomic test admission, upgrade drain leases, and activity receipts."""

from __future__ import annotations

import contextlib
import ctypes
import fcntl
import os
import secrets
import select
import threading
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2.check_evidence import read_json_bounded, write_json_atomic

LOCK_FILE = "test-admission.lock"
DRAIN_FILE = "test-drain.json"
ACTIVITY_FILE = "test-activity.json"
_INOTIFY_EVENTS = 0x00000008 | 0x00000080 | 0x00000100 | 0x00000200


class AdmissionError(Exception):
    pass


class TestsDraining(AdmissionError):
    pass


def _iso_now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _process_start(pid: int) -> str | None:
    try:
        text = Path(f"/proc/{pid}/stat").read_text()
        tail = text[text.rfind(")") + 2:].split()
        return tail[19]
    except (OSError, IndexError, ValueError):
        return None


def _lease_is_live(document: dict | None) -> bool:
    if not isinstance(document, dict) or document.get("schema") != 1:
        return False
    pid = document.get("pid")
    start = document.get("process_start")
    return isinstance(pid, int) and isinstance(start, str) \
        and _process_start(pid) == start


class DirectoryEvents:
    """Blocking Linux directory-change events; callers subscribe before reading."""

    def __init__(self, directory: Path):
        libc = ctypes.CDLL(None, use_errno=True)
        self._close = libc.close
        fd = libc.inotify_init1(os.O_CLOEXEC)
        if fd < 0:
            errno = ctypes.get_errno()
            raise OSError(errno, os.strerror(errno))
        encoded = os.fsencode(directory)
        if libc.inotify_add_watch(fd, encoded, _INOTIFY_EVENTS) < 0:
            errno = ctypes.get_errno()
            self._close(fd)
            raise OSError(errno, os.strerror(errno))
        self.fd = fd

    def wait(self, watchdog_seconds: float | None = None) -> None:
        if watchdog_seconds is not None:
            watcher = select.poll()
            watcher.register(self.fd, select.POLLIN | select.POLLERR)
            if not watcher.poll(max(0, int(watchdog_seconds * 1000))):
                raise TimeoutError("directory event watchdog expired")
        os.read(self.fd, 65536)

    def close(self) -> None:
        if self.fd >= 0:
            self._close(self.fd)
            self.fd = -1

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.close()


class TestAdmission:
    def __init__(self, runtime_dir: Path):
        self.runtime_dir = runtime_dir
        runtime_dir.mkdir(parents=True, exist_ok=True)
        self.lock_path = runtime_dir / LOCK_FILE
        self.drain_path = runtime_dir / DRAIN_FILE
        self.activity_path = runtime_dir / ACTIVITY_FILE
        self._lock_fd = os.open(
            self.lock_path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o666)
        os.fchmod(self._lock_fd, 0o666)
        self._thread_lock = threading.RLock()
        self._depth = 0
        self._active: dict[str, str] = {}
        self._generation = 0

    @contextlib.contextmanager
    def _critical(self):
        with self._thread_lock:
            if self._depth == 0:
                fcntl.flock(self._lock_fd, fcntl.LOCK_EX)
            self._depth += 1
            try:
                yield
            finally:
                self._depth -= 1
                if self._depth == 0:
                    fcntl.flock(self._lock_fd, fcntl.LOCK_UN)

    def _live_drain(self) -> dict | None:
        document = read_json_bounded(self.drain_path)
        if document is None:
            return None
        if _lease_is_live(document):
            return document
        self.drain_path.unlink(missing_ok=True)
        return None

    @contextlib.contextmanager
    def start_guard(self):
        with self._critical():
            drain = self._live_drain()
            if drain is not None:
                raise TestsDraining(
                    str(drain.get("reason") or "coordinator upgrade in progress"))
            yield self

    def started(self, run_id: str, unit: str) -> None:
        with self._critical():
            self._active[run_id] = unit
            try:
                self._write_activity()
            except BaseException:
                self._active.pop(run_id, None)
                raise

    def finished(self, run_id: str) -> None:
        with self._critical():
            if self._active.pop(run_id, None) is not None:
                self._write_activity()

    def reset(self) -> None:
        with self._critical():
            self._active.clear()
            self._write_activity()

    def _write_activity(self) -> None:
        self._generation += 1
        write_json_atomic(self.activity_path, {
            "schema": 1,
            "generation": self._generation,
            "active": [{"run_id": run_id, "unit": self._active[run_id]}
                       for run_id in sorted(self._active)],
        }, mode=0o644)


@dataclass(frozen=True)
class DrainLease:
    runtime_dir: Path
    nonce: str


def begin_drain(runtime_dir: Path, reason: str) -> DrainLease:
    runtime_dir.mkdir(parents=True, exist_ok=True)
    lock_path = runtime_dir / LOCK_FILE
    lock_fd = os.open(lock_path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o666)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        path = runtime_dir / DRAIN_FILE
        current = read_json_bounded(path)
        if _lease_is_live(current):
            raise AdmissionError("another live test drain already exists")
        path.unlink(missing_ok=True)
        nonce = secrets.token_hex(16)
        start = _process_start(os.getpid())
        if start is None:
            raise AdmissionError("cannot identify installer process for drain lease")
        write_json_atomic(path, {
            "schema": 1,
            "pid": os.getpid(),
            "process_start": start,
            "nonce": nonce,
            "reason": reason,
            "created_at": _iso_now(),
        }, mode=0o600)
        return DrainLease(runtime_dir=runtime_dir, nonce=nonce)
    finally:
        fcntl.flock(lock_fd, fcntl.LOCK_UN)
        os.close(lock_fd)


def end_drain(lease: DrainLease) -> None:
    lock_path = lease.runtime_dir / LOCK_FILE
    lock_fd = os.open(lock_path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o666)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        path = lease.runtime_dir / DRAIN_FILE
        current = read_json_bounded(path)
        if isinstance(current, dict) and current.get("nonce") == lease.nonce:
            path.unlink(missing_ok=True)
    finally:
        fcntl.flock(lock_fd, fcntl.LOCK_UN)
        os.close(lock_fd)


def read_activity(runtime_dir: Path) -> dict | None:
    document = read_json_bounded(runtime_dir / ACTIVITY_FILE)
    if document is None or document.get("schema") != 1 \
            or not isinstance(document.get("active"), list):
        return None
    return document


def wait_for_zero_activity(runtime_dir: Path) -> None:
    """Wait for the exact atomic activity receipt; no polling or time success."""
    while True:
        with DirectoryEvents(runtime_dir) as events:
            activity = read_activity(runtime_dir)
            if activity is None:
                raise AdmissionError("test activity receipt is unavailable")
            if not activity["active"]:
                return
            events.wait()
