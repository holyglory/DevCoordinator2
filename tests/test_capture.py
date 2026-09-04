import os
import subprocess
from pathlib import Path

from devcoordinator2.daemon.capture import Drainer, tail_file


def private_log(path: Path):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    return os.fdopen(fd, "wb", buffering=0)


def test_drainer_retains_every_byte_and_uses_private_mode(tmp_path: Path):
    log = tmp_path / "stdout.log"
    read_fd, write_fd = os.pipe()
    drainer = Drainer(os.fdopen(read_fd, "rb"), private_log(log))
    drainer.start()
    payload = b"x" * 5000
    with os.fdopen(write_fd, "wb") as w:
        w.write(payload)
    drainer.join(5)
    counts = drainer.counts
    assert counts.observed == 5000
    assert counts.retained == 5000
    assert counts.error_code is None
    assert log.read_bytes() == payload
    assert log.stat().st_mode & 0o777 == 0o600


def test_noisy_child_never_blocks(tmp_path: Path):
    """A child writing far more than the pipe buffer must run to completion."""
    log = tmp_path / "stdout.log"
    child = subprocess.Popen(
        ["dd", "if=/dev/zero", "bs=64k", "count=512", "status=none"],
        stdout=subprocess.PIPE,
    )
    drainer = Drainer(child.stdout, private_log(log))
    drainer.start()
    assert child.wait(timeout=30) == 0
    drainer.join(10)
    counts = drainer.counts
    assert counts.observed == 64 * 1024 * 512
    assert counts.retained == counts.observed
    assert log.stat().st_size == counts.observed


def test_live_output_visible_before_child_exits(tmp_path: Path):
    """Small early writes must reach the log promptly (read1 + flush), not
    sit in a 64 KiB buffered read while the child is still running."""
    import sys
    import time

    log = tmp_path / "stdout.log"
    child = subprocess.Popen(
        [sys.executable, "-c",
         "import sys,time; sys.stdout.write('early-line\\n');"
         " sys.stdout.flush(); time.sleep(30)"],
        stdout=subprocess.PIPE,
    )
    drainer = Drainer(child.stdout, private_log(log))
    drainer.start()
    try:
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if log.exists() and b"early-line" in log.read_bytes():
                break
            time.sleep(0.05)
        assert b"early-line" in log.read_bytes()
    finally:
        child.kill()
        child.wait(10)
        drainer.join(10)


def test_storage_error_is_typed_and_invokes_termination_callback():
    class BrokenLog:
        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def write(self, _payload):
            raise OSError("disk unavailable")

    read_fd, write_fd = os.pipe()
    failures = []
    drainer = Drainer(
        os.fdopen(read_fd, "rb"), BrokenLog(),
        on_storage_error=lambda: failures.append("stop"),
    )
    drainer.start()
    with os.fdopen(write_fd, "wb") as writer:
        writer.write(b"not-retained")
    drainer.join(5)
    assert drainer.counts.error_code == "log_storage"
    assert drainer.counts.observed == len(b"not-retained")
    assert drainer.counts.retained == 0
    assert failures == ["stop"]


def test_tail_file(tmp_path: Path):
    log = tmp_path / "log"
    log.write_bytes(b"0123456789")
    tail, truncated_before = tail_file(log, 4)
    assert tail == b"6789"
    assert truncated_before is True
    tail, truncated_before = tail_file(log, 100)
    assert tail == b"0123456789"
    assert truncated_before is False
    tail, truncated_before = tail_file(tmp_path / "missing", 4)
    assert tail == b""
