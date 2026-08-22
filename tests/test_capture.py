import os
import subprocess
from pathlib import Path

from devcoordinator2.daemon.capture import Drainer, tail_file


def test_drainer_caps_but_keeps_counting(tmp_path: Path):
    log = tmp_path / "stdout.log"
    read_fd, write_fd = os.pipe()
    drainer = Drainer(os.fdopen(read_fd, "rb"), log, cap=1000)
    drainer.start()
    payload = b"x" * 5000
    with os.fdopen(write_fd, "wb") as w:
        w.write(payload)
    drainer.join(5)
    counts = drainer.counts
    assert counts.observed == 5000
    assert counts.retained == 1000
    assert log.stat().st_size == 1000


def test_noisy_child_never_blocks(tmp_path: Path):
    """A child writing far more than the pipe buffer must run to completion."""
    log = tmp_path / "stdout.log"
    child = subprocess.Popen(
        ["dd", "if=/dev/zero", "bs=64k", "count=512", "status=none"],
        stdout=subprocess.PIPE,
    )
    drainer = Drainer(child.stdout, log, cap=4096)
    drainer.start()
    assert child.wait(timeout=30) == 0
    drainer.join(10)
    counts = drainer.counts
    assert counts.observed == 64 * 1024 * 512
    assert counts.retained == 4096


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
    drainer = Drainer(child.stdout, log)
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
