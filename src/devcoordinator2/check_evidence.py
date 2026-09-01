"""Rust-backed fingerprints plus atomic control-plane JSON helpers."""

from __future__ import annotations

import json
import os
import pwd
import stat
import subprocess
import tempfile
from pathlib import Path

MAX_JSON_BYTES = 2 * 1024 * 1024
MAX_EXECUTOR_REPLY_BYTES = 64 * 1024
EXECUTOR_BINARY = (Path(__file__).resolve().parents[2] / "target" / "release"
                   / "devcoordinator2-executor")


class EvidenceError(Exception):
    pass


def _executor_call(args: list[str], *, payload: bytes | None = None,
                   run_as: tuple[int, int] | None = None,
                   mismatch_exit: int | None = None) -> dict:
    """Invoke one bounded, non-executing Rust evidence operation."""
    if not EXECUTOR_BINARY.is_file() or not os.access(EXECUTOR_BINARY, os.X_OK):
        raise EvidenceError(
            "Rust governed-test executor is unavailable; build the locked release target")
    prefix: list[str] = []
    env = {"PATH": "/usr/bin:/bin", "HOME": "/root"}
    if run_as is not None and os.geteuid() == 0 and run_as[0] != 0:
        uid, gid = run_as
        try:
            env["HOME"] = pwd.getpwuid(uid).pw_dir
        except KeyError as exc:
            raise EvidenceError(f"unknown caller uid {uid}") from exc
        prefix = ["/usr/bin/setpriv", f"--reuid={uid}", f"--regid={gid}",
                  "--init-groups", "--"]
    try:
        proc = subprocess.run(
            [*prefix, str(EXECUTOR_BINARY), *args],
            input=payload if payload is not None else b"",
            capture_output=True, timeout=60, check=False, env=env,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise EvidenceError(f"cannot invoke Rust evidence engine: {exc}") from exc
    if len(proc.stdout) > MAX_EXECUTOR_REPLY_BYTES \
            or len(proc.stderr) > MAX_EXECUTOR_REPLY_BYTES:
        raise EvidenceError("Rust evidence engine returned an oversized response")
    if proc.returncode != 0 and proc.returncode != mismatch_exit:
        detail = proc.stderr.decode("utf-8", errors="replace").strip()[:512]
        raise EvidenceError(detail or "Rust evidence engine failed")
    try:
        document = json.loads(proc.stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise EvidenceError("Rust evidence engine returned invalid JSON") from exc
    if not isinstance(document, dict) or document.get("schema") != 2:
        raise EvidenceError("Rust evidence engine returned the wrong schema")
    document["_exit_code"] = proc.returncode
    return document


def source_digest(worktree_root: Path,
                  run_as: tuple[int, int] | None = None) -> str:
    """Return the Rust engine's schema-2 source fingerprint."""
    document = _executor_call(
        ["source-digest", "--worktree", str(worktree_root.resolve())],
        run_as=run_as)
    digest = document.get("sha256")
    if not isinstance(digest, str) or len(digest) != 64 \
            or any(character not in "0123456789abcdef" for character in digest):
        raise EvidenceError("Rust evidence engine returned an invalid source digest")
    return digest


def receipts_match(worktree_root: Path, receipts: list[dict]) -> bool:
    try:
        payload = json.dumps(receipts, separators=(",", ":"),
                             sort_keys=True).encode()
    except (TypeError, ValueError) as exc:
        raise EvidenceError("artifact receipts are not valid JSON") from exc
    if len(payload) > MAX_JSON_BYTES:
        raise EvidenceError("artifact receipts exceed 2 MiB")
    document = _executor_call(
        ["receipts-match", "--worktree", str(worktree_root.resolve()),
         "--receipts", "-"], payload=payload, mismatch_exit=1)
    matches = document.get("matches")
    if not isinstance(matches, bool) or document["_exit_code"] != (0 if matches else 1):
        raise EvidenceError("Rust evidence engine returned an invalid receipt result")
    return matches


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
