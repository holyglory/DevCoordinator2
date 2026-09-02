"""Byte-complete output capture for the small systemd executor wrapper.

Governed leaf streams are owned by the Rust log store. This drainer captures
only the wrapper's own stdout/stderr and never silently discards evidence.
Storage write failures remain observable because retained bytes stop matching
observed bytes and the lifecycle cannot claim complete output.
"""

from __future__ import annotations

import os
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

_CHUNK = 65536


@dataclass
class StreamCounts:
    observed: int
    retained: int


class Drainer:
    def __init__(self, pipe: BinaryIO, log_path: Path,
                 owner: tuple[int, int] | None = None):
        self._pipe = pipe
        self._log_path = log_path
        self._owner = owner
        self._observed = 0
        self._retained = 0
        self._lock = threading.Lock()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def start(self) -> None:
        self._thread.start()

    def join(self, timeout: float | None = None) -> None:
        self._thread.join(timeout)

    @property
    def counts(self) -> StreamCounts:
        with self._lock:
            return StreamCounts(observed=self._observed, retained=self._retained)

    def _run(self) -> None:
        try:
            with open(self._log_path, "wb") as log:
                try:
                    os.fchmod(log.fileno(), 0o600)
                    if self._owner is not None:
                        os.fchown(log.fileno(), self._owner[0], self._owner[1])
                except OSError:
                    pass
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
                        log.write(chunk)
                        log.flush()
                        self._retained += len(chunk)
        except (OSError, ValueError):
            pass  # pipe closed underneath us; counts remain truthful
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
