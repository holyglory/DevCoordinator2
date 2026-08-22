"""systemd transient unit control: launch, inspect, stop, cgroup-empty proof.

All invocations are argv arrays. The daemon (root) delegates credential
changes to PID 1 via systemd-run --uid/--gid; it never manipulates
credentials itself.
"""

from __future__ import annotations

import grp
import os
import pwd
import subprocess
import time
from pathlib import Path

_SYSTEMCTL_TIMEOUT = 30
CGROUP_ROOT = Path("/sys/fs/cgroup")
STOP_GRACE_SECONDS = "10s"


class SystemdError(Exception):
    """systemctl/systemd-run failure with a bounded diagnostic."""


def _run(argv: list[str], timeout: int = _SYSTEMCTL_TIMEOUT) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(argv, capture_output=True, text=True,
                              timeout=timeout, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise SystemdError(f"{argv[0]} invocation failed: {exc}") from exc


def supplementary_groups(uid: int) -> list[int]:
    """Explicit supplementary gids for the caller, never including root."""
    entry = pwd.getpwuid(uid)
    gids = os.getgrouplist(entry.pw_name, entry.pw_gid)
    return sorted({g for g in gids if g != 0})


def build_systemd_run_argv(*, unit: str, slice_name: str, uid: int, gid: int,
                           timeout_seconds: int, cwd: Path,
                           env: dict[str, str], command: tuple[str, ...],
                           scratch_dir: Path) -> list[str]:
    entry = pwd.getpwuid(uid)
    argv = [
        "systemd-run", "--quiet", "--pipe",
        f"--unit={unit}",
        f"--slice={slice_name}",
        f"--uid={uid}", f"--gid={gid}",
        "--property=KillMode=control-group",
        f"--property=TimeoutStopSec={STOP_GRACE_SECONDS}",
        f"--property=RuntimeMaxSec={timeout_seconds}s",
        "--property=NoNewPrivileges=yes",
        "--property=UMask=0077",
        f"--working-directory={cwd}",
        f"--setenv=HOME={entry.pw_dir}",
        f"--setenv=USER={entry.pw_name}",
        f"--setenv=LOGNAME={entry.pw_name}",
        f"--setenv=TMPDIR={scratch_dir}",
    ]
    sup = supplementary_groups(uid)
    if sup:
        names = " ".join(_group_name(g) for g in sup)  # space-separated list
        argv.append(f"--property=SupplementaryGroups={names}")
    for key, value in env.items():
        argv.append(f"--setenv={key}={value}")
    argv.append("--")
    argv.extend(command)
    return argv


def _group_name(gid: int) -> str:
    try:
        return grp.getgrgid(gid).gr_name
    except KeyError:
        return str(gid)


def spawn(argv: list[str]) -> subprocess.Popen:
    """Launch systemd-run --pipe holding the unit's output pipes."""
    return subprocess.Popen(
        argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def show_unit(unit: str, properties: list[str]) -> dict[str, str]:
    """Return requested properties; missing/unloaded units yield empty values."""
    proc = _run(["systemctl", "show", unit,
                 "-p", ",".join(properties), "--no-pager"])
    values: dict[str, str] = {}
    if proc.returncode != 0:
        return values
    for line in proc.stdout.splitlines():
        key, _, value = line.partition("=")
        if key in properties:
            values[key] = value
    return values


def unit_is_loaded(unit: str) -> bool:
    props = show_unit(unit, ["LoadState"])
    return props.get("LoadState", "not-found") not in ("", "not-found")


def unit_is_active_or_activating(unit: str) -> bool:
    props = show_unit(unit, ["ActiveState"])
    return props.get("ActiveState") in ("active", "activating", "deactivating")


def list_matching_units(pattern: str) -> list[str]:
    proc = _run(["systemctl", "list-units", "--all", "--plain", "--no-legend",
                 "--no-pager", pattern])
    units = []
    for line in proc.stdout.splitlines():
        fields = line.split()
        if fields:
            units.append(fields[0])
    return units


def stop_unit(unit: str) -> None:
    proc = _run(["systemctl", "stop", unit], timeout=60)
    if proc.returncode != 0 and unit_is_loaded(unit):
        raise SystemdError(
            f"systemctl stop {unit} failed: {proc.stderr.strip()[:512]}")


def reset_failed(unit: str) -> None:
    _run(["systemctl", "reset-failed", unit])


def control_group_path(unit: str) -> Path | None:
    props = show_unit(unit, ["ControlGroup"])
    cg = props.get("ControlGroup", "")
    if not cg:
        return None
    return CGROUP_ROOT / cg.lstrip("/")


def prove_cgroup_empty(cgroup: Path | None, deadline_seconds: float = 15.0) -> bool:
    """True only when the cgroup provably has no processes (or is gone)."""
    if cgroup is None:
        return True
    procs_file = cgroup / "cgroup.procs"
    deadline = time.monotonic() + deadline_seconds
    while time.monotonic() < deadline:
        try:
            content = procs_file.read_text()
        except FileNotFoundError:
            return True
        except OSError:
            return False
        if not content.strip():
            return True
        time.sleep(0.2)
    return False


def process_uids(pid: int) -> tuple[int, int, int, int] | None:
    """All four UIDs of a process (real, effective, saved, fs), or None."""
    try:
        text = Path(f"/proc/{pid}/status").read_text()
    except OSError:
        return None
    for line in text.splitlines():
        if line.startswith("Uid:"):
            parts = line.split()
            if len(parts) == 5:
                return tuple(int(p) for p in parts[1:5])  # type: ignore[return-value]
    return None
