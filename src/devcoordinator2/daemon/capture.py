"""Byte-complete output capture for the small systemd executor wrapper.

Governed leaf streams are owned by the Rust log store. This drainer captures
only the wrapper's own stdout/stderr and never silently discards evidence.
Storage write failures remain observable because retained bytes stop matching
observed bytes and the lifecycle cannot claim complete output.
"""

from __future__ import annotations

import os
import threading
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

_CHUNK = 65536


@dataclass
class StreamCounts:
    observed: int
    retained: int
    error_code: str | None


class Drainer:
    def __init__(self, pipe: BinaryIO, log: BinaryIO,
                 on_storage_error: Callable[[], None] | None = None):
        self._pipe = pipe
        self._log = log
        self._on_storage_error = on_storage_error
        self._observed = 0
        self._retained = 0
        self._error_code: str | None = None
        self._lock = threading.Lock()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def start(self) -> None:
        self._thread.start()

    def join(self, timeout: float | None = None) -> None:
        self._thread.join(timeout)

    @property
    def counts(self) -> StreamCounts:
        with self._lock:
            return StreamCounts(
                observed=self._observed,
                retained=self._retained,
                error_code=self._error_code,
            )

    def _record_storage_error(self) -> None:
        with self._lock:
            if self._error_code is not None:
                return
            self._error_code = "log_storage"
        if self._on_storage_error is not None:
            try:
                self._on_storage_error()
            except Exception:
                pass

    def _run(self) -> None:
        try:
            with self._log as log:
                # read1 returns as soon as any bytes are available (read
                # would block until a full chunk), keeping on-demand tails
                # fresh while a test is still running.
                read = getattr(self._pipe, "read1", self._pipe.read)
                while True:
                    chunk = read(_CHUNK)
                    if not chunk:
                        break
                    with self._lock:
                        self._observed += len(chunk)
                        written = log.write(chunk)
                        if written != len(chunk):
                            raise OSError("short governed-test log write")
                        log.flush()
                        self._retained += written
                os.fsync(log.fileno())
        except (OSError, ValueError):
            self._record_storage_error()
        finally:
            try:
                self._pipe.close()
            except OSError:
                pass


def tail_file(log_path: Path, tail_bytes: int) -> tuple[bytes, bool]:
    """Return (last tail_bytes of the file, whether earlier bytes exist)."""
    try:
        size = log_path.stat().st_size
        with open(log_path, "rb") as fh:
            if size > tail_bytes:
                fh.seek(size - tail_bytes)
                return fh.read(tail_bytes), True
            return fh.read(), False
    except OSError:
        return b"", False
