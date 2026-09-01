#!/usr/bin/env python3
"""Transactionally deploy the canonical universal policy to explicit runtimes.

This manager never discovers a home directory and never changes a target that
was not named in a reviewed plan.  A plan is immutable and content-addressed;
apply and rollback require its printed digest.  POSIX runtimes use direct
absolute symlinks.  Native Windows may use an explicitly selected, one-line
Claude ``@absolute/path`` import wrapper; Codex never falls back to a copy or
wrapper when symlink creation is unavailable.
"""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
import platform
import re
import shlex
import stat
import sys
import uuid
from contextlib import contextmanager
from pathlib import Path, PureWindowsPath
from typing import Any, Callable, Dict, Iterable, Iterator, List, Optional, Tuple, Union

try:  # pragma: no cover - selected by native platform jobs
    import fcntl  # type: ignore
except ImportError:  # pragma: no cover - selected by native Windows
    fcntl = None

try:  # pragma: no cover - selected by native platform jobs
    import msvcrt  # type: ignore
except ImportError:  # pragma: no cover - selected by POSIX
    msvcrt = None


SCHEMA_VERSION = 1
MANAGER_VERSION = "1.0.0"
MANAGER_NAME = "devcoordinator2-universal-policy"
CANONICAL_RELATIVE = Path("reference") / "universal" / "AGENTS.md"
PLAN_NAME = "plan.json"
JOURNAL_NAME = "journal.json"
BACKUP_MARKER = "devcoordinator2-universal-policy-backup"
TEMP_MARKER = "devcoordinator2-universal-policy-new"
WINDOWS_LOCK_NAME = ".devcoordinator2-universal-policy.lock"
TRANSACTION_LOCK_NAME = ".devcoordinator2-universal-policy-transaction.lock"
LOCK_MARKER = b"devcoordinator2-universal-policy-lock-v1\n"
WINDOWS_WRAPPER_MODE = "claude-absolute-import-wrapper-v1"
DIRECT_LINK_MODE = "direct-absolute-symlink"
VALID_ROLES = {"codex", "claude"}
VALID_STATUSES = {
    "planned",
    "applying",
    "applied",
    "verified",
    "apply-failed-rolled-back",
    "apply-failed-rollback-blocked",
    "rolling-back",
    "rolled-back",
    "rollback-blocked",
}
VALID_STAGES = {
    "pending",
    "retained",
    "backing-up",
    "backup-moved",
    "installing",
    "installed",
    "rolled-back",
}
HEX_64 = re.compile(r"^[0-9a-f]{64}$")


class PolicyManagerError(RuntimeError):
    """A fail-closed, user-actionable policy deployment error."""


FaultHook = Callable[[str, str], None]


def lexical_exists(path: Path) -> bool:
    """Return true for ordinary paths and broken symlinks."""

    return os.path.lexists(os.fspath(path))


def _path_key(path: Path) -> str:
    return os.path.normcase(os.path.normpath(os.fspath(path)))


def _same_lexical_path(left: Path, right: Path) -> bool:
    return _path_key(left) == _path_key(right)


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def _require_safe_text(value: str, label: str) -> None:
    if not value or any(ord(character) < 32 or ord(character) == 127 for character in value):
        raise PolicyManagerError(f"{label} contains an empty or control-character path")


def require_absolute(raw: Union[str, Path], label: str) -> Path:
    text = os.fspath(raw)
    _require_safe_text(text, label)
    path = Path(text)
    if not path.is_absolute():
        raise PolicyManagerError(f"{label} must be an explicit absolute path: {text!r}")
    return Path(os.path.abspath(text))


def _is_reparse_point(metadata: os.stat_result) -> bool:
    attributes = getattr(metadata, "st_file_attributes", 0)
    return bool(attributes & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400))


def _identity(metadata: os.stat_result) -> Dict[str, int]:
    result = {
        "device": int(metadata.st_dev),
        "inode": int(metadata.st_ino),
        "mode": int(stat.S_IMODE(metadata.st_mode)),
    }
    attributes = getattr(metadata, "st_file_attributes", None)
    if attributes is not None:
        result["file_attributes"] = int(attributes)
    return result


def _metadata(metadata: os.stat_result) -> Dict[str, int]:
    result = _identity(metadata)
    result.update(
        {
            "uid": int(getattr(metadata, "st_uid", 0)),
            "gid": int(getattr(metadata, "st_gid", 0)),
            "nlink": int(metadata.st_nlink),
            "size": int(metadata.st_size),
            "mtime_ns": int(metadata.st_mtime_ns),
        }
    )
    return result


def _require_real_directory(path: Path, label: str) -> Path:
    try:
        metadata = path.lstat()
    except FileNotFoundError as error:
        raise PolicyManagerError(f"{label} does not exist: {path}") from error
    if stat.S_ISLNK(metadata.st_mode) or _is_reparse_point(metadata) or not stat.S_ISDIR(metadata.st_mode):
        raise PolicyManagerError(f"{label} must be a real directory, not a link or reparse point: {path}")
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise PolicyManagerError(f"{label} cannot be resolved safely: {path}: {error}") from error
    if not _same_lexical_path(path, resolved):
        raise PolicyManagerError(f"{label} must not contain symlinked path components: {path}")

    # ``resolve`` catches ordinary links.  Inspect every existing component as
    # well so native Windows junctions/reparse points fail even when their
    # normalized spelling happens to compare equal.
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        try:
            component = current.lstat()
        except OSError as error:
            raise PolicyManagerError(f"{label} component cannot be inspected: {current}: {error}") from error
        if stat.S_ISLNK(component.st_mode) or _is_reparse_point(component):
            raise PolicyManagerError(f"{label} contains a link or reparse-point component: {current}")
    return path


def canonical_repository(raw: Union[str, Path]) -> Path:
    repository = _require_real_directory(require_absolute(raw, "--repo-root"), "repository root")
    source = repository / CANONICAL_RELATIVE
    _require_real_directory(source.parent, "canonical policy directory")
    try:
        metadata = source.lstat()
    except FileNotFoundError as error:
        raise PolicyManagerError(f"canonical policy is missing: {source}") from error
    if (
        stat.S_ISLNK(metadata.st_mode)
        or _is_reparse_point(metadata)
        or not stat.S_ISREG(metadata.st_mode)
    ):
        raise PolicyManagerError(f"canonical policy must be one real regular file: {source}")
    if metadata.st_nlink != 1:
        raise PolicyManagerError(f"canonical policy must not be hard-linked: {source}")
    return repository


def _stable_file_digest(path: Path, expected: os.stat_result) -> str:
    flags = os.O_RDONLY
    if hasattr(os, "O_BINARY"):
        flags |= os.O_BINARY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(os.fspath(path), flags)
    except OSError as error:
        raise PolicyManagerError(f"cannot safely open regular file: {path}: {error}") from error
    digest = hashlib.sha256()
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or (
            opened.st_dev,
            opened.st_ino,
        ) != (expected.st_dev, expected.st_ino):
            raise PolicyManagerError(f"regular file changed while it was opened: {path}")
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
        after = os.fstat(descriptor)
        compared = ("st_dev", "st_ino", "st_mode", "st_nlink", "st_size", "st_mtime_ns")
        if any(getattr(after, field) != getattr(opened, field) for field in compared):
            raise PolicyManagerError(f"regular file changed while it was hashed: {path}")
    finally:
        os.close(descriptor)
    return digest.hexdigest()


def snapshot_path(path: Path, *, allow_missing: bool = True) -> Dict[str, Any]:
    try:
        before = path.lstat()
    except FileNotFoundError:
        if allow_missing:
            return {"kind": "missing"}
        raise PolicyManagerError(f"required path is missing: {path}")
    if stat.S_ISLNK(before.st_mode):
        reparse_tag = getattr(before, "st_reparse_tag", None)
        symlink_tag = getattr(stat, "IO_REPARSE_TAG_SYMLINK", None)
        if reparse_tag is not None and symlink_tag is not None and reparse_tag != symlink_tag:
            raise PolicyManagerError(f"unsupported non-symlink reparse-point target: {path}")
        raw = os.readlink(os.fspath(path))
        after = path.lstat()
        if _metadata(before) != _metadata(after):
            raise PolicyManagerError(f"symlink changed while it was inspected: {path}")
        return {"kind": "symlink", "link_text": raw, "metadata": _metadata(after)}
    if _is_reparse_point(before):
        raise PolicyManagerError(f"unsupported reparse-point target: {path}")
    if stat.S_ISREG(before.st_mode):
        if before.st_nlink != 1:
            raise PolicyManagerError(f"regular-file target must not be hard-linked: {path}")
        digest = _stable_file_digest(path, before)
        after = path.lstat()
        if _metadata(before) != _metadata(after):
            raise PolicyManagerError(f"regular file changed while it was inspected: {path}")
        return {"kind": "regular-file", "sha256": digest, "metadata": _metadata(after)}
    kind = "directory" if stat.S_ISDIR(before.st_mode) else "special-object"
    raise PolicyManagerError(f"unsupported {kind} target; only missing files, files, and symlinks are safe: {path}")


def source_snapshot(repository: Path) -> Dict[str, Any]:
    checked = canonical_repository(repository)
    if not _same_lexical_path(checked, repository):
        raise PolicyManagerError(f"canonical repository identity changed: {repository}")
    source = checked / CANONICAL_RELATIVE
    nodes = []
    for relative in (Path("."), Path("reference"), Path("reference/universal")):
        node = checked if relative == Path(".") else checked / relative
        _require_real_directory(node, f"canonical source component {relative.as_posix()}")
        nodes.append({"relative": relative.as_posix(), "identity": _identity(node.lstat())})
    policy = snapshot_path(source, allow_missing=False)
    if policy["kind"] != "regular-file":
        raise PolicyManagerError(f"canonical policy is not a regular file: {source}")
    return {
        "path": str(source),
        "nodes": nodes,
        "policy": policy,
    }


def require_source_snapshot(repository: Path, expected: Dict[str, Any]) -> None:
    try:
        actual = source_snapshot(repository)
    except (OSError, PolicyManagerError) as error:
        raise PolicyManagerError(f"canonical policy changed after planning: {error}") from error
    if actual != expected:
        raise PolicyManagerError(
            f"canonical policy identity or bytes changed after planning: {repository / CANONICAL_RELATIVE}"
        )


def host_family() -> str:
    if os.name == "nt":
        return "windows"
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("linux"):
        try:
            release = Path("/proc/sys/kernel/osrelease").read_text(encoding="utf-8").casefold()
        except OSError:
            release = platform.release().casefold()
        return "wsl" if "microsoft" in release else "linux"
    return "posix-other"


def serialize_windows_import_path(raw: str) -> str:
    """Return deterministic Claude import syntax for a local Windows path."""

    _require_safe_text(raw, "Windows canonical policy path")
    path = PureWindowsPath(raw)
    if not path.is_absolute() or not path.drive:
        raise PolicyManagerError(f"Windows import path must be drive-absolute: {raw!r}")
    if str(path).startswith("\\\\") or path.drive.startswith("\\\\"):
        raise PolicyManagerError("Windows import wrappers reject UNC/network policy sources")
    normalized = path.as_posix()
    if not re.fullmatch(r"[A-Za-z]:/.+", normalized):
        raise PolicyManagerError(f"ambiguous Windows import path: {raw!r}")
    return normalized[0].upper() + normalized[1:]


def claude_wrapper_bytes(source: Union[str, Path], *, windows: bool) -> bytes:
    text = os.fspath(source)
    _require_safe_text(text, "canonical policy path")
    if windows:
        serialized = serialize_windows_import_path(text)
    else:
        path = Path(text)
        if not path.is_absolute():
            raise PolicyManagerError("Claude import wrapper source must be absolute")
        serialized = path.as_posix()
    return ("@" + serialized + "\n").encode("utf-8")


def _parent_snapshot(parent: Path) -> Dict[str, int]:
    _require_real_directory(parent, "target parent")
    return _identity(parent.lstat())


def _validate_target(
    raw: Union[str, Path],
    *,
    role: str,
    mode: str,
    repository: Path,
    platform_family: str,
) -> Dict[str, Any]:
    destination = require_absolute(raw, f"--{role}-target")
    expected_name = "AGENTS.md" if role == "codex" else "CLAUDE.md"
    if destination.name != expected_name:
        raise PolicyManagerError(
            f"{role} target must name {expected_name} exactly: {destination}"
        )
    parent = _require_real_directory(destination.parent, f"{role} target parent")
    if _is_within(destination, repository) or _is_within(repository, parent):
        raise PolicyManagerError(f"runtime policy target must be outside the canonical repository: {destination}")
    if role == "codex" and mode != DIRECT_LINK_MODE:
        raise PolicyManagerError("Codex global policy supports direct-link mode only")
    if mode == WINDOWS_WRAPPER_MODE and (role != "claude" or platform_family != "windows"):
        raise PolicyManagerError(
            "Claude import-wrapper mode is explicit and available only on native Windows"
        )
    if mode not in {DIRECT_LINK_MODE, WINDOWS_WRAPPER_MODE}:
        raise PolicyManagerError(f"unsupported policy deployment mode: {mode}")
    return {
        "role": role,
        "mode": mode,
        "destination": str(destination),
        "parent": str(parent),
        "parent_identity": _parent_snapshot(parent),
    }


def _open_windows_directory_handle(path: Path) -> int:
    """Open a directory without delete sharing so its name cannot be swapped."""

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    create_file = kernel32.CreateFileW
    create_file.argtypes = [
        ctypes.c_wchar_p,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_void_p,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_void_p,
    ]
    create_file.restype = ctypes.c_void_p
    handle = create_file(
        str(path),
        0x00000080,  # FILE_READ_ATTRIBUTES
        0x00000001 | 0x00000002,  # FILE_SHARE_READ | FILE_SHARE_WRITE; no delete sharing
        None,
        3,  # OPEN_EXISTING
        0x02000000 | 0x00200000,  # BACKUP_SEMANTICS | OPEN_REPARSE_POINT
        None,
    )
    invalid = ctypes.c_void_p(-1).value
    if handle in (None, invalid):
        error = ctypes.get_last_error()
        raise PolicyManagerError(
            f"cannot hold no-reparse Windows directory handle for {path}: "
            f"{ctypes.FormatError(error)}"
        )
    return int(handle)


def _close_windows_handle(handle: int) -> None:
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    close = kernel32.CloseHandle
    close.argtypes = [ctypes.c_void_p]
    close.restype = ctypes.c_int
    if not close(ctypes.c_void_p(handle)):
        error = ctypes.get_last_error()
        raise PolicyManagerError(f"cannot close Windows directory handle: {ctypes.FormatError(error)}")


def _open_lock_stream(path: Path) -> Tuple[Any, bool]:
    """Open one manager-owned regular lock file without following replacements."""

    created = False
    flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
    try:
        descriptor = os.open(os.fspath(path), flags, 0o600)
        created = True
    except FileExistsError:
        before = path.lstat()
        if (
            stat.S_ISLNK(before.st_mode)
            or _is_reparse_point(before)
            or not stat.S_ISREG(before.st_mode)
            or before.st_nlink != 1
        ):
            raise PolicyManagerError(f"policy lock must be one real regular file: {path}")
        open_flags = os.O_RDWR | getattr(os, "O_BINARY", 0)
        if hasattr(os, "O_NOFOLLOW"):
            open_flags |= os.O_NOFOLLOW
        descriptor = os.open(os.fspath(path), open_flags)
        opened = os.fstat(descriptor)
        if _identity(opened) != _identity(before) or opened.st_nlink != 1:
            os.close(descriptor)
            raise PolicyManagerError(f"policy lock changed while it was opened: {path}")
    stream = os.fdopen(descriptor, "r+b", buffering=0)
    try:
        if created:
            stream.write(LOCK_MARKER)
            stream.flush()
            os.fsync(stream.fileno())
        stream.seek(0)
        if stream.read() != LOCK_MARKER:
            raise PolicyManagerError(f"policy lock has unexpected contents: {path}")
        stream.seek(0)
        return stream, created
    except BaseException:
        stream.close()
        raise


class ParentLock:
    __slots__ = (
        "path",
        "identity",
        "descriptor",
        "stream",
        "windows_lock_path",
        "windows_directory_handle",
    )

    def __init__(self, path: Path, identity: Dict[str, int]) -> None:
        self.path = path
        self.identity = identity
        self.descriptor: Optional[int] = None
        self.stream: Any = None
        self.windows_lock_path: Optional[Path] = None
        self.windows_directory_handle: Optional[int] = None


def _require_parent_identity(lock: ParentLock) -> None:
    actual = _parent_snapshot(lock.path)
    if actual != lock.identity:
        raise PolicyManagerError(f"target parent identity changed after planning: {lock.path}")
    if lock.descriptor is not None and os.name != "nt":
        opened = os.fstat(lock.descriptor)
        if _identity(opened) != lock.identity:
            raise PolicyManagerError(f"locked target parent descriptor changed: {lock.path}")


@contextmanager
def lock_parents(specifications: Iterable[Tuple[Path, Dict[str, int]]]) -> Iterator[Dict[str, ParentLock]]:
    unique: Dict[str, Tuple[Path, Dict[str, int]]] = {}
    for parent, identity in specifications:
        key = _path_key(parent)
        previous = unique.get(key)
        if previous is not None and previous[1] != identity:
            raise PolicyManagerError(f"conflicting target-parent snapshots: {parent}")
        unique[key] = (parent, identity)
    locks: Dict[str, ParentLock] = {}
    acquired: List[ParentLock] = []
    try:
        for key in sorted(unique):
            parent, identity = unique[key]
            lock = ParentLock(parent, identity)
            acquired.append(lock)
            if os.name == "nt":  # pragma: no cover - native Windows CI
                if msvcrt is None:
                    raise PolicyManagerError("native Windows locking support is unavailable")
                lock.windows_directory_handle = _open_windows_directory_handle(parent)
                _require_parent_identity(lock)
                lock_path = parent / WINDOWS_LOCK_NAME
                stream, _ = _open_lock_stream(lock_path)
                msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
                lock.stream = stream
                lock.windows_lock_path = lock_path
            else:
                if fcntl is None:
                    raise PolicyManagerError("POSIX directory locking support is unavailable")
                flags = os.O_RDONLY
                if hasattr(os, "O_DIRECTORY"):
                    flags |= os.O_DIRECTORY
                if hasattr(os, "O_NOFOLLOW"):
                    flags |= os.O_NOFOLLOW
                descriptor = os.open(os.fspath(parent), flags)
                fcntl.flock(descriptor, fcntl.LOCK_EX)
                lock.descriptor = descriptor
            _require_parent_identity(lock)
            locks[key] = lock
        yield locks
    finally:
        for lock in reversed(acquired):
            if os.name == "nt":  # pragma: no cover - native Windows CI
                try:
                    if lock.stream is not None:
                        assert msvcrt is not None
                        try:
                            lock.stream.seek(0)
                            msvcrt.locking(lock.stream.fileno(), msvcrt.LK_UNLCK, 1)
                        finally:
                            lock.stream.close()
                finally:
                    if lock.windows_directory_handle is not None:
                        _close_windows_handle(lock.windows_directory_handle)
            elif lock.descriptor is not None:
                assert fcntl is not None
                try:
                    fcntl.flock(lock.descriptor, fcntl.LOCK_UN)
                finally:
                    os.close(lock.descriptor)


class TransactionLock:
    __slots__ = ("path", "identity", "descriptor", "stream", "windows_directory_handle")

    def __init__(self, path: Path) -> None:
        self.path = path
        self.identity: Dict[str, int] = {}
        self.descriptor: Optional[int] = None
        self.stream: Any = None
        self.windows_directory_handle: Optional[int] = None


def _require_transaction_identity(
    lock: TransactionLock,
    expected: Dict[str, int],
    *,
    require_path: bool = True,
) -> None:
    if lock.identity != expected:
        raise PolicyManagerError("held transaction directory identity does not match the reviewed plan")
    if lock.descriptor is not None and _identity(os.fstat(lock.descriptor)) != expected:
        raise PolicyManagerError("held transaction directory descriptor changed identity")
    if require_path:
        try:
            current = lock.path.lstat()
        except FileNotFoundError as error:
            raise PolicyManagerError("transaction directory disappeared during the operation") from error
        if _identity(current) != expected or stat.S_ISLNK(current.st_mode) or _is_reparse_point(current):
            raise PolicyManagerError("transaction directory identity changed during the operation")


@contextmanager
def lock_transaction(transaction_raw: Union[str, Path]) -> Iterator[TransactionLock]:
    path = _require_real_directory(
        require_absolute(transaction_raw, "--transaction-dir"), "transaction directory"
    )
    lock = TransactionLock(path)
    try:
        if os.name == "nt":  # pragma: no cover - native Windows CI
            if msvcrt is None:
                raise PolicyManagerError("native Windows transaction locking support is unavailable")
            lock.windows_directory_handle = _open_windows_directory_handle(path)
            lock.identity = _identity(path.lstat())
            stream, _ = _open_lock_stream(path / TRANSACTION_LOCK_NAME)
            msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
            lock.stream = stream
        else:
            if fcntl is None:
                raise PolicyManagerError("POSIX transaction locking support is unavailable")
            flags = os.O_RDONLY
            if hasattr(os, "O_DIRECTORY"):
                flags |= os.O_DIRECTORY
            if hasattr(os, "O_NOFOLLOW"):
                flags |= os.O_NOFOLLOW
            lock.descriptor = os.open(os.fspath(path), flags)
            lock.identity = _identity(os.fstat(lock.descriptor))
            fcntl.flock(lock.descriptor, fcntl.LOCK_EX)
        _require_transaction_identity(lock, lock.identity)
        yield lock
    finally:
        if os.name == "nt":  # pragma: no cover - native Windows CI
            try:
                if lock.stream is not None:
                    assert msvcrt is not None
                    try:
                        lock.stream.seek(0)
                        msvcrt.locking(lock.stream.fileno(), msvcrt.LK_UNLCK, 1)
                    finally:
                        lock.stream.close()
            finally:
                if lock.windows_directory_handle is not None:
                    _close_windows_handle(lock.windows_directory_handle)
        elif lock.descriptor is not None:
            assert fcntl is not None
            try:
                fcntl.flock(lock.descriptor, fcntl.LOCK_UN)
            finally:
                os.close(lock.descriptor)


def _lock_for(entry: Dict[str, Any], locks: Dict[str, ParentLock]) -> ParentLock:
    return locks[_path_key(Path(entry["parent"]))]


def _fsync_directory(lock: ParentLock) -> None:
    if lock.descriptor is not None:
        try:
            os.fsync(lock.descriptor)
        except OSError:
            pass
        return
    try:
        descriptor = os.open(os.fspath(lock.path), os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(descriptor)
    except OSError:
        pass
    finally:
        os.close(descriptor)


def _move_noreplace_in_parent(lock: ParentLock, source_name: str, destination_name: str) -> None:
    """Atomically move one immediate child and fail if the destination exists.

    Ordinary POSIX rename and ``os.replace`` are intentionally forbidden here:
    both can destroy a writer-created collision after our last observation.
    Linux/WSL and macOS use their native rename-no-replace primitive; Windows
    uses ``MoveFileExW`` without ``MOVEFILE_REPLACE_EXISTING``.
    """

    _require_parent_identity(lock)
    for name in (source_name, destination_name):
        if not name or Path(name).name != name or name in {".", ".."}:
            raise PolicyManagerError(f"unsafe adjacent transaction name: {name!r}")
    if os.name == "nt":  # pragma: no cover - native Windows CI
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        move = kernel32.MoveFileExW
        move.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint32]
        move.restype = ctypes.c_int
        # MOVEFILE_WRITE_THROUGH is durability-oriented and, unlike
        # MOVEFILE_REPLACE_EXISTING, does not permit overwriting the target.
        result = move(
            str(lock.path / source_name),
            str(lock.path / destination_name),
            0x00000008,
        )
        if not result:
            error = ctypes.get_last_error()
            if lexical_exists(lock.path / destination_name):
                raise PolicyManagerError(
                    f"atomic no-replace move refused a destination collision: "
                    f"{lock.path / destination_name}"
                )
            raise OSError(error, ctypes.FormatError(error), str(lock.path / source_name))
    else:
        if lock.descriptor is None:
            raise PolicyManagerError("POSIX no-replace move lacks a locked directory descriptor")
        libc = ctypes.CDLL(None, use_errno=True)
        source_bytes = os.fsencode(source_name)
        destination_bytes = os.fsencode(destination_name)
        if platform.system() == "Linux" and hasattr(libc, "renameat2"):
            function = libc.renameat2
            function.argtypes = [
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_uint,
            ]
            function.restype = ctypes.c_int
            result = function(
                lock.descriptor,
                source_bytes,
                lock.descriptor,
                destination_bytes,
                0x00000001,  # RENAME_NOREPLACE
            )
        elif platform.system() == "Darwin" and hasattr(libc, "renameatx_np"):
            function = libc.renameatx_np
            function.argtypes = [
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_uint,
            ]
            function.restype = ctypes.c_int
            result = function(
                lock.descriptor,
                source_bytes,
                lock.descriptor,
                destination_bytes,
                0x00000004,  # RENAME_EXCL
            )
        else:
            raise PolicyManagerError(
                "this POSIX platform lacks an atomic rename-no-replace primitive"
            )
        if result != 0:
            error = ctypes.get_errno()
            if error == errno.EEXIST or lexical_exists(
                lock.path / destination_name
            ):
                raise PolicyManagerError(
                    f"atomic no-replace move refused a destination collision: "
                    f"{lock.path / destination_name}"
                )
            raise OSError(error, os.strerror(error), source_name)
    _fsync_directory(lock)
    _require_parent_identity(lock)


def _atomic_bytes_write(path: Path, payload: bytes, mode: int = 0o600) -> None:
    temporary = path.parent / f".{path.name}.tmp-{os.getpid()}-{uuid.uuid4().hex}"
    descriptor = os.open(os.fspath(temporary), os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        if os.name != "nt":
            os.chmod(path, mode)
        try:
            directory = os.open(os.fspath(path.parent), os.O_RDONLY)
        except OSError:
            directory = -1
        if directory >= 0:
            try:
                os.fsync(directory)
            except OSError:
                pass
            finally:
                os.close(directory)
    finally:
        if lexical_exists(temporary):
            temporary.unlink()


def _atomic_bytes_write_held(
    lock: TransactionLock,
    name: str,
    payload: bytes,
    mode: int = 0o600,
) -> None:
    """Atomically write one transaction child through its held directory."""

    if not name or Path(name).name != name:
        raise PolicyManagerError(f"unsafe transaction child name: {name!r}")
    if lock.descriptor is None:
        # Native Windows holds a no-delete-share directory handle, so its
        # pathname cannot be renamed or replaced during this operation.
        _atomic_bytes_write(lock.path / name, payload, mode)
        return
    if os.open not in getattr(os, "supports_dir_fd", set()) or os.rename not in getattr(
        os, "supports_dir_fd", set()
    ):
        raise PolicyManagerError("this POSIX runtime lacks directory-anchored atomic writes")
    temporary_name = f".{name}.tmp-{os.getpid()}-{uuid.uuid4().hex}"
    descriptor = os.open(
        temporary_name,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL,
        mode,
        dir_fd=lock.descriptor,
    )
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.rename(
            temporary_name,
            name,
            src_dir_fd=lock.descriptor,
            dst_dir_fd=lock.descriptor,
        )
        os.fsync(lock.descriptor)
    finally:
        try:
            os.unlink(temporary_name, dir_fd=lock.descriptor)
        except FileNotFoundError:
            pass


def _canonical_json(value: Dict[str, Any]) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode("utf-8")


def _write_journal(
    transaction: Path,
    journal: Dict[str, Any],
    transaction_lock: Optional[TransactionLock] = None,
    *,
    allow_detached: bool = False,
) -> None:
    expected = journal.get("transaction_identity")
    if not isinstance(expected, dict):
        raise PolicyManagerError("transaction journal lacks its directory identity")
    if transaction_lock is None:
        current = transaction.lstat()
        if _identity(current) != expected or stat.S_ISLNK(current.st_mode) or _is_reparse_point(current):
            raise PolicyManagerError("transaction directory identity changed before journal write")
        _atomic_bytes_write(transaction / JOURNAL_NAME, _canonical_json(journal))
        after = transaction.lstat()
        if _identity(after) != expected:
            raise PolicyManagerError("transaction directory identity changed during journal write")
        return
    _require_transaction_identity(
        transaction_lock,
        expected,
        require_path=not allow_detached,
    )
    _atomic_bytes_write_held(transaction_lock, JOURNAL_NAME, _canonical_json(journal))
    _require_transaction_identity(
        transaction_lock,
        expected,
        require_path=not allow_detached,
    )


def _transaction_directory(raw: Union[str, Path], repository: Path, parents: Iterable[Path]) -> Path:
    requested = require_absolute(raw, "--transaction-dir")
    if lexical_exists(requested):
        raise PolicyManagerError(f"transaction directory already exists: {requested}")
    parent = _require_real_directory(requested.parent, "transaction parent")
    transaction = parent / requested.name
    if not transaction.name or transaction.name in {".", ".."}:
        raise PolicyManagerError("transaction directory needs a safe leaf name")
    relationships = [repository, *parents]
    if any(_is_within(transaction, item) or _is_within(item, transaction) for item in relationships):
        raise PolicyManagerError(
            "transaction directory must be outside the repository and every target parent"
        )
    os.mkdir(transaction, 0o700)
    if os.name != "nt":
        os.chmod(transaction, 0o700)
    stream, _ = _open_lock_stream(transaction / TRANSACTION_LOCK_NAME)
    stream.close()
    return transaction


def _desired(entry: Dict[str, Any], source: Path, platform_family: str) -> Dict[str, Any]:
    if entry["mode"] == DIRECT_LINK_MODE:
        return {"kind": "symlink", "link_text": str(source)}
    payload = claude_wrapper_bytes(source, windows=platform_family == "windows")
    return {
        "kind": "regular-file",
        "format": WINDOWS_WRAPPER_MODE,
        "sha256": hashlib.sha256(payload).hexdigest(),
        "size": len(payload),
    }


def _matches_desired(path: Path, entry: Dict[str, Any]) -> bool:
    try:
        snapshot = snapshot_path(path)
    except PolicyManagerError:
        return False
    desired = entry["desired"]
    if desired["kind"] == "symlink":
        if snapshot["kind"] != "symlink":
            return False
        actual_text = snapshot["link_text"]
        expected_text = desired["link_text"]
        if actual_text == expected_text:
            return True
        if os.name == "nt":  # pragma: no cover - native Windows CI
            # Python may surface a Win32 symlink's substitution name with an
            # extended-path prefix even when the stored print name was the
            # ordinary absolute path supplied to os.symlink.
            def ordinary_windows_path(value: str) -> str:
                if value.startswith("\\\\?\\UNC\\"):
                    return "\\\\" + value[8:]
                if value.startswith("\\\\?\\"):
                    return value[4:]
                return value

            return os.path.normcase(os.path.normpath(ordinary_windows_path(actual_text))) == os.path.normcase(
                os.path.normpath(expected_text)
            )
        return False
    return (
        snapshot["kind"] == "regular-file"
        and snapshot["sha256"] == desired["sha256"]
        and snapshot["metadata"]["size"] == desired["size"]
    )


def _entry_paths(plan_id: str, index: int, destination: Path) -> Tuple[Path, Path]:
    suffix = f"{plan_id}-{index:03d}"
    backup = destination.parent / f".{destination.name}.{BACKUP_MARKER}-{suffix}"
    temporary = destination.parent / f".{destination.name}.{TEMP_MARKER}-{suffix}"
    return backup, temporary


def create_plan(
    repository_raw: Union[str, Path],
    transaction_raw: Union[str, Path],
    *,
    codex_targets: Iterable[Union[str, Path]] = (),
    claude_targets: Iterable[Union[str, Path]] = (),
    claude_wrapper_targets: Iterable[Union[str, Path]] = (),
) -> Tuple[Dict[str, Any], str]:
    repository = canonical_repository(repository_raw)
    family = host_family()
    specifications: List[Dict[str, Any]] = []
    for raw in codex_targets:
        specifications.append(
            _validate_target(
                raw,
                role="codex",
                mode=DIRECT_LINK_MODE,
                repository=repository,
                platform_family=family,
            )
        )
    for raw in claude_targets:
        specifications.append(
            _validate_target(
                raw,
                role="claude",
                mode=DIRECT_LINK_MODE,
                repository=repository,
                platform_family=family,
            )
        )
    for raw in claude_wrapper_targets:
        specifications.append(
            _validate_target(
                raw,
                role="claude",
                mode=WINDOWS_WRAPPER_MODE,
                repository=repository,
                platform_family=family,
            )
        )
    if not specifications:
        raise PolicyManagerError("plan requires at least one explicit runtime policy target")
    seen: Dict[str, str] = {}
    for specification in specifications:
        destination = Path(specification["destination"])
        key = _path_key(destination)
        if key in seen:
            raise PolicyManagerError(
                f"duplicate runtime policy target: {destination} (already named for {seen[key]})"
            )
        seen[key] = specification["role"]
    specifications.sort(key=lambda item: (_path_key(Path(item["destination"])), item["role"]))
    parents = [Path(item["parent"]) for item in specifications]
    source_expected = source_snapshot(repository)
    plan_id = uuid.uuid4().hex

    with lock_parents((parent, item["parent_identity"]) for parent, item in zip(parents, specifications)) as locks:
        require_source_snapshot(repository, source_expected)
        transaction = _transaction_directory(transaction_raw, repository, parents)
        try:
            entries: List[Dict[str, Any]] = []
            source = repository / CANONICAL_RELATIVE
            for index, specification in enumerate(specifications):
                lock = _lock_for(specification, locks)
                _require_parent_identity(lock)
                destination = Path(specification["destination"])
                before = snapshot_path(destination)
                backup, temporary = _entry_paths(plan_id, index, destination)
                if lexical_exists(backup) or lexical_exists(temporary):
                    raise PolicyManagerError(
                        f"manager-owned backup or temporary path already exists: {backup} or {temporary}"
                    )
                entry = dict(specification)
                entry.update(
                    {
                        "id": f"target-{index + 1:03d}",
                        "order": index,
                        "before": before,
                        "desired": _desired(specification, source, family),
                        "backup": str(backup),
                        "temporary": str(temporary),
                    }
                )
                entry["action"] = "retain" if _matches_desired(destination, entry) else "replace"
                entries.append(entry)
            require_source_snapshot(repository, source_expected)
            transaction_identity = _identity(transaction.lstat())
            plan = {
                "schema_version": SCHEMA_VERSION,
                "manager": MANAGER_NAME,
                "manager_version": MANAGER_VERSION,
                "plan_id": plan_id,
                "platform": {
                    "family": family,
                    "os_name": os.name,
                    "python": platform.python_version(),
                },
                "repository_root": str(repository),
                "canonical_source": source_expected,
                "transaction_dir": str(transaction),
                "transaction_identity": transaction_identity,
                "entries": entries,
            }
            plan_payload = _canonical_json(plan)
            digest = hashlib.sha256(plan_payload).hexdigest()
            _atomic_bytes_write(transaction / PLAN_NAME, plan_payload, 0o400)
            journal = {
                "schema_version": SCHEMA_VERSION,
                "manager": MANAGER_NAME,
                "manager_version": MANAGER_VERSION,
                "plan_id": plan_id,
                "plan_sha256": digest,
                "transaction_identity": transaction_identity,
                "status": "planned",
                "entries": [
                    {"id": entry["id"], "stage": "pending", "changed": False}
                    for entry in entries
                ],
            }
            _write_journal(transaction, journal)
            require_source_snapshot(repository, source_expected)
            for entry in entries:
                _require_parent_identity(_lock_for(entry, locks))
                if snapshot_path(Path(entry["destination"])) != entry["before"]:
                    raise PolicyManagerError(
                        f"target changed while its plan was being persisted: {entry['destination']}"
                    )
            return plan, digest
        except BaseException:
            # Planning has not changed runtime targets.  Remove only artifacts
            # this invocation just created, and only while they retain the
            # manager-owned names inside the new transaction directory.
            try:
                for name in (JOURNAL_NAME, PLAN_NAME, TRANSACTION_LOCK_NAME):
                    path = transaction / name
                    if lexical_exists(path) and path.is_file() and not path.is_symlink():
                        path.unlink()
                transaction.rmdir()
            except OSError:
                pass
            raise


def _validate_snapshot(value: Any, label: str) -> None:
    if not isinstance(value, dict) or value.get("kind") not in {"missing", "regular-file", "symlink"}:
        raise PolicyManagerError(f"invalid {label} snapshot")
    if value["kind"] == "missing":
        if set(value) != {"kind"}:
            raise PolicyManagerError(f"invalid missing {label} snapshot")
        return
    metadata = value.get("metadata")
    required = {"device", "inode", "mode", "uid", "gid", "nlink", "size", "mtime_ns"}
    if not isinstance(metadata, dict) or not required.issubset(metadata):
        raise PolicyManagerError(f"invalid metadata in {label} snapshot")
    if value["kind"] == "regular-file" and not HEX_64.fullmatch(str(value.get("sha256", ""))):
        raise PolicyManagerError(f"invalid digest in {label} snapshot")
    if value["kind"] == "symlink" and not isinstance(value.get("link_text"), str):
        raise PolicyManagerError(f"invalid link text in {label} snapshot")


def _validate_loaded_plan(plan: Dict[str, Any], transaction: Path) -> None:
    if plan.get("schema_version") != SCHEMA_VERSION or plan.get("manager") != MANAGER_NAME:
        raise PolicyManagerError("unsupported or foreign global-policy plan")
    plan_id = plan.get("plan_id")
    if not isinstance(plan_id, str) or not re.fullmatch(r"[0-9a-f]{32}", plan_id):
        raise PolicyManagerError("invalid global-policy plan ID")
    recorded_transaction = require_absolute(str(plan.get("transaction_dir", "")), "plan transaction_dir")
    if not _same_lexical_path(recorded_transaction, transaction):
        raise PolicyManagerError("plan was moved away from its bound transaction directory")
    if _identity(transaction.lstat()) != plan.get("transaction_identity"):
        raise PolicyManagerError("transaction directory identity changed after planning")
    repository = require_absolute(str(plan.get("repository_root", "")), "plan repository_root")
    source = repository / CANONICAL_RELATIVE
    source_record = plan.get("canonical_source")
    if not isinstance(source_record, dict) or source_record.get("path") != str(source):
        raise PolicyManagerError("plan canonical source is malformed or not the required repository path")
    entries = plan.get("entries")
    if not isinstance(entries, list) or not entries:
        raise PolicyManagerError("plan has no explicit targets")
    seen: set[str] = set()
    family = plan.get("platform", {}).get("family")
    if family not in {"windows", "macos", "linux", "wsl", "posix-other"}:
        raise PolicyManagerError("plan platform family is invalid")
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict) or entry.get("id") != f"target-{index + 1:03d}" or entry.get("order") != index:
            raise PolicyManagerError("plan target ordering or ID is invalid")
        role = entry.get("role")
        mode = entry.get("mode")
        if role not in VALID_ROLES:
            raise PolicyManagerError("plan target role is invalid")
        if mode == WINDOWS_WRAPPER_MODE and not (role == "claude" and family == "windows"):
            raise PolicyManagerError("plan contains an invalid import-wrapper target")
        if mode not in {DIRECT_LINK_MODE, WINDOWS_WRAPPER_MODE} or (role == "codex" and mode != DIRECT_LINK_MODE):
            raise PolicyManagerError("plan target mode is invalid")
        destination = require_absolute(str(entry.get("destination", "")), "plan destination")
        parent = require_absolute(str(entry.get("parent", "")), "plan target parent")
        if destination.parent != parent:
            raise PolicyManagerError("plan target is not an immediate child of its bound parent")
        expected_name = "AGENTS.md" if role == "codex" else "CLAUDE.md"
        if destination.name != expected_name:
            raise PolicyManagerError("plan target basename does not match its runtime")
        key = _path_key(destination)
        if key in seen:
            raise PolicyManagerError("plan contains duplicate targets")
        seen.add(key)
        if not isinstance(entry.get("parent_identity"), dict):
            raise PolicyManagerError("plan target-parent identity is missing")
        _validate_snapshot(entry.get("before"), "before")
        desired = entry.get("desired")
        if not isinstance(desired, dict):
            raise PolicyManagerError("plan desired state is invalid")
        if mode == DIRECT_LINK_MODE:
            if desired != {"kind": "symlink", "link_text": str(source)}:
                raise PolicyManagerError("plan direct link does not name the canonical source exactly")
        else:
            payload = claude_wrapper_bytes(str(source), windows=True)
            expected_desired = {
                "kind": "regular-file",
                "format": WINDOWS_WRAPPER_MODE,
                "sha256": hashlib.sha256(payload).hexdigest(),
                "size": len(payload),
            }
            if desired != expected_desired:
                raise PolicyManagerError("plan Claude wrapper bytes are not canonical")
        expected_backup, expected_temporary = _entry_paths(plan_id, index, destination)
        if entry.get("backup") != str(expected_backup) or entry.get("temporary") != str(expected_temporary):
            raise PolicyManagerError("plan backup or temporary path was changed")
        if entry.get("action") not in {"retain", "replace"}:
            raise PolicyManagerError("plan target action is invalid")


def _load_transaction(
    transaction_raw: Union[str, Path],
    expected_digest: str,
    *,
    transaction_lock: Optional[TransactionLock] = None,
) -> Tuple[Path, Dict[str, Any], Dict[str, Any]]:
    if not HEX_64.fullmatch(expected_digest):
        raise PolicyManagerError("--plan-digest must be the exact 64-character digest printed by plan")
    transaction = _require_real_directory(
        require_absolute(transaction_raw, "--transaction-dir"), "transaction directory"
    )
    if transaction_lock is not None:
        _require_transaction_identity(transaction_lock, transaction_lock.identity)
    plan_path = transaction / PLAN_NAME
    journal_path = transaction / JOURNAL_NAME
    for path, label in ((plan_path, "plan"), (journal_path, "journal")):
        try:
            metadata = path.lstat()
        except FileNotFoundError as error:
            raise PolicyManagerError(f"transaction {label} is missing: {path}") from error
        if stat.S_ISLNK(metadata.st_mode) or _is_reparse_point(metadata) or not stat.S_ISREG(metadata.st_mode):
            raise PolicyManagerError(f"transaction {label} must be a real regular file: {path}")
        if metadata.st_nlink != 1:
            raise PolicyManagerError(f"transaction {label} must not be hard-linked: {path}")
    plan_payload = plan_path.read_bytes()
    actual_digest = hashlib.sha256(plan_payload).hexdigest()
    if actual_digest != expected_digest:
        raise PolicyManagerError("reviewed plan digest does not match the persisted immutable plan")
    try:
        plan = json.loads(plan_payload.decode("utf-8"))
        journal = json.loads(journal_path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PolicyManagerError(f"transaction metadata is malformed: {error}") from error
    if not isinstance(plan, dict) or not isinstance(journal, dict):
        raise PolicyManagerError("transaction metadata must contain JSON objects")
    _validate_loaded_plan(plan, transaction)
    if transaction_lock is not None:
        _require_transaction_identity(transaction_lock, plan["transaction_identity"])
    if (
        journal.get("schema_version") != SCHEMA_VERSION
        or journal.get("manager") != MANAGER_NAME
        or journal.get("manager_version") != MANAGER_VERSION
        or journal.get("plan_id") != plan["plan_id"]
        or journal.get("plan_sha256") != expected_digest
        or journal.get("transaction_identity") != plan["transaction_identity"]
        or journal.get("status") not in VALID_STATUSES
    ):
        raise PolicyManagerError("transaction journal does not match the reviewed plan")
    journal_entries = journal.get("entries")
    if not isinstance(journal_entries, list) or len(journal_entries) != len(plan["entries"]):
        raise PolicyManagerError("transaction journal target count does not match the plan")
    for expected, actual in zip(plan["entries"], journal_entries):
        if (
            not isinstance(actual, dict)
            or actual.get("id") != expected["id"]
            or actual.get("stage") not in VALID_STAGES
            or not isinstance(actual.get("changed"), bool)
        ):
            raise PolicyManagerError("transaction journal target state is malformed")
    return transaction, plan, journal


def render_shell_command(arguments: Iterable[str], *, windows: bool) -> str:
    values = list(arguments)
    if windows:
        quote = lambda value: "'" + value.replace("'", "''") + "'"
        return "& " + " ".join(quote(value) for value in values)
    return " ".join(shlex.quote(value) for value in values)


def render_plan(plan: Dict[str, Any], digest: str) -> str:
    lines = [
        f"plan_id: {plan['plan_id']}",
        f"plan_sha256: {digest}",
        f"canonical_source: {plan['canonical_source']['path']}",
        f"canonical_sha256: {plan['canonical_source']['policy']['sha256']}",
        f"transaction_dir: {plan['transaction_dir']}",
    ]
    for entry in plan["entries"]:
        lines.append(
            f"{entry['action']:<7} {entry['role']:<6} {entry['mode']:<35} "
            f"{entry['before']['kind']:<12} {entry['destination']}"
        )
    lines.extend(
        [
            "filesystem_activation: not-applied",
            "runtime_activation: not-observed (restart and inspect the runtime instruction surface after apply)",
        ]
    )
    if any(entry["mode"] == WINDOWS_WRAPPER_MODE for entry in plan["entries"]):
        lines.append(
            "claude_external_import: may require first-use approval; confirm the canonical file with /memory"
        )
    arguments = [
        str(Path(sys.executable)),
        str(Path(__file__).resolve()),
        "apply",
        "--transaction-dir",
        plan["transaction_dir"],
        "--plan-digest",
        digest,
    ]
    command = render_shell_command(
        arguments,
        windows=plan.get("platform", {}).get("family") == "windows",
    )
    lines.append("apply: " + command)
    return "\n".join(lines)


def _set_entry_stage(
    transaction: Path,
    journal: Dict[str, Any],
    index: int,
    stage: str,
    *,
    changed: Optional[bool] = None,
    transaction_lock: Optional[TransactionLock] = None,
    allow_detached: bool = False,
) -> None:
    journal["entries"][index]["stage"] = stage
    if changed is not None:
        journal["entries"][index]["changed"] = changed
    _write_journal(
        transaction,
        journal,
        transaction_lock,
        allow_detached=allow_detached,
    )


def _call_fault(hook: Optional[FaultHook], point: str, entry_id: str) -> None:
    if hook is not None:
        hook(point, entry_id)


def _install_desired(lock: ParentLock, entry: Dict[str, Any], source: Path) -> None:
    destination = Path(entry["destination"])
    temporary = Path(entry["temporary"])
    if lexical_exists(destination) or lexical_exists(temporary):
        raise PolicyManagerError(f"destination or temporary path is not empty before install: {destination}")
    if entry["mode"] == DIRECT_LINK_MODE:
        if lock.descriptor is not None and os.symlink in getattr(os, "supports_dir_fd", set()):
            os.symlink(str(source), temporary.name, target_is_directory=False, dir_fd=lock.descriptor)
        else:  # pragma: no cover - native Windows CI
            try:
                os.symlink(str(source), temporary, target_is_directory=False)
            except OSError as error:
                raise PolicyManagerError(
                    "direct symlink creation failed; no copy or import-wrapper fallback was attempted: "
                    f"{destination}: {error}"
                ) from error
    else:
        payload = claude_wrapper_bytes(source, windows=True)
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
        if hasattr(os, "O_BINARY"):
            flags |= os.O_BINARY
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        if lock.descriptor is not None and os.open in getattr(os, "supports_dir_fd", set()):
            descriptor = os.open(temporary.name, flags, 0o600, dir_fd=lock.descriptor)
        else:  # pragma: no cover - native Windows CI
            descriptor = os.open(os.fspath(temporary), flags, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
    _fsync_directory(lock)
    if not _matches_desired(temporary, entry):
        raise PolicyManagerError(f"new policy artifact failed pre-install verification: {temporary}")


def _apply_preflight(
    plan: Dict[str, Any], locks: Dict[str, ParentLock]
) -> None:
    repository = Path(plan["repository_root"])
    require_source_snapshot(repository, plan["canonical_source"])
    for entry in plan["entries"]:
        lock = _lock_for(entry, locks)
        _require_parent_identity(lock)
        destination = Path(entry["destination"])
        if snapshot_path(destination) != entry["before"]:
            raise PolicyManagerError(f"target drifted after planning: {destination}")
        for artifact in (Path(entry["backup"]), Path(entry["temporary"])):
            if lexical_exists(artifact):
                raise PolicyManagerError(f"manager-owned transaction path is not empty: {artifact}")
        expected_action = "retain" if _matches_desired(destination, entry) else "replace"
        if expected_action != entry["action"]:
            raise PolicyManagerError(f"target classification changed after planning: {destination}")


def _rollback_preflight(
    plan: Dict[str, Any], locks: Dict[str, ParentLock]
) -> List[Dict[str, Any]]:
    states: List[Dict[str, Any]] = []
    for entry in plan["entries"]:
        lock = _lock_for(entry, locks)
        _require_parent_identity(lock)
        destination = Path(entry["destination"])
        backup = Path(entry["backup"])
        temporary = Path(entry["temporary"])
        current = snapshot_path(destination)
        backup_state = snapshot_path(backup)
        temp_state = snapshot_path(temporary)
        temp_desired = temp_state["kind"] != "missing" and _matches_desired(temporary, entry)
        temp_ok = temp_state["kind"] == "missing" or temp_desired
        if not temp_ok:
            raise PolicyManagerError(f"temporary policy artifact drift blocks all-action rollback: {temporary}")

        before = entry["before"]
        desired_now = _matches_desired(destination, entry)
        if entry["action"] == "retain":
            if current != before or backup_state["kind"] != "missing" or temp_state["kind"] != "missing":
                raise PolicyManagerError(f"retained target drift blocks all-action rollback: {destination}")
            states.append({"case": "retained", "temp": temp_state["kind"]})
            continue

        if before["kind"] == "missing":
            if backup_state["kind"] != "missing":
                raise PolicyManagerError(f"unexpected backup blocks all-action rollback: {backup}")
            if current["kind"] == "missing":
                case = "prepared-missing" if temp_desired else "not-mutated"
            elif desired_now:
                if temp_desired:
                    raise PolicyManagerError(
                        f"duplicate desired artifact blocks all-action rollback: {destination}"
                    )
                case = "installed-without-backup"
            else:
                raise PolicyManagerError(f"target drift blocks all-action rollback: {destination}")
        elif backup_state == before:
            if current["kind"] == "missing":
                case = "backup-with-prepared" if temp_desired else "backup-only"
            elif desired_now:
                if temp_desired:
                    raise PolicyManagerError(
                        f"duplicate desired artifact blocks all-action rollback: {destination}"
                    )
                case = "installed-with-backup"
            else:
                raise PolicyManagerError(f"target drift blocks all-action rollback: {destination}")
        elif backup_state["kind"] == "missing" and current == before:
            # A crash after the prior object was restored but before the
            # captured installed artifact was removed leaves exactly this
            # durable state.  It is safe to resume only when the adjacent
            # temporary still matches the reviewed desired artifact.
            case = "restored-with-captured" if temp_desired else "not-mutated"
        else:
            raise PolicyManagerError(f"backup drift blocks all-action rollback: {backup}")
        states.append({"case": case, "temp": temp_state["kind"]})
    return states


def _rollback_locked(
    transaction: Path,
    plan: Dict[str, Any],
    journal: Dict[str, Any],
    locks: Dict[str, ParentLock],
    transaction_lock: TransactionLock,
    *,
    failure_status: Optional[str] = None,
    fault_hook: Optional[FaultHook] = None,
) -> None:
    allow_detached = failure_status is not None
    try:
        states = _rollback_preflight(plan, locks)
    except PolicyManagerError:
        journal["status"] = "apply-failed-rollback-blocked" if failure_status else "rollback-blocked"
        _write_journal(
            transaction,
            journal,
            transaction_lock,
            allow_detached=allow_detached,
        )
        raise
    journal["status"] = "rolling-back"
    _write_journal(
        transaction,
        journal,
        transaction_lock,
        allow_detached=allow_detached,
    )
    try:
        for index in reversed(range(len(plan["entries"]))):
            entry = plan["entries"][index]
            state = states[index]
            lock = _lock_for(entry, locks)
            destination = Path(entry["destination"])
            backup = Path(entry["backup"])
            temporary = Path(entry["temporary"])
            if state["case"] in {"installed-without-backup", "installed-with-backup"}:
                _require_transaction_identity(
                    transaction_lock,
                    plan["transaction_identity"],
                    require_path=not allow_detached,
                )
                _call_fault(fault_hook, "before-rollback-capture", entry["id"])
                _require_transaction_identity(
                    transaction_lock,
                    plan["transaction_identity"],
                    require_path=not allow_detached,
                )
                if not _matches_desired(destination, entry) or lexical_exists(temporary):
                    raise PolicyManagerError(
                        f"installed target changed at rollback capture boundary: {destination}"
                    )
                _move_noreplace_in_parent(lock, destination.name, temporary.name)
                if not _matches_desired(temporary, entry) or snapshot_path(destination)["kind"] != "missing":
                    # A concurrent writer was captured rather than deleted.
                    # Put it back only through no-replace movement; otherwise
                    # preserve both names for inspection.
                    if snapshot_path(destination)["kind"] == "missing":
                        try:
                            _move_noreplace_in_parent(lock, temporary.name, destination.name)
                        except BaseException:
                            pass
                    raise PolicyManagerError(
                        f"target changed during rollback capture; no captured object was deleted: {destination}"
                    )
                _call_fault(fault_hook, "after-rollback-capture", entry["id"])
            if state["case"] in {
                "backup-only",
                "backup-with-prepared",
                "installed-with-backup",
            }:
                _require_transaction_identity(
                    transaction_lock,
                    plan["transaction_identity"],
                    require_path=not allow_detached,
                )
                _call_fault(fault_hook, "before-rollback-restore", entry["id"])
                _require_transaction_identity(
                    transaction_lock,
                    plan["transaction_identity"],
                    require_path=not allow_detached,
                )
                if snapshot_path(backup) != entry["before"] or snapshot_path(destination)["kind"] != "missing":
                    raise PolicyManagerError(
                        f"rollback restore boundary drifted; prior state remains preserved at {backup}"
                    )
                _move_noreplace_in_parent(lock, backup.name, destination.name)
                if snapshot_path(destination) != entry["before"]:
                    raise PolicyManagerError(f"exact rollback restore verification failed: {destination}")
                _call_fault(fault_hook, "after-rollback-restore", entry["id"])
            if snapshot_path(destination) != entry["before"]:
                raise PolicyManagerError(f"exact rollback verification failed: {destination}")
            if lexical_exists(backup):
                raise PolicyManagerError(f"rollback left the prior object in its backup name: {backup}")
            # Portable path-based unlink has no compare-and-delete primitive.
            # Retain a captured desired artifact under its unique reviewed
            # transaction name rather than risk deleting a writer replacement
            # in the final check/unlink race.  It is inert because the runtime
            # destination has already been restored exactly.
            _call_fault(fault_hook, "before-rollback-retain", entry["id"])
            if lexical_exists(temporary) and not _matches_desired(temporary, entry):
                raise PolicyManagerError(
                    f"captured policy artifact drifted and was preserved for inspection: {temporary}"
                )
            _call_fault(fault_hook, "after-rollback-retain", entry["id"])
            _set_entry_stage(
                transaction,
                journal,
                index,
                "rolled-back",
                changed=False,
                transaction_lock=transaction_lock,
                allow_detached=allow_detached,
            )
    except BaseException:
        journal["status"] = "apply-failed-rollback-blocked" if failure_status else "rollback-blocked"
        _write_journal(
            transaction,
            journal,
            transaction_lock,
            allow_detached=allow_detached,
        )
        raise
    journal["status"] = failure_status or "rolled-back"
    _write_journal(
        transaction,
        journal,
        transaction_lock,
        allow_detached=allow_detached,
    )


def apply_transaction(
    transaction_raw: Union[str, Path],
    plan_digest: str,
    *,
    fault_hook: Optional[FaultHook] = None,
) -> Dict[str, Any]:
    with lock_transaction(transaction_raw) as transaction_lock:
        return _apply_transaction_locked(
            transaction_lock,
            plan_digest,
            fault_hook=fault_hook,
        )


def _apply_transaction_locked(
    transaction_lock: TransactionLock,
    plan_digest: str,
    *,
    fault_hook: Optional[FaultHook] = None,
) -> Dict[str, Any]:
    transaction, plan, journal = _load_transaction(
        transaction_lock.path,
        plan_digest,
        transaction_lock=transaction_lock,
    )
    _require_transaction_identity(transaction_lock, plan["transaction_identity"])
    if journal["status"] != "planned":
        raise PolicyManagerError(
            f"apply requires a planned transaction; current status is {journal['status']}"
        )
    parent_specs = [
        (Path(entry["parent"]), entry["parent_identity"]) for entry in plan["entries"]
    ]
    with lock_parents(parent_specs) as locks:
        _apply_preflight(plan, locks)
        journal["status"] = "applying"
        _write_journal(transaction, journal, transaction_lock)
        try:
            source = Path(plan["canonical_source"]["path"])
            repository = Path(plan["repository_root"])
            for index, entry in enumerate(plan["entries"]):
                lock = _lock_for(entry, locks)
                destination = Path(entry["destination"])
                backup = Path(entry["backup"])
                if entry["action"] == "retain":
                    require_source_snapshot(repository, plan["canonical_source"])
                    if snapshot_path(destination) != entry["before"] or not _matches_desired(destination, entry):
                        raise PolicyManagerError(f"retained target drifted during apply: {destination}")
                    _set_entry_stage(
                        transaction,
                        journal,
                        index,
                        "retained",
                        changed=False,
                        transaction_lock=transaction_lock,
                    )
                    continue

                require_source_snapshot(repository, plan["canonical_source"])
                _require_parent_identity(lock)
                if snapshot_path(destination) != entry["before"]:
                    raise PolicyManagerError(f"target drifted before mutation: {destination}")
                _call_fault(fault_hook, "before-backup", entry["id"])
                _set_entry_stage(
                    transaction,
                    journal,
                    index,
                    "backing-up",
                    transaction_lock=transaction_lock,
                )
                if entry["before"]["kind"] != "missing":
                    _require_transaction_identity(transaction_lock, plan["transaction_identity"])
                    if snapshot_path(destination) != entry["before"] or lexical_exists(backup):
                        raise PolicyManagerError(
                            f"target or backup changed at the backup mutation boundary: {destination}"
                        )
                    _move_noreplace_in_parent(lock, destination.name, backup.name)
                    captured = snapshot_path(backup)
                    if captured != entry["before"] or snapshot_path(destination)["kind"] != "missing":
                        if snapshot_path(destination)["kind"] == "missing":
                            try:
                                _move_noreplace_in_parent(lock, backup.name, destination.name)
                            except BaseException:
                                pass
                        raise PolicyManagerError(
                            f"target changed at the atomic backup boundary; captured state was not deleted: {destination}"
                        )
                require_source_snapshot(repository, plan["canonical_source"])
                _call_fault(fault_hook, "after-backup", entry["id"])
                _set_entry_stage(
                    transaction,
                    journal,
                    index,
                    "backup-moved",
                    changed=True,
                    transaction_lock=transaction_lock,
                )
                _set_entry_stage(
                    transaction,
                    journal,
                    index,
                    "installing",
                    transaction_lock=transaction_lock,
                )
                _require_transaction_identity(transaction_lock, plan["transaction_identity"])
                _install_desired(lock, entry, source)
                require_source_snapshot(repository, plan["canonical_source"])
                _call_fault(fault_hook, "after-temp-create", entry["id"])
                _call_fault(fault_hook, "before-install-move", entry["id"])
                if not _matches_desired(Path(entry["temporary"]), entry):
                    raise PolicyManagerError(
                        f"prepared policy changed at install boundary: {entry['temporary']}"
                    )
                if snapshot_path(destination)["kind"] != "missing":
                    raise PolicyManagerError(
                        f"destination appeared at the install mutation boundary: {destination}"
                    )
                _require_transaction_identity(transaction_lock, plan["transaction_identity"])
                _move_noreplace_in_parent(lock, Path(entry["temporary"]).name, destination.name)
                if not _matches_desired(destination, entry):
                    raise PolicyManagerError(f"installed policy failed exact verification: {destination}")
                require_source_snapshot(repository, plan["canonical_source"])
                _call_fault(fault_hook, "after-install", entry["id"])
                _set_entry_stage(
                    transaction,
                    journal,
                    index,
                    "installed",
                    changed=True,
                    transaction_lock=transaction_lock,
                )
            _call_fault(fault_hook, "before-commit", "all")
            require_source_snapshot(repository, plan["canonical_source"])
            for entry in plan["entries"]:
                _require_parent_identity(_lock_for(entry, locks))
                if not _matches_desired(Path(entry["destination"]), entry):
                    raise PolicyManagerError(f"final installed state drifted: {entry['destination']}")
            _require_transaction_identity(transaction_lock, plan["transaction_identity"])
            journal["status"] = "applied"
            _write_journal(transaction, journal, transaction_lock)
        except BaseException as error:
            try:
                _rollback_locked(
                    transaction,
                    plan,
                    journal,
                    locks,
                    transaction_lock,
                    failure_status="apply-failed-rolled-back",
                )
            except BaseException as rollback_error:
                if isinstance(error, (KeyboardInterrupt, SystemExit)):
                    raise
                raise PolicyManagerError(
                    f"apply failed ({error}); rollback was blocked or failed ({rollback_error})"
                ) from error
            if isinstance(error, (KeyboardInterrupt, SystemExit)):
                raise
            if isinstance(error, PolicyManagerError):
                raise
            raise PolicyManagerError(f"apply failed and exact rollback succeeded: {error}") from error
    return journal


def verify_transaction(transaction_raw: Union[str, Path], plan_digest: str) -> Dict[str, Any]:
    with lock_transaction(transaction_raw) as transaction_lock:
        return _verify_transaction_locked(transaction_lock, plan_digest)


def _verify_transaction_locked(
    transaction_lock: TransactionLock,
    plan_digest: str,
) -> Dict[str, Any]:
    transaction, plan, journal = _load_transaction(
        transaction_lock.path,
        plan_digest,
        transaction_lock=transaction_lock,
    )
    _require_transaction_identity(transaction_lock, plan["transaction_identity"])
    if journal["status"] not in {"applied", "verified", "rolled-back", "apply-failed-rolled-back"}:
        raise PolicyManagerError(
            f"transaction status {journal['status']} is not a verifiable terminal filesystem state"
        )
    parent_specs = [
        (Path(entry["parent"]), entry["parent_identity"]) for entry in plan["entries"]
    ]
    with lock_parents(parent_specs) as locks:
        if journal["status"] in {"applied", "verified"}:
            # Apply is bound to the reviewed source bytes.  Later topology
            # verification deliberately is not: a direct link or import
            # wrapper exists so normal canonical edits reach new sessions
            # without reinstalling the runtime entry.  The current source must
            # still be the one real canonical file at the recorded path.
            current_source = source_snapshot(Path(plan["repository_root"]))
            for entry in plan["entries"]:
                _require_parent_identity(_lock_for(entry, locks))
                if not _matches_desired(Path(entry["destination"]), entry):
                    raise PolicyManagerError(f"installed policy verification failed: {entry['destination']}")
                if entry["action"] == "replace" and entry["before"]["kind"] != "missing":
                    if snapshot_path(Path(entry["backup"])) != entry["before"]:
                        raise PolicyManagerError(f"rollback backup verification failed: {entry['backup']}")
                elif lexical_exists(Path(entry["backup"])):
                    raise PolicyManagerError(f"unexpected rollback backup: {entry['backup']}")
                if lexical_exists(Path(entry["temporary"])):
                    raise PolicyManagerError(f"temporary artifact remains: {entry['temporary']}")
            journal["status"] = "verified"
            _write_journal(transaction, journal, transaction_lock)
            state = "installed"
            source_changed = current_source != plan["canonical_source"]
        else:
            for entry in plan["entries"]:
                _require_parent_identity(_lock_for(entry, locks))
                if snapshot_path(Path(entry["destination"])) != entry["before"]:
                    raise PolicyManagerError(f"rolled-back policy verification failed: {entry['destination']}")
                if lexical_exists(Path(entry["backup"])):
                    raise PolicyManagerError(f"rolled-back transaction left a prior-state backup: {entry['backup']}")
                temporary = Path(entry["temporary"])
                if lexical_exists(temporary) and not _matches_desired(temporary, entry):
                    raise PolicyManagerError(f"retained rollback artifact drifted: {temporary}")
            state = "rolled-back"
            source_changed = None
    return {
        "status": journal["status"],
        "filesystem_state": state,
        "runtime_activation": "not-observed",
        "canonical_source_changed_since_plan": source_changed,
        "targets": [entry["destination"] for entry in plan["entries"]],
        "retained_artifacts": [
            entry["temporary"]
            for entry in plan["entries"]
            if lexical_exists(Path(entry["temporary"]))
        ],
    }


def rollback_transaction(
    transaction_raw: Union[str, Path],
    plan_digest: str,
    *,
    fault_hook: Optional[FaultHook] = None,
) -> Dict[str, Any]:
    with lock_transaction(transaction_raw) as transaction_lock:
        return _rollback_transaction_locked(
            transaction_lock,
            plan_digest,
            fault_hook=fault_hook,
        )


def _rollback_transaction_locked(
    transaction_lock: TransactionLock,
    plan_digest: str,
    *,
    fault_hook: Optional[FaultHook] = None,
) -> Dict[str, Any]:
    transaction, plan, journal = _load_transaction(
        transaction_lock.path,
        plan_digest,
        transaction_lock=transaction_lock,
    )
    _require_transaction_identity(transaction_lock, plan["transaction_identity"])
    if journal["status"] == "rolled-back" or journal["status"] == "apply-failed-rolled-back":
        result = _verify_transaction_locked(transaction_lock, plan_digest)
        result["status"] = journal["status"]
        return result
    if journal["status"] not in {
        "planned",
        "applying",
        "rolling-back",
        "applied",
        "verified",
        "apply-failed-rollback-blocked",
        "rollback-blocked",
    }:
        raise PolicyManagerError(f"transaction cannot be rolled back from status {journal['status']}")
    parent_specs = [
        (Path(entry["parent"]), entry["parent_identity"]) for entry in plan["entries"]
    ]
    with lock_parents(parent_specs) as locks:
        _rollback_locked(
            transaction,
            plan,
            journal,
            locks,
            transaction_lock,
            fault_hook=fault_hook,
        )
    return {
        "status": "rolled-back",
        "filesystem_state": "rolled-back",
        "runtime_activation": "not-observed",
        "targets": [entry["destination"] for entry in plan["entries"]],
        "retained_artifacts": [
            entry["temporary"]
            for entry in plan["entries"]
            if lexical_exists(Path(entry["temporary"]))
        ],
    }


def _print_result(result: Dict[str, Any]) -> None:
    print(f"status: {result['status']}")
    print(f"filesystem_state: {result.get('filesystem_state', 'installed')}")
    print("runtime_activation: not-observed (restart and inspect the runtime instruction surface)")
    for target in result.get("targets", []):
        print(f"target: {target}")
    for artifact in result.get("retained_artifacts", []):
        print(f"retained_rollback_artifact: {artifact}")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    plan = commands.add_parser("plan", help="create a private immutable reviewed plan")
    plan.add_argument("--repo-root", required=True)
    plan.add_argument("--transaction-dir", required=True)
    plan.add_argument("--codex-target", action="append", default=[])
    plan.add_argument("--claude-target", action="append", default=[])
    plan.add_argument(
        "--claude-windows-import-wrapper-target",
        action="append",
        default=[],
        help="native Windows only; explicit alternative when Claude symlink capability is unavailable",
    )
    for name in ("apply", "verify", "rollback"):
        command = commands.add_parser(name)
        command.add_argument("--transaction-dir", required=True)
        command.add_argument("--plan-digest", required=True)
    return parser


def main(argv: Optional[List[str]] = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "plan":
            plan, digest = create_plan(
                args.repo_root,
                args.transaction_dir,
                codex_targets=args.codex_target,
                claude_targets=args.claude_target,
                claude_wrapper_targets=args.claude_windows_import_wrapper_target,
            )
            print(render_plan(plan, digest))
        elif args.command == "apply":
            journal = apply_transaction(args.transaction_dir, args.plan_digest)
            _print_result(
                {
                    "status": journal["status"],
                    "filesystem_state": "installed",
                    "targets": [],
                }
            )
        elif args.command == "verify":
            _print_result(verify_transaction(args.transaction_dir, args.plan_digest))
        else:
            _print_result(rollback_transaction(args.transaction_dir, args.plan_digest))
    except (OSError, PolicyManagerError) as error:
        print(f"global policy manager error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
