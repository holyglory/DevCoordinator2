"""Symlink-safe file operations for a root daemon inside caller-writable trees.

Every descent uses dirfd-relative opens with O_NOFOLLOW so a caller-planted
symlink can never redirect creation, chown, or deletion outside the
repository's .devcoordinator tree.
"""

from __future__ import annotations

import os
from pathlib import Path


class SecureFsError(Exception):
    pass


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
