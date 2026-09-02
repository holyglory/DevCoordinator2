"""Symlink-safe file operations for a root daemon inside caller-writable trees.

Every descent uses dirfd-relative opens with O_NOFOLLOW so a caller-planted
symlink can never redirect creation, chown, or deletion outside the
repository's .devcoordinator tree.
"""

from __future__ import annotations

import os
import re
import stat
import threading
import time
from typing import BinaryIO
from pathlib import Path


class SecureFsError(Exception):
    pass


_TEST_HISTORY_MAX_BYTES = 2 * 1024 * 1024
_TEST_EVIDENCE_MAX_BYTES = 2 * 1024 * 1024
_RUN_ID_RE = re.compile(r"t[0-9]{8}T[0-9]{6}Z-[0-9a-f]{6}$")


def _open_dir(dir_fd: int | None, name: str | Path) -> int:
    try:
        return os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                       dir_fd=dir_fd)
    except OSError as exc:
        raise SecureFsError(f"cannot open directory {name!r}: {exc}") from exc


def _rmtree_at(dir_fd: int, name: str) -> None:
    """Delete dir_fd/name (file or directory) without following symlinks."""
    try:
        fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                     dir_fd=dir_fd)
    except OSError:
        # Not a directory (regular file, symlink, fifo, …): unlink in place.
        try:
            os.unlink(name, dir_fd=dir_fd)
        except FileNotFoundError:
            pass
        except OSError as exc:
            raise SecureFsError(f"cannot unlink {name!r}: {exc}") from exc
        return
    try:
        for entry in os.listdir(fd):
            _rmtree_at(fd, entry)
    finally:
        os.close(fd)
    try:
        os.rmdir(name, dir_fd=dir_fd)
    except FileNotFoundError:
        pass
    except OSError as exc:
        raise SecureFsError(f"cannot rmdir {name!r}: {exc}") from exc


def remove_test_dir(worktree_root: Path) -> None:
    """Delete exactly <worktree>/.devcoordinator/test/current, symlink-safely."""
    root_fd = _open_dir(None, worktree_root)
    try:
        try:
            devco_fd = _open_dir(root_fd, ".devcoordinator")
        except SecureFsError:
            return  # nothing to delete
        try:
            try:
                test_fd = _open_dir(devco_fd, "test")
            except SecureFsError:
                return
            try:
                _rmtree_at(test_fd, "current")
            finally:
                os.close(test_fd)
        finally:
            os.close(devco_fd)
    finally:
        os.close(root_fd)


def create_test_dir(worktree_root: Path, uid: int, gid: int) -> Path:
    """Create <worktree>/.devcoordinator/test/current/{artifacts,scratch},
    owned by the caller, without following symlinks anywhere."""
    current = worktree_root / ".devcoordinator" / "test" / "current"
    fd = _open_dir(None, worktree_root)
    try:
        for name in (".devcoordinator", "test"):
            try:
                os.mkdir(name, mode=0o755, dir_fd=fd)
            except FileExistsError:
                pass
            os.chown(name, uid, gid, dir_fd=fd, follow_symlinks=False)
            child = _open_dir(fd, name)
            os.close(fd)
            fd = child
        try:
            os.mkdir("current", mode=0o755, dir_fd=fd)
        except FileExistsError as exc:
            raise SecureFsError("prior test directory still present") from exc
        os.chown("current", uid, gid, dir_fd=fd, follow_symlinks=False)
        current_fd = _open_dir(fd, "current")
        try:
            for name in ("artifacts", "scratch"):
                os.mkdir(name, mode=0o755, dir_fd=current_fd)
                os.chown(name, uid, gid, dir_fd=current_fd, follow_symlinks=False)
        finally:
            os.close(current_fd)
    except OSError as exc:
        raise SecureFsError(f"cannot create test directory: {exc}") from exc
    finally:
        os.close(fd)
    return current


def create_test_log_run_dir(worktree_root: Path, run_id: str,
                            uid: int, gid: int) -> Path:
    """Create one stable private log run without following repository links."""
    if not _RUN_ID_RE.fullmatch(run_id):
        raise SecureFsError("invalid governed-test run identifier")
    run_path = (worktree_root / ".devcoordinator" / "test" / "logs"
                / "runs" / run_id)
    root_fd = _open_dir(None, worktree_root)
    opened: list[int] = []
    fd = root_fd
    try:
        for name in (".devcoordinator", "test"):
            child = _open_dir(fd, name)
            opened.append(child)
            fd = child
        for name in ("logs", "runs"):
            try:
                os.mkdir(name, mode=0o711, dir_fd=fd)
            except FileExistsError:
                pass
            child = _open_dir(fd, name)
            opened.append(child)
            fd = child
        try:
            os.mkdir(run_id, mode=0o700, dir_fd=fd)
        except FileExistsError as exc:
            raise SecureFsError("governed-test log run already exists") from exc
        os.chown(run_id, uid, gid, dir_fd=fd, follow_symlinks=False)
        run_fd = _open_dir(fd, run_id)
        try:
            os.mkdir("executor", mode=0o700, dir_fd=run_fd)
            os.chown("executor", uid, gid, dir_fd=run_fd, follow_symlinks=False)
            os.fsync(run_fd)
        finally:
            os.close(run_fd)
        os.fsync(fd)
    except OSError as exc:
        raise SecureFsError(f"cannot create governed-test log run: {exc}") from exc
    finally:
        for opened_fd in reversed(opened):
            os.close(opened_fd)
        os.close(root_fd)
    return run_path


def remove_test_log_run_dir(worktree_root: Path, run_id: str) -> None:
    """Remove one exact never-started log run during launch rollback."""
    if not _RUN_ID_RE.fullmatch(run_id):
        raise SecureFsError("invalid governed-test run identifier")
    root_fd = _open_dir(None, worktree_root)
    opened: list[int] = []
    fd = root_fd
    try:
        try:
            for name in (".devcoordinator", "test", "logs", "runs"):
                child = _open_dir(fd, name)
                opened.append(child)
                fd = child
        except SecureFsError:
            return
        _rmtree_at(fd, run_id)
        os.fsync(fd)
    finally:
        for opened_fd in reversed(opened):
            os.close(opened_fd)
        os.close(root_fd)


def create_test_executor_log_files(worktree_root: Path, run_id: str,
                                   uid: int, gid: int) \
        -> tuple[BinaryIO, BinaryIO]:
    """Create and hold the wrapper streams before caller code can run."""
    if not _RUN_ID_RE.fullmatch(run_id):
        raise SecureFsError("invalid governed-test run identifier")
    root_fd = _open_dir(None, worktree_root)
    opened: list[int] = []
    fd = root_fd
    stream_fds: list[int] = []
    created_names: list[str] = []
    stream_parent_ready = False
    try:
        for name in (
                ".devcoordinator", "test", "logs", "runs", run_id, "executor"):
            child = _open_dir(fd, name)
            opened.append(child)
            fd = child
        stream_parent_ready = True
        for name in ("stdout.log", "stderr.log"):
            stream_fd = os.open(
                name,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
                | os.O_CLOEXEC,
                0o600,
                dir_fd=fd,
            )
            os.fchmod(stream_fd, 0o600)
            os.fchown(stream_fd, uid, gid)
            stream_fds.append(stream_fd)
            created_names.append(name)
        os.fsync(fd)
        stdout = os.fdopen(stream_fds.pop(0), "wb", buffering=0)
        stderr = os.fdopen(stream_fds.pop(0), "wb", buffering=0)
        return stdout, stderr
    except OSError as exc:
        for stream_fd in stream_fds:
            try:
                os.close(stream_fd)
            except OSError:
                pass
        if stream_parent_ready:
            for name in created_names:
                try:
                    os.unlink(name, dir_fd=fd)
                except OSError:
                    pass
        raise SecureFsError(
            f"cannot create governed-test executor log: {exc}") from exc
    finally:
        for opened_fd in reversed(opened):
            os.close(opened_fd)
        os.close(root_fd)


def read_test_history(worktree_root: Path) -> bytes | None:
    """Read bounded test/history.json without following repository symlinks."""
    root_fd = _open_dir(None, worktree_root)
    try:
        try:
            devco_fd = _open_dir(root_fd, ".devcoordinator")
        except SecureFsError:
            return None
        try:
            try:
                test_fd = _open_dir(devco_fd, "test")
            except SecureFsError:
                return None
            try:
                try:
                    fd = os.open("history.json", os.O_RDONLY | os.O_NOFOLLOW,
                                 dir_fd=test_fd)
                except FileNotFoundError:
                    return None
                except OSError as exc:
                    raise SecureFsError(f"cannot open test history: {exc}") from exc
                try:
                    details = os.fstat(fd)
                    if not stat.S_ISREG(details.st_mode):
                        raise SecureFsError("test history is not a regular file")
                    if details.st_size > _TEST_HISTORY_MAX_BYTES:
                        raise SecureFsError("test history exceeds the 2 MiB limit")
                    chunks = []
                    remaining = _TEST_HISTORY_MAX_BYTES + 1
                    while remaining:
                        chunk = os.read(fd, min(65536, remaining))
                        if not chunk:
                            break
                        chunks.append(chunk)
                        remaining -= len(chunk)
                    payload = b"".join(chunks)
                    if len(payload) > _TEST_HISTORY_MAX_BYTES:
                        raise SecureFsError("test history exceeds the 2 MiB limit")
                    return payload
                finally:
                    os.close(fd)
            finally:
                os.close(test_fd)
        finally:
            os.close(devco_fd)
    finally:
        os.close(root_fd)


def write_test_history(worktree_root: Path, payload: bytes,
                       owner: tuple[int, int]) -> None:
    """Atomically replace test/history.json through a symlink-safe dirfd."""
    if len(payload) > _TEST_HISTORY_MAX_BYTES:
        raise SecureFsError("test history exceeds the 2 MiB limit")
    root_fd = _open_dir(None, worktree_root)
    try:
        devco_fd = _open_dir(root_fd, ".devcoordinator")
        try:
            test_fd = _open_dir(devco_fd, "test")
            try:
                tmp_name = (f".history-{os.getpid()}-{threading.get_ident()}-"
                            f"{time.time_ns()}")
                fd = os.open(tmp_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL
                             | os.O_NOFOLLOW, 0o644, dir_fd=test_fd)
                try:
                    written = 0
                    while written < len(payload):
                        written += os.write(fd, payload[written:])
                    os.fsync(fd)
                    os.fchmod(fd, 0o644)
                    os.fchown(fd, owner[0], owner[1])
                finally:
                    os.close(fd)
                try:
                    os.replace(tmp_name, "history.json", src_dir_fd=test_fd,
                               dst_dir_fd=test_fd)
                    os.fsync(test_fd)
                except OSError:
                    try:
                        os.unlink(tmp_name, dir_fd=test_fd)
                    except OSError:
                        pass
                    raise
            finally:
                os.close(test_fd)
        finally:
            os.close(devco_fd)
    except OSError as exc:
        raise SecureFsError(f"cannot write test history: {exc}") from exc
    finally:
        os.close(root_fd)


def read_test_evidence(worktree_root: Path) -> bytes | None:
    """Read bounded test/evidence.json without following repository symlinks."""
    root_fd = _open_dir(None, worktree_root)
    try:
        try:
            devco_fd = _open_dir(root_fd, ".devcoordinator")
        except SecureFsError:
            return None
        try:
            try:
                test_fd = _open_dir(devco_fd, "test")
            except SecureFsError:
                return None
            try:
                try:
                    fd = os.open("evidence.json", os.O_RDONLY | os.O_NOFOLLOW,
                                 dir_fd=test_fd)
                except FileNotFoundError:
                    return None
                except OSError as exc:
                    raise SecureFsError(f"cannot open test evidence: {exc}") from exc
                try:
                    details = os.fstat(fd)
                    if not stat.S_ISREG(details.st_mode):
                        raise SecureFsError("test evidence is not a regular file")
                    if details.st_size > _TEST_EVIDENCE_MAX_BYTES:
                        raise SecureFsError("test evidence exceeds the 2 MiB limit")
                    payload = os.read(fd, _TEST_EVIDENCE_MAX_BYTES + 1)
                    if len(payload) > _TEST_EVIDENCE_MAX_BYTES:
                        raise SecureFsError("test evidence exceeds the 2 MiB limit")
                    return payload
                finally:
                    os.close(fd)
            finally:
                os.close(test_fd)
        finally:
            os.close(devco_fd)
    finally:
        os.close(root_fd)


def write_test_evidence(worktree_root: Path, payload: bytes,
                        owner: tuple[int, int]) -> None:
    """Atomically replace bounded test/evidence.json through a safe dirfd."""
    if len(payload) > _TEST_EVIDENCE_MAX_BYTES:
        raise SecureFsError("test evidence exceeds the 2 MiB limit")
    root_fd = _open_dir(None, worktree_root)
    try:
        devco_fd = _open_dir(root_fd, ".devcoordinator")
        try:
            test_fd = _open_dir(devco_fd, "test")
            try:
                tmp_name = (f".evidence-{os.getpid()}-{threading.get_ident()}-"
                            f"{time.time_ns()}")
                fd = os.open(tmp_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL
                             | os.O_NOFOLLOW, 0o600, dir_fd=test_fd)
                try:
                    written = 0
                    while written < len(payload):
                        written += os.write(fd, payload[written:])
                    os.fsync(fd)
                    os.fchmod(fd, 0o600)
                    os.fchown(fd, owner[0], owner[1])
                finally:
                    os.close(fd)
                try:
                    os.replace(tmp_name, "evidence.json", src_dir_fd=test_fd,
                               dst_dir_fd=test_fd)
                    os.fsync(test_fd)
                except OSError:
                    try:
                        os.unlink(tmp_name, dir_fd=test_fd)
                    except OSError:
                        pass
                    raise
            finally:
                os.close(test_fd)
        finally:
            os.close(devco_fd)
    except OSError as exc:
        raise SecureFsError(f"cannot write test evidence: {exc}") from exc
    finally:
        os.close(root_fd)


def tail_test_file(worktree_root: Path, relative: tuple[str, ...],
                   tail_bytes: int) -> tuple[bytes, bool]:
    """Read one caller-owned test file without following any symlink."""
    if not relative or any(not name or "/" in name or name in (".", "..")
                           for name in relative):
        raise SecureFsError("invalid test output path")
    root_fd = _open_dir(None, worktree_root)
    fd = root_fd
    opened = []
    try:
        for name in (".devcoordinator", "test", "current", *relative[:-1]):
            child = _open_dir(fd, name)
            opened.append(child)
            fd = child
        try:
            file_fd = os.open(relative[-1], os.O_RDONLY | os.O_NOFOLLOW, dir_fd=fd)
        except FileNotFoundError:
            return b"", False
        except OSError as exc:
            raise SecureFsError(f"cannot open test output: {exc}") from exc
        try:
            details = os.fstat(file_fd)
            if not stat.S_ISREG(details.st_mode):
                raise SecureFsError("test output is not a regular file")
            truncated = details.st_size > tail_bytes
            if truncated:
                os.lseek(file_fd, details.st_size - tail_bytes, os.SEEK_SET)
            chunks = []
            remaining = tail_bytes
            while remaining:
                block = os.read(file_fd, min(65536, remaining))
                if not block:
                    break
                chunks.append(block)
                remaining -= len(block)
            return b"".join(chunks), truncated
        finally:
            os.close(file_fd)
    finally:
        for opened_fd in reversed(opened):
            os.close(opened_fd)
        os.close(root_fd)
