"""Component runtimes: how each component type is created, started, stopped,
inspected, and removed. Pure mechanics; the engine decides when."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from devcoordinator2.daemon import docker_cli, systemd_unit
from devcoordinator2.daemon.deploy_config import ComponentSpec

LOG_ROTATIONS = 1


class RuntimeError_(Exception):
    """Bounded runtime failure."""


# -- process components (fixed-name transient systemd units) ----------------

def process_unit_name(prefix: str, deployment_id: str, component: str,
                      generation: int) -> str:
    return f"{prefix}-{deployment_id}-{component}-g{generation}.service"


def deployment_slice(prefix: str, deployment_id: str) -> str:
    return f"{prefix}-{deployment_id}.slice"


def rotate_log(path: Path) -> None:
    """Keep one previous log so restarts never grow a file without bound."""
    if path.exists():
        previous = path.with_suffix(path.suffix + ".1")
        try:
            os.replace(path, previous)
        except OSError:
            pass


def start_process(*, unit: str, slice_name: str, uid: int, gid: int,
                  cwd: Path, env_file: Path, command: tuple[str, ...],
                  log_path: Path) -> None:
    rotate_log(log_path)
    argv = [
        "systemd-run", "--quiet", f"--unit={unit}", f"--slice={slice_name}",
        f"--uid={uid}", f"--gid={gid}",
        "--property=KillMode=control-group",
        "--property=TimeoutStopSec=15s",
        "--property=Restart=on-failure", "--property=RestartSec=2s",
        "--property=StartLimitIntervalSec=120", "--property=StartLimitBurst=5",
        "--property=NoNewPrivileges=yes", "--property=UMask=0027",
        f"--property=EnvironmentFile={env_file}",
        f"--property=StandardOutput=append:{log_path}",
        f"--property=StandardError=append:{log_path}",
        f"--working-directory={cwd}",
    ]
    sup = systemd_unit.supplementary_groups(uid)
    if sup:
        names = " ".join(systemd_unit._group_name(g) for g in sup)
        argv.append(f"--property=SupplementaryGroups={names}")
    argv += ["--", *command]
    proc = subprocess.run(argv, capture_output=True, text=True, timeout=60,
                          check=False)
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[:1024] or "systemd-run failed")


def process_state(unit: str) -> dict:
    props = systemd_unit.show_unit(
        unit, ["ActiveState", "SubState", "Result", "MainPID", "NRestarts",
               "ExecMainStatus", "ControlGroup"])
    active = props.get("ActiveState", "")
    if active in ("", "inactive") and not props.get("ControlGroup"):
        state = "stopped"
    elif active == "active":
        state = "running"
    elif active in ("activating", "reloading"):
        state = "starting"
    elif active == "deactivating":
        state = "stopping"
    else:
        state = "failed"
    return {
        "state": state, "active_state": active or "absent",
        "sub_state": props.get("SubState", ""), "result": props.get("Result", ""),
        "main_pid": int(props.get("MainPID") or 0),
        "restarts": int(props.get("NRestarts") or 0),
        "cgroup": props.get("ControlGroup", ""),
    }


def stop_process(unit: str) -> None:
    cgroup = systemd_unit.control_group_path(unit)
    try:
        systemd_unit.stop_unit(unit)
    except systemd_unit.SystemdError as exc:
        raise RuntimeError_(str(exc)) from exc
    if not systemd_unit.prove_cgroup_empty(cgroup):
        raise RuntimeError_(f"cgroup of {unit} still has processes after stop")
    systemd_unit.reset_failed(unit)


# -- docker containers (plain and PostgreSQL) -------------------------------

def container_name(deployment_id: str, component: str) -> str:
    return f"devcoordinator2-deploy-{deployment_id}-{component}"


def volume_name(deployment_id: str, component: str, declared: str) -> str:
    return f"devcoordinator2-{deployment_id}-{component}-{declared}"


def create_container(*, name: str, image: str, labels: dict[str, str],
                     env_file: Path | None, publish: list[str],
                     volumes: list[str], command: tuple[str, ...],
                     restart: str = "on-failure:5") -> str:
    argv = ["create", "--name", name, f"--restart={restart}"]
    for key, value in labels.items():
        argv += ["--label", f"{key}={value}"]
    if env_file is not None:
        argv += ["--env-file", str(env_file)]
    for spec in publish:
        argv += ["--publish", spec]
    for spec in volumes:
        argv += ["--volume", spec]
    argv.append(image)
    argv += list(command)
    proc = docker_cli._run(argv)
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[:1024] or "docker create failed")
    container_id = proc.stdout.strip()
    if len(container_id) != 64:
        raise RuntimeError_("unexpected docker create output")
    return container_id


def ensure_image(image: str) -> None:
    if docker_cli._run(["image", "inspect", image], timeout=30).returncode == 0:
        return
    proc = docker_cli._run(["pull", image], timeout=600)
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[:1024] or f"cannot pull {image}")


def start_container(container_id: str) -> None:
    proc = docker_cli._run(["start", container_id], timeout=60)
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[:1024] or "docker start failed")


def restart_container(container_id: str) -> None:
    proc = docker_cli._run(["restart", "--time", "15", container_id], timeout=90)
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[:1024] or "docker restart failed")


def stop_container(container_id: str) -> None:
    proc = docker_cli._run(["stop", "--time", "15", container_id], timeout=60)
    if proc.returncode != 0 and "No such container" not in proc.stderr:
        raise RuntimeError_(proc.stderr.strip()[:1024] or "docker stop failed")


def container_state(container_id: str) -> dict:
    try:
        info = docker_cli.inspect(container_id)
    except docker_cli.DockerError:
        return {"state": "missing", "restarts": 0}
    st = info.get("State") or {}
    status = st.get("Status", "unknown")
    state = {"running": "running", "created": "stopped", "exited": "stopped",
             "paused": "stopped", "restarting": "starting",
             "dead": "failed"}.get(status, "failed")
    if status == "exited" and st.get("ExitCode", 0) not in (0, None):
        state = "failed"
    return {"state": state, "status": status, "restarts": info.get("RestartCount", 0),
            "exit_code": st.get("ExitCode"), "started_at": st.get("StartedAt")}


def remove_container(container_id: str, delete_volumes: bool) -> None:
    argv = ["rm", "--force"]
    if delete_volumes:
        argv.append("--volumes")
    proc = docker_cli._run([*argv, container_id], timeout=60)
    if proc.returncode != 0 and "No such container" not in proc.stderr:
        raise RuntimeError_(proc.stderr.strip()[:512] or "docker rm failed")


def remove_volume(name: str) -> None:
    proc = docker_cli._run(["volume", "rm", name], timeout=60)
    if proc.returncode != 0 and "no such volume" not in proc.stderr.lower():
        raise RuntimeError_(proc.stderr.strip()[:512] or "docker volume rm failed")


def container_logs(container_id: str, tail_lines: int) -> str:
    proc = docker_cli._run(["logs", "--tail", str(tail_lines), container_id],
                           timeout=30)
    return (proc.stdout + proc.stderr)[-65536:]


# -- compose projects --------------------------------------------------------

def compose_project(deployment_id: str, component: str) -> str:
    return f"dc2-{deployment_id}-{component}"


def _compose(project: str, file: Path, cwd: Path, env_file: Path | None,
             args: list[str], timeout: int = 600) -> subprocess.CompletedProcess:
    argv = ["docker", "compose", "--project-name", project, "--file", str(file)]
    if env_file is not None:
        argv += ["--env-file", str(env_file)]
    argv += args
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout,
                              check=False, cwd=cwd, env={"PATH": "/usr/bin:/bin"})
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise RuntimeError_(f"docker compose failed: {exc}") from exc


def compose_up(project: str, file: Path, cwd: Path, env_file: Path | None,
               services: tuple[str, ...]) -> None:
    proc = _compose(project, file, cwd, env_file, ["up", "--detach", "--remove-orphans",
                                                   *services])
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[-1024:] or "compose up failed")


def compose_stop(project: str, file: Path, cwd: Path, env_file: Path | None) -> None:
    proc = _compose(project, file, cwd, env_file, ["stop"])
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[-1024:] or "compose stop failed")


def compose_start(project: str, file: Path, cwd: Path, env_file: Path | None) -> None:
    proc = _compose(project, file, cwd, env_file, ["start"])
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[-1024:] or "compose start failed")


def compose_down(project: str, file: Path, cwd: Path, env_file: Path | None,
                 delete_volumes: bool) -> None:
    args = ["down", "--remove-orphans"]
    if delete_volumes:
        args.append("--volumes")
    proc = _compose(project, file, cwd, env_file, args)
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[-1024:] or "compose down failed")


def compose_container_ids(project: str) -> list[str]:
    return docker_cli.list_ids_by_labels({"com.docker.compose.project": project})


def compose_state(project: str) -> dict:
    ids = compose_container_ids(project)
    if not ids:
        return {"state": "stopped", "containers": 0, "running": 0}
    running = sum(1 for cid in ids if container_state(cid)["state"] == "running")
    if running == len(ids):
        state = "running"
    elif running == 0:
        state = "stopped"
    else:
        state = "failed"
    return {"state": state, "containers": len(ids), "running": running}


def compose_logs(project: str, file: Path, cwd: Path, tail_lines: int) -> str:
    proc = _compose(project, file, cwd, None, ["logs", "--no-color", "--tail",
                                               str(tail_lines)], timeout=60)
    return (proc.stdout + proc.stderr)[-65536:]


def describe(spec: ComponentSpec) -> str:
    return f"{spec.name} ({spec.type})"
