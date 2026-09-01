"""Content-free fingerprints and receipts for governed checks."""

from __future__ import annotations

import hashlib
import json
import os
import pwd
import stat
import subprocess
import tempfile
from pathlib import Path

MAX_GIT_PATH_BYTES = 16 * 1024 * 1024
MAX_JSON_BYTES = 2 * 1024 * 1024


class EvidenceError(Exception):
    pass


def _git_output(worktree_root: Path, args: list[str],
                run_as: tuple[int, int] | None = None) -> bytes:
    prefix: list[str] = []
    env = {"PATH": "/usr/bin:/bin", "HOME": os.environ.get("HOME", "/root")}
    if run_as is not None and os.geteuid() == 0 and run_as[0] != 0:
        uid, gid = run_as
        try:
            env["HOME"] = pwd.getpwuid(uid).pw_dir
        except KeyError as exc:
            raise EvidenceError(f"unknown caller uid {uid}") from exc
        prefix = ["setpriv", f"--reuid={uid}", f"--regid={gid}",
                  "--init-groups", "--"]
    else:
        env.update({
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "safe.directory",
            "GIT_CONFIG_VALUE_0": "*",
        })
    try:
        proc = subprocess.run(
            [*prefix, "git", "-C", str(worktree_root), *args],
            capture_output=True, timeout=30, check=False, env=env,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise EvidenceError(f"cannot inventory repository source: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", errors="replace").strip()[:512]
        raise EvidenceError(detail or "cannot inventory repository source")
    if len(proc.stdout) > MAX_GIT_PATH_BYTES:
        raise EvidenceError("repository source inventory exceeds 16 MiB")
    return proc.stdout


def _decode_paths(payload: bytes) -> list[str]:
    try:
        paths = [raw.decode("utf-8") for raw in payload.split(b"\0") if raw]
    except UnicodeDecodeError as exc:
        raise EvidenceError("repository contains a non-UTF-8 source path") from exc
    if len(paths) != len(set(paths)):
        raise EvidenceError("repository source inventory contains duplicate paths")
    return [path for path in paths
            if path != ".devcoordinator" and not path.startswith(".devcoordinator/")]


def _safe_relative(value: str) -> Path:
    path = Path(value)
    if not value or path.is_absolute() or "\\" in value or "\0" in value \
            or any(part in ("", ".", "..") for part in path.parts) \
            or path.as_posix() != value:
        raise EvidenceError("evidence path is not normalized repository-relative")
    return path


def _hash_path(root_fd: int, relative: str, digest, *, missing_ok: bool = False) -> None:
    path = _safe_relative(relative)
    digest.update(relative.encode("utf-8") + b"\0")
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=root_fd)
    except OSError as exc:
        try:
            target = os.readlink(path, dir_fd=root_fd)
        except OSError:
            if missing_ok and isinstance(exc, FileNotFoundError):
                digest.update(b"missing\0")
                return
            raise EvidenceError(f"cannot read source path {relative!r}: {exc}") from exc
        digest.update(b"symlink\0" + target.encode("utf-8", errors="surrogateescape") + b"\0")
        return
    try:
        before = os.fstat(fd)
        digest.update(f"{stat.S_IMODE(before.st_mode):o}\0".encode())
        if stat.S_ISDIR(before.st_mode):
            # Git submodules appear as directories in the parent worktree.
            digest.update(b"gitlink\0")
            return
        if not stat.S_ISREG(before.st_mode):
            raise EvidenceError(f"source path {relative!r} is not a regular file")
        while True:
            block = os.read(fd, 1024 * 1024)
            if not block:
                break
            digest.update(block)
        after = os.fstat(fd)
        if (before.st_size, before.st_mtime_ns, before.st_ino) != \
                (after.st_size, after.st_mtime_ns, after.st_ino):
            raise EvidenceError(f"source path {relative!r} changed while hashing")
    finally:
        os.close(fd)


def source_digest(worktree_root: Path,
                  run_as: tuple[int, int] | None = None) -> str:
    """Hash the index plus only unstaged/untracked content.

    Clean and staged files use Git's existing blob identities, so a large
    repository does not need to be reread for every governed run.
    """
    root = worktree_root.resolve()
    digest = hashlib.sha256(b"devcoordinator2-source-v1\0")
    index = _git_output(root, ["ls-files", "--stage", "-z"], run_as)
    digest.update(b"index\0" + index + b"\0")
    modified = _decode_paths(_git_output(
        root, ["diff-files", "--name-only", "--no-ext-diff", "-z"], run_as))
    untracked = _decode_paths(_git_output(
        root, ["ls-files", "--others", "--exclude-standard", "-z"], run_as))
    root_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for relative in sorted(set(modified) | set(untracked)):
            _hash_path(root_fd, relative, digest, missing_ok=True)
    finally:
        os.close(root_fd)
    return digest.hexdigest()


def artifact_receipts(worktree_root: Path,
                      paths: list[str] | tuple[str, ...]) -> list[dict]:
    receipts = []
    root = worktree_root.resolve()
    root_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for value in paths:
            path = _safe_relative(value)
            try:
                fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=root_fd)
            except OSError as exc:
                raise EvidenceError(f"declared artifact {value!r} is unavailable: {exc}") \
                    from exc
            try:
                details = os.fstat(fd)
                if not stat.S_ISREG(details.st_mode):
                    raise EvidenceError(
                        f"declared artifact {value!r} is not a regular file")
                digest = hashlib.sha256()
                while True:
                    block = os.read(fd, 1024 * 1024)
                    if not block:
                        break
                    digest.update(block)
                after = os.fstat(fd)
                if (details.st_size, details.st_mtime_ns, details.st_ino) != \
                        (after.st_size, after.st_mtime_ns, after.st_ino):
                    raise EvidenceError(
                        f"declared artifact {value!r} changed while hashing")
                receipts.append({
                    "path": value,
                    "size": details.st_size,
                    "sha256": digest.hexdigest(),
                })
            finally:
                os.close(fd)
    finally:
        os.close(root_fd)
    return receipts


def receipts_match(worktree_root: Path, receipts: list[dict]) -> bool:
    try:
        expected_paths = [row["path"] for row in receipts]
        current = artifact_receipts(worktree_root, expected_paths)
    except (EvidenceError, KeyError, TypeError):
        return False
    return current == receipts


def write_json_atomic(path: Path, document: dict, mode: int = 0o600) -> None:
    payload = (json.dumps(document, separators=(",", ":"), sort_keys=True)
               + "\n").encode()
    if len(payload) > MAX_JSON_BYTES:
        raise EvidenceError("governed-check report exceeds 2 MiB")
    fd, tmp_name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}-")
    try:
        written = 0
        while written < len(payload):
            written += os.write(fd, payload[written:])
        os.fsync(fd)
        os.fchmod(fd, mode)
    finally:
        os.close(fd)
    try:
        os.replace(tmp_name, path)
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise
    dir_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)


def read_json_bounded(path: Path) -> dict | None:
    try:
        details = path.stat()
        if not stat.S_ISREG(details.st_mode) or details.st_size > MAX_JSON_BYTES:
            return None
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None
