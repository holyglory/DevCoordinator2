"""Component runtimes: how each component type is created, started, stopped,
inspected, and removed. Pure mechanics; the engine decides when."""

from __future__ import annotations

import os
import subprocess
import time
from pathlib import Path

from devcoordinator2.daemon import docker_cli, systemd_unit
from devcoordinator2.daemon.deploy_config import ComponentSpec

LOG_ROTATIONS = 1


class RuntimeError_(Exception):
    """Bounded runtime failure."""


def _bounded_process_error(proc: subprocess.CompletedProcess,
                           fallback: str) -> str:
    detail = (proc.stdout + "\n" + proc.stderr).strip()
    return detail[-4096:] if detail else fallback


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
    health = (st.get("Health") or {}).get("Status")
    if status == "running" and health == "starting":
        state = "starting"
    elif status == "running" and health == "unhealthy":
        state = "failed"
    labels = (info.get("Config") or {}).get("Labels") or {}
    return {"state": state, "status": status, "restarts": info.get("RestartCount", 0),
            "exit_code": st.get("ExitCode"), "health": health,
            "started_at": st.get("StartedAt"), "finished_at": st.get("FinishedAt"),
            "image_id": info.get("Image"),
            "compose_service": labels.get("com.docker.compose.service")}


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


def _compose(project: str, files: tuple[Path, ...], cwd: Path,
             env_files: tuple[Path, ...],
             args: list[str], timeout: int = 600) -> subprocess.CompletedProcess:
    argv = ["docker", "compose", "--project-name", project]
    for file in files:
        argv += ["--file", str(file)]
    for env_file in env_files:
        argv += ["--env-file", str(env_file)]
    argv += args
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout,
                              check=False, cwd=cwd, env={"PATH": "/usr/bin:/bin"})
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise RuntimeError_(f"docker compose failed: {exc}") from exc


def compose_config_services(project: str, files: tuple[Path, ...], cwd: Path,
                            env_files: tuple[Path, ...]) -> tuple[str, ...]:
    proc = _compose(project, files, cwd, env_files, ["config", "--services"], timeout=120)
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[-1024:] or
                            "compose configuration could not list services")
    return tuple(line.strip() for line in proc.stdout.splitlines() if line.strip())


def compose_up(project: str, files: tuple[Path, ...], cwd: Path,
               env_files: tuple[Path, ...], services: tuple[str, ...],
               finite_services: tuple[str, ...], build: bool) -> None:
    actual = set(compose_config_services(project, files, cwd, env_files))
    missing = sorted(set(services) - actual)
    if missing:
        raise RuntimeError_(f"compose services not found: {missing}")
    if finite_services:
        remove = _compose(project, files, cwd, env_files,
                          ["rm", "--stop", "--force", *finite_services], timeout=120)
        if remove.returncode != 0:
            raise RuntimeError_(remove.stderr.strip()[-1024:] or
                                "compose finite-service reset failed")
    args = ["up", "--detach", "--remove-orphans"]
    if build:
        args.append("--build")
    args += list(services)
    proc = _compose(project, files, cwd, env_files, args, timeout=1800)
    if proc.returncode != 0:
        raise RuntimeError_(_bounded_process_error(proc, "compose up failed"))


def compose_stop(project: str, files: tuple[Path, ...], cwd: Path,
                 env_files: tuple[Path, ...]) -> None:
    proc = _compose(project, files, cwd, env_files, ["stop"])
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[-1024:] or "compose stop failed")


def compose_start(project: str, files: tuple[Path, ...], cwd: Path,
                  env_files: tuple[Path, ...], services: tuple[str, ...]) -> None:
    proc = _compose(project, files, cwd, env_files, ["start", *services])
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[-1024:] or "compose start failed")


def compose_start_exact_services(project: str, services: tuple[str, ...]) -> None:
    """Start only existing long-running service containers by exact ID.

    ``docker compose start <service>`` also follows dependencies and can rerun
    a successfully completed finite prerequisite. Exact daemon-owned bindings
    preserve the completed receipt and make ordinary start non-mutating.
    """
    found = compose_service_container_ids(project, services)
    for service in services:
        for container_id in found[service]:
            start_container(container_id)


def compose_stop_exact_services(project: str, services: tuple[str, ...]) -> None:
    found = compose_service_container_ids(project, services)
    for service in services:
        for container_id in found[service]:
            stop_container(container_id)


def compose_service_container_ids(project: str,
                                  services: tuple[str, ...]) -> dict[str, list[str]]:
    wanted = set(services)
    found: dict[str, list[str]] = {service: [] for service in services}
    for container_id in compose_container_ids(project):
        service = container_state(container_id).get("compose_service")
        if service in wanted:
            found[service].append(container_id)
    missing = sorted(service for service, ids in found.items() if not ids)
    if missing:
        raise RuntimeError_(f"compose service containers not found: {missing}")
    return found


def compose_service_ready(project: str, service: str,
                          timeout_seconds: int) -> tuple[bool, str]:
    deadline = time.monotonic() + timeout_seconds
    last = "service container missing"
    while time.monotonic() < deadline:
        try:
            found = compose_service_container_ids(project, (service,))[service]
        except RuntimeError_ as exc:
            return False, str(exc)
        states = [container_state(container_id) for container_id in found]
        if all(item["state"] == "running" for item in states):
            return True, f"{service} running"
        if any(item["state"] in ("failed", "stopped", "missing") for item in states):
            last = ", ".join(item.get("status", item["state"]) for item in states)
            return False, f"{service} became terminal: {last}"
        last = ", ".join(item["state"] for item in states)
        time.sleep(0.5)
    return False, f"{service} did not become ready: {last}"


def compose_down(project: str, files: tuple[Path, ...], cwd: Path,
                 env_files: tuple[Path, ...],
                 delete_volumes: bool) -> None:
    args = ["down", "--remove-orphans"]
    if delete_volumes:
        args.append("--volumes")
    proc = _compose(project, files, cwd, env_files, args)
    if proc.returncode != 0:
        raise RuntimeError_(proc.stderr.strip()[-1024:] or "compose down failed")


def compose_container_ids(project: str) -> list[str]:
    return docker_cli.list_ids_by_labels({"com.docker.compose.project": project})


def compose_publishes_host_port(project: str, host_port: int) -> tuple[bool, str]:
    """Prove that the exact managed Compose project owns a host-port binding."""
    for container_id in compose_container_ids(project):
        try:
            info = docker_cli.inspect(container_id)
        except docker_cli.DockerError:
            return False, f"allocated host port {host_port} could not be verified"
        published = (info.get("NetworkSettings") or {}).get("Ports") or {}
        for bindings in published.values():
            for binding in bindings or ():
                try:
                    bound_port = int(binding.get("HostPort", ""))
                except (AttributeError, TypeError, ValueError):
                    continue
                if bound_port == host_port:
                    return True, f"allocated host port {host_port} is published"
    return (False,
            f"allocated host port {host_port} is not published by the Compose project")


def compose_state(project: str, services: tuple[str, ...] = (),
                  finite_services: tuple[str, ...] = (),
                  completions: dict[str, dict] | None = None,
                  desired_states: dict[str, str] | None = None) -> dict:
    ids = compose_container_ids(project)
    if not ids and not completions:
        return {"state": "stopped", "containers": 0, "running": 0}
    by_service: dict[str, list[dict]] = {}
    for cid in ids:
        item = {"container_id": cid, **container_state(cid)}
        name = item.get("compose_service")
        if name:
            by_service.setdefault(name, []).append(item)
    expected = tuple(services) if services else tuple(sorted(by_service))
    finite = set(finite_services)
    completion_rows = completions or {}
    desired = desired_states or {}
    details = []
    candidates = []
    for name in expected:
        items = by_service.get(name, [])
        if name in finite:
            failed = [item for item in items if item["state"] == "failed"]
            completed = [item for item in items
                         if item["status"] == "exited" and item.get("exit_code") == 0]
            if failed:
                service_state = "failed"
            elif completed and len(completed) == len(items):
                service_state = "completed"
                candidates.extend({"service": name, **item} for item in completed)
            elif any(item["state"] in ("running", "starting") for item in items):
                service_state = "starting"
            elif name in completion_rows:
                service_state = "completed"
            else:
                service_state = "missing"
        elif not items:
            service_state = "missing"
        elif desired.get(name) == "stopped" and all(
                item["state"] in ("failed", "stopped") for item in items):
            service_state = "stopped"
        elif all(item["state"] == "running" for item in items):
            service_state = "running"
        elif any(item["state"] == "failed" for item in items):
            service_state = "failed"
        elif any(item["state"] == "starting" for item in items):
            service_state = "starting"
        elif all(item["state"] == "stopped" for item in items):
            service_state = "failed"
        else:
            service_state = "failed"
        details.append({"name": name, "role": "finite" if name in finite else "running",
                        "state": service_state, "desired_state": desired.get(name, "running"),
                        "containers": len(items)})
    finite_states = [item["state"] for item in details if item["role"] == "finite"]
    running_states = [item["state"] for item in details if item["role"] == "running"]
    if any(value in ("failed", "missing") for value in finite_states + running_states):
        state = "failed"
    elif any(value == "starting" for value in finite_states + running_states):
        state = "starting"
    elif running_states and all(value == "running" for value in running_states) \
            and all(value == "completed" for value in finite_states):
        state = "running"
    elif running_states and all(value == "stopped" for value in running_states) \
            and all(value == "completed" for value in finite_states):
        state = "stopped"
    else:
        state = "failed"
    running = sum(1 for item in by_service.values()
                  for container in item if container["state"] == "running")
    return {"state": state, "containers": len(ids), "running": running,
            "services": details, "completion_candidates": candidates}


def compose_ready(project: str, services: tuple[str, ...],
                  finite_services: tuple[str, ...], completions: dict[str, dict],
                  timeout_seconds: int) -> tuple[bool, str, dict]:
    deadline = time.monotonic() + timeout_seconds
    state = compose_state(project, services, finite_services, completions)
    while state["state"] == "starting" and time.monotonic() < deadline:
        time.sleep(0.5)
        state = compose_state(project, services, finite_services, completions)
    detail = ", ".join(f"{item['name']}={item['state']}" for item in state.get("services", []))
    return state["state"] == "running", detail or state["state"], state


def compose_logs(project: str, files: tuple[Path, ...], cwd: Path,
                 env_files: tuple[Path, ...], tail_lines: int) -> str:
    proc = _compose(project, files, cwd, env_files,
                    ["logs", "--no-color", "--tail", str(tail_lines)], timeout=60)
    return (proc.stdout + proc.stderr)[-65536:]


def describe(spec: ComponentSpec) -> str:
    return f"{spec.name} ({spec.type})"
