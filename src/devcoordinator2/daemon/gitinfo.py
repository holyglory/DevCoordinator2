"""Canonical Git worktree and repository resolution.

Repository identity is the realpath of the Git common root (the directory
owning the shared .git), so every worktree of one repository maps to one
repository_id while keeping its own worktree_id.

The root daemon never runs git itself over caller-owned repositories: git
would refuse with "dubious ownership" (safe.directory), and root parsing a
repository's config is an unnecessary privilege exposure. Resolution runs as
the physical caller via a setpriv argv prefix. Only when dropping is
impossible (non-root daemon, or a root caller on admin commands) does git
run directly, with safe.directory disabled for that invocation.
"""

from __future__ import annotations

import os
import pwd
import subprocess
from dataclasses import dataclass
from pathlib import Path

_GIT_TIMEOUT = 10
_EXEC_PATH = "/usr/bin:/bin"


class GitResolveError(Exception):
    """Path is not inside a usable Git worktree."""


@dataclass(frozen=True)
class WorktreeInfo:
    repository_root: Path  # realpath of the common root (owns the shared .git)
    worktree_root: Path  # realpath of this worktree's top level


def _invocation(run_as: tuple[int, int] | None) -> tuple[list[str], dict[str, str]]:
    env = {"PATH": _EXEC_PATH}
    if run_as is not None and os.geteuid() == 0 and run_as[0] != 0:
        uid, gid = run_as
        try:
            entry = pwd.getpwuid(uid)
        except KeyError as exc:
            raise GitResolveError(f"unknown caller uid {uid}") from exc
        env["HOME"] = entry.pw_dir
        prefix = ["setpriv", f"--reuid={uid}", f"--regid={gid}",
                  "--init-groups", "--"]
        return prefix, env
    env["HOME"] = os.environ.get("HOME", "/root")
    env["GIT_CONFIG_COUNT"] = "1"
    env["GIT_CONFIG_KEY_0"] = "safe.directory"
    env["GIT_CONFIG_VALUE_0"] = "*"
    return [], env


def resolve_worktree(path: Path,
                     run_as: tuple[int, int] | None = None) -> WorktreeInfo:
    probe = path if path.is_dir() else path.parent
    if not probe.is_dir():
        raise GitResolveError(f"not a directory: {path}")
    prefix, env = _invocation(run_as)
    argv = [
        *prefix,
        "git", "-C", str(probe), "rev-parse", "--path-format=absolute",
        "--show-toplevel", "--git-common-dir",
    ]
    try:
        proc = subprocess.run(
            argv, capture_output=True, text=True, timeout=_GIT_TIMEOUT,
            check=False, env=env,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise GitResolveError(f"git invocation failed: {exc}") from exc
    if proc.returncode != 0:
        raise GitResolveError(proc.stderr.strip()[:512] or "not a git repository")
    lines = proc.stdout.splitlines()
    if len(lines) != 2 or not lines[0]:
        raise GitResolveError("bare or unusable repository (no worktree)")
    worktree_root = Path(lines[0]).resolve()
    common_dir = Path(lines[1]).resolve()
    if common_dir.name != ".git":
        raise GitResolveError(f"unsupported repository layout: {common_dir}")
    return WorktreeInfo(
        repository_root=common_dir.parent, worktree_root=worktree_root
    )
