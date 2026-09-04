"""Raw measurement sources: /proc, cgroup v2, filesystems, Docker sizes.
Content-free by construction; every reader tolerates missing files."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

from devcoordinator2.daemon import docker_cli

CGROUP_ROOT = Path("/sys/fs/cgroup")
NCPU = os.cpu_count() or 1


def host_cpu_ticks() -> tuple[int, int]:
    """(busy, total) jiffies from /proc/stat."""
    try:
        fields = Path("/proc/stat").read_text().splitlines()[0].split()[1:]
        values = [int(v) for v in fields]
    except (OSError, ValueError, IndexError):
        return 0, 0
    idle = values[3] + (values[4] if len(values) > 4 else 0)
    total = sum(values)
    return total - idle, total


def host_memory() -> dict[str, int]:
    out = {"total": 0, "available": 0, "swap_total": 0, "swap_free": 0}
    keys = {"MemTotal": "total", "MemAvailable": "available",
            "SwapTotal": "swap_total", "SwapFree": "swap_free"}
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            key, _, rest = line.partition(":")
            if key in keys:
                out[keys[key]] = int(rest.split()[0]) * 1024
    except (OSError, ValueError):
        pass
    out["used"] = max(out["total"] - out["available"], 0)
    return out


def host_load() -> tuple[float, float, float]:
    try:
        a, b, c = Path("/proc/loadavg").read_text().split()[:3]
        return float(a), float(b), float(c)
    except (OSError, ValueError):
        return 0.0, 0.0, 0.0


def filesystem(path: Path) -> dict[str, int]:
    try:
        stat = os.statvfs(path)
    except OSError:
        return {"size": 0, "free": 0, "used": 0}
    size = stat.f_frsize * stat.f_blocks
    free = stat.f_frsize * stat.f_bavail
    return {"size": size, "free": free, "used": size - stat.f_frsize * stat.f_bfree}


def cgroup_stats(cgroup: Path | None) -> dict[str, int] | None:
    """cpu_usec, memory_current, memory_peak, pids, io_rbytes, io_wbytes."""
    if cgroup is None or not cgroup.is_dir():
        return None
    out = {"cpu_usec": 0, "memory_current": 0, "memory_peak": 0, "pids": 0,
           "io_rbytes": 0, "io_wbytes": 0}
    try:
        for line in (cgroup / "cpu.stat").read_text().splitlines():
            if line.startswith("usage_usec "):
                out["cpu_usec"] = int(line.split()[1])
    except (OSError, ValueError):
        pass
    for name, key in (("memory.current", "memory_current"), ("memory.peak", "memory_peak"),
                      ("pids.current", "pids")):
        try:
            out[key] = int((cgroup / name).read_text().strip())
        except (OSError, ValueError):
            pass
    try:
        for line in (cgroup / "io.stat").read_text().splitlines():
            for field in line.split()[1:]:
                k, _, v = field.partition("=")
                if k == "rbytes":
                    out["io_rbytes"] += int(v)
                elif k == "wbytes":
                    out["io_wbytes"] += int(v)
    except (OSError, ValueError):
        pass
    return out


def own_cgroup() -> Path | None:
    try:
        for line in Path("/proc/self/cgroup").read_text().splitlines():
            if line.startswith("0::"):
                return CGROUP_ROOT / line[3:].lstrip("/")
    except OSError:
        pass
    return None


def container_cgroup(container_id: str) -> Path:
    return CGROUP_ROOT / "system.slice" / f"docker-{container_id}.scope"


def running_containers() -> list[dict]:
    """id, names, labels (parsed), created, state for every container."""
    proc = docker_cli._run(["ps", "--all", "--no-trunc", "--format", "{{json .}}"],
                           timeout=30)
    rows = []
    if proc.returncode != 0:
        return rows
    for line in proc.stdout.splitlines():
        try:
            c = json.loads(line)
        except json.JSONDecodeError:
            continue
        labels = {}
        for item in c.get("Labels", "").split(","):
            if "=" in item:
                k, _, v = item.partition("=")
                labels[k] = v
        rows.append({"id": c.get("ID"), "name": c.get("Names"), "state": c.get("State"),
                     "image": c.get("Image"), "labels": labels,
                     "created": c.get("CreatedAt"), "status": c.get("Status")})
    return rows


def container_sizes() -> dict[str, int]:
    """Writable-layer bytes per full container ID (docker ps --size)."""
    proc = docker_cli._run(["ps", "--all", "--no-trunc", "--size", "--format",
                            "{{.ID}} {{.Size}}"], timeout=120)
    sizes = {}
    for line in proc.stdout.splitlines():
        parts = line.split()
        if len(parts) >= 2:
            sizes[parts[0]] = _parse_size(parts[1])
    return sizes


def docker_shared_sizes() -> dict[str, int]:
    """Images, build cache, and per-volume sizes (shared/unattributed unless a
    volume is a recorded deployment volume)."""
    proc = docker_cli._run(["system", "df", "-v", "--format", "{{json .}}"], timeout=120)
    out = {"images": 0, "build_cache": 0, "volumes": {}}
    if proc.returncode != 0:
        return out
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return out
    for image in data.get("Images") or []:
        out["images"] += _parse_size(str(image.get("UniqueSize", "0B")))
    for entry in data.get("BuildCache") or []:
        out["build_cache"] += _parse_size(str(entry.get("Size", "0B")))
    for vol in data.get("Volumes") or []:
        out["volumes"][vol.get("Name", "")] = _parse_size(str(vol.get("Size", "0B")))
    return out


def directory_size(path: Path, timeout: int = 120) -> int | None:
    """Bytes under path on one filesystem, bounded by a timeout."""
    try:
        proc = subprocess.run(["du", "-sbx", str(path)], capture_output=True, text=True,
                              timeout=timeout, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return None
    try:
        return int(proc.stdout.split()[0])
    except (ValueError, IndexError):
        return None


_UNITS = {"B": 1, "kB": 10**3, "KB": 10**3, "MB": 10**6, "GB": 10**9, "TB": 10**12,
          "KiB": 2**10, "MiB": 2**20, "GiB": 2**30, "TiB": 2**40}


def _parse_size(text: str) -> int:
    text = text.strip().split(" (")[0]
    for unit in sorted(_UNITS, key=len, reverse=True):
        if text.endswith(unit):
            try:
                return int(float(text[:-len(unit)]) * _UNITS[unit])
            except ValueError:
                return 0
    try:
        return int(float(text))
    except ValueError:
        return 0
