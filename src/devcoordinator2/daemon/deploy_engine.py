"""Deployment apply / rollback (HANDOVER §9). Control actions live in
deploy_control.py; both share DeploymentContext."""

from __future__ import annotations

import logging
import os
import pwd
import stat
import subprocess
import threading
import time
from dataclasses import dataclass
from pathlib import Path

from devcoordinator2.daemon import deploy_runtime as rt
from devcoordinator2.daemon import deploy_state as st
from devcoordinator2.daemon import docker_cli, health_checks, ports, routes
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.deploy_config import ComponentSpec, DeploymentSpec
from devcoordinator2.daemon.gitinfo import GitResolveError
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

log = logging.getLogger("devcoordinator2.deploy")
BUILD_TIMEOUT = 1800


@dataclass
class Ctx:
    """Everything one deployment operation needs."""
    config: InstanceConfig
    db: Database
    dep_id: str
    spec: DeploymentSpec
    source: str
    caller_uid: int
    caller_gid: int
    client: str
    session: str | None

    @property
    def dir(self) -> Path:
        return self.config.deployments_dir / self.dep_id

    def env_path(self, component: str, generation: int) -> Path:
        return self.dir / "env" / f"{component}-g{generation}.env"

    def log_path(self, component: str) -> Path:
        return self.dir / "logs" / f"{component}.log"

    def compose_files(self, comp: ComponentSpec, path: Path) -> tuple[Path, ...]:
        return tuple(path / file for file in comp.compose_files)

    def compose_env_files(self, comp: ComponentSpec, path: Path,
                          generation: int) -> tuple[Path, ...]:
        files = []
        if comp.compose_env_file:
            row = st.get_deployment(self.db, self.dep_id)
            repository_id = row.get("repository_id") if row else None
            if not repository_id or not self.config.compose_env_authorized(
                    repository_id, comp.compose_env_file):
                raise ProtocolError(
                    "repository_config_invalid",
                    f"Compose env_file {comp.compose_env_file!r} is not authorized"
                    " by private instance configuration")
            candidate = path / comp.compose_env_file
            try:
                info = candidate.lstat()
                resolved_root = path.resolve(strict=True)
                resolved = candidate.resolve(strict=True)
            except OSError as exc:
                raise ProtocolError(
                    "repository_config_invalid",
                    f"Compose env_file {comp.compose_env_file!r} is unavailable") from exc
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode) \
                    or (resolved != resolved_root and resolved_root not in resolved.parents):
                raise ProtocolError(
                    "repository_config_invalid",
                    f"Compose env_file {comp.compose_env_file!r} is unsafe")
            ignored = _as_caller(
                self, ["git", "check-ignore", "--quiet", "--", comp.compose_env_file],
                path, 30)
            if ignored.returncode != 0:
                raise ProtocolError(
                    "repository_config_invalid",
                    f"Compose env_file {comp.compose_env_file!r} must remain ignored")
            files.append(resolved)
        generated = self.env_path(comp.name, generation)
        if generated.exists():
            files.append(generated)
        return tuple(files)

    def labels(self, component: str, generation: int, data_class: str) -> dict[str, str]:
        row = st.get_deployment(self.db, self.dep_id) or {}
        purpose = "preview" if self.spec.ttl_seconds else "permanent"
        labels = docker_cli.managed_labels(
            instance=self.config.unit_prefix, repository_id=row.get("repository_id", ""),
            worktree_id=row.get("worktree_id", ""), run_id="", purpose=purpose,
            caller_uid=self.caller_uid, client=self.client, session=self.session,
            created_at=st.now_iso(), data_class=data_class)
        labels.pop(f"{docker_cli.LABEL_PREFIX}.run")
        labels[f"{docker_cli.LABEL_PREFIX}.deployment"] = self.dep_id
        labels[f"{docker_cli.LABEL_PREFIX}.component"] = component
        labels[f"{docker_cli.LABEL_PREFIX}.generation"] = str(generation)
        if self.spec.ttl_seconds:
            labels[f"{docker_cli.LABEL_PREFIX}.ttl_seconds"] = str(self.spec.ttl_seconds)
        return labels

    def ensure_dirs(self) -> None:
        for sub in ("", "env", "logs"):
            path = self.dir / sub if sub else self.dir
            path.mkdir(parents=True, exist_ok=True, mode=0o755)
            if sub == "logs":
                os.chown(path, self.caller_uid, self.caller_gid)
        self.dir.chmod(0o755)


def runtime_generation(ctx: Ctx, row: dict, component: dict) -> int:
    """Resolve the environment generation for a live or failed binding."""

    for value in (row.get("current_generation"), component.get("generation")):
        if isinstance(value, int) and not isinstance(value, bool) and value > 0:
            return value
    env_dir = ctx.dir / "env"
    prefix = f"{component['name']}-g"
    candidates = []
    if env_dir.is_dir():
        for path in env_dir.glob(f"{prefix}*.env"):
            suffix = path.name.removeprefix(prefix).removesuffix(".env")
            if suffix.isdigit():
                candidates.append(int(suffix))
    return max(candidates, default=0)


class Busy:
    def __init__(self):
        self._locks: dict[str, threading.Lock] = {}
        self._guard = threading.Lock()

    def acquire(self, dep_id: str) -> threading.Lock:
        with self._guard:
            lock = self._locks.setdefault(dep_id, threading.Lock())
        if not lock.acquire(blocking=False):
            raise ProtocolError("busy", f"deployment {dep_id} has a mutation in progress")
        return lock


# -- environment assembly ----------------------------------------------------

def component_env(ctx: Ctx, comp: ComponentSpec, generation: int,
                  port_map: dict[str, int]) -> dict[str, str]:
    env = dict(comp.env)
    env["DC2_DEPLOYMENT"] = ctx.dep_id
    env["DC2_COMPONENT"] = comp.name
    env["DC2_GENERATION"] = str(generation)
    if comp.wants_port and comp.name in port_map:
        env["PORT"] = str(port_map[comp.name])
    for name, port in port_map.items():
        env[f"DC2_PORT_{name.upper().replace('-', '_')}"] = str(port)
    urls: dict[str, str] = {}
    for other in ctx.spec.components:
        if other.type != "postgres":
            continue
        url = postgres_url(ctx, other, port_map)
        if url:
            urls[other.name] = url
            env[f"DC2_POSTGRES_{other.name.upper().replace('-', '_')}_URL"] = url
    if len(urls) == 1:
        env.setdefault("DATABASE_URL", next(iter(urls.values())))
    return env


def postgres_url(ctx: Ctx, comp: ComponentSpec, port_map: dict[str, int]) -> str | None:
    if comp.shared_from:
        other_dep, other_comp = comp.shared_from.split("/", 1)
        creds = st.read_postgres_credentials(ctx.config, other_dep, other_comp)
        port = ports.assigned(ctx.db, other_dep, 0).get(other_comp)
        if not creds or port is None:
            return None
    else:
        creds = st.postgres_credentials(ctx.config, ctx.dep_id, comp.name,
                                        comp.user or "app", comp.database or "app")
        port = port_map.get(comp.name)
        if port is None:
            return None
    return (f"postgresql://{creds['user']}:{creds['password']}@127.0.0.1:{port}"
            f"/{creds['database']}")


# -- candidate preparation ---------------------------------------------------

def _as_caller(ctx: Ctx, argv: list[str], cwd: Path, timeout: int,
               log_path: Path | None = None) -> subprocess.CompletedProcess:
    prefix = ["setpriv", f"--reuid={ctx.caller_uid}", f"--regid={ctx.caller_gid}",
              "--init-groups", "--"] if os.geteuid() == 0 else []
    env = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": _home(ctx.caller_uid)}
    if log_path is not None:
        with open(log_path, "ab") as fh:
            os.fchown(fh.fileno(), ctx.caller_uid, ctx.caller_gid)
            return subprocess.run([*prefix, *argv], cwd=cwd, env=env, stdout=fh,
                                  stderr=subprocess.STDOUT, timeout=timeout, check=False)
    return subprocess.run([*prefix, *argv], cwd=cwd, env=env, capture_output=True,
                          text=True, timeout=timeout, check=False)


def _home(uid: int) -> str:
    try:
        return pwd.getpwuid(uid).pw_dir
    except KeyError:
        return "/nonexistent"


def head_commit(ctx: Ctx, worktree: Path) -> tuple[str | None, bool]:
    rev = _as_caller(ctx, ["git", "rev-parse", "HEAD"], worktree, 30)
    commit = rev.stdout.strip() if rev.returncode == 0 else None
    status = _as_caller(ctx, ["git", "status", "--porcelain"], worktree, 30)
    dirty = bool(status.stdout.strip()) if status.returncode == 0 else True
    return commit, dirty


def prepare_generation_path(ctx: Ctx, worktree: Path, number: int,
                            commit: str | None) -> Path:
    if ctx.source == "worktree":
        return worktree
    if commit is None:
        raise ProtocolError("deployment_apply_failed",
                            "checkout source needs a committed HEAD")
    target = ctx.dir / f"gen-{number}"
    ctx.dir.mkdir(parents=True, exist_ok=True)
    os.chown(ctx.dir, ctx.caller_uid, ctx.caller_gid)
    proc = _as_caller(ctx, ["git", "worktree", "add", "--detach", str(target), commit],
                      worktree, 300)
    if proc.returncode != 0:
        raise ProtocolError("deployment_apply_failed",
                            "git worktree add failed", proc.stderr[-2048:])
    return target


def remove_generation_path(ctx: Ctx, worktree: Path, path: Path) -> None:
    if ctx.source != "checkout" or not path.is_dir() or ctx.dir not in path.parents:
        return
    _as_caller(ctx, ["git", "worktree", "remove", "--force", str(path)], worktree, 120)
    if path.is_dir():
        subprocess.run(["rm", "-rf", "--one-file-system", str(path)], check=False)


def run_build(ctx: Ctx, path: Path) -> None:
    if not ctx.spec.build:
        return
    ctx.ensure_dirs()
    proc = _as_caller(ctx, list(ctx.spec.build), path, BUILD_TIMEOUT,
                      log_path=ctx.log_path("build"))
    if proc.returncode != 0:
        raise ProtocolError("deployment_apply_failed",
                            f"build exited {proc.returncode}; see {ctx.log_path('build')}")


# -- component start/health --------------------------------------------------

def start_component(ctx: Ctx, comp: ComponentSpec, generation: int, path: Path,
                    port_map: dict[str, int]) -> tuple[str, str]:
    """Create/start one component; returns (binding_kind, binding_identity)."""
    ctx.ensure_dirs()
    env = component_env(ctx, comp, generation, port_map)
    env_path = ctx.env_path(comp.name, generation)
    owner = (ctx.caller_uid, ctx.caller_gid) if comp.type == "process" else (0, 0)
    st.write_env_file(env_path, env, owner,
                      fmt="systemd" if comp.type == "process" else "docker")
    try:
        if comp.type == "process":
            unit = rt.process_unit_name(ctx.config.deploy_unit_prefix, ctx.dep_id,
                                        comp.name, generation)
            rt.start_process(
                unit=unit, slice_name=rt.deployment_slice(ctx.config.deploy_unit_prefix,
                                                          ctx.dep_id),
                uid=ctx.caller_uid, gid=ctx.caller_gid, cwd=(path / comp.cwd).resolve(),
                env_file=env_path, command=comp.command, log_path=ctx.log_path(comp.name))
            return "unit", unit
        if comp.type in ("docker", "postgres"):
            return "container", _start_container_component(ctx, comp, generation, port_map,
                                                            env_path)
        if comp.type == "compose":
            project = rt.compose_project(ctx.dep_id, comp.name)
            rt.compose_up(project, ctx.compose_files(comp, path), path,
                          ctx.compose_env_files(comp, path, generation), comp.services,
                          comp.finite_services, comp.compose_build)
            return "compose", project
    except rt.RuntimeError_ as exc:
        raise ProtocolError("deployment_apply_failed",
                            f"component {comp.name} failed to start: {exc}") from exc
    return "none", ""


def _start_container_component(ctx: Ctx, comp: ComponentSpec, generation: int,
                                port_map: dict[str, int], env_path: Path) -> str:
    name = rt.container_name(ctx.dep_id, comp.name)
    if st.is_generation_scoped(comp):
        name = f"{name}-g{generation}"
    existing = docker_cli.list_ids_by_labels({
        f"{docker_cli.LABEL_PREFIX}.deployment": ctx.dep_id,
        f"{docker_cli.LABEL_PREFIX}.component": comp.name})
    if not st.is_generation_scoped(comp):
        for cid in existing:  # stable component being (re)created
            rt.remove_container(cid, delete_volumes=False)
    publish, volumes, command, env_file = [], [], comp.command, env_path
    data_class = "persistent" if comp.owns_persistent_data else "disposable"
    if comp.type == "postgres":
        creds = st.postgres_credentials(ctx.config, ctx.dep_id, comp.name,
                                        comp.user or "app", comp.database or "app")
        pg_env = ctx.env_path(comp.name, generation).with_suffix(".pg.env")
        st.write_env_file(pg_env, {"POSTGRES_USER": creds["user"],
                                   "POSTGRES_PASSWORD": creds["password"],
                                   "POSTGRES_DB": creds["database"]}, (0, 0),
                          fmt="docker")
        env_file = pg_env
        publish = [f"127.0.0.1:{port_map[comp.name]}:5432"]
        pgdata = rt.volume_name(ctx.dep_id, comp.name, "pgdata")
        volumes = [f"{pgdata}:/var/lib/postgresql/data"]
        command = ()
    else:
        if comp.container_port is not None and comp.name in port_map:
            publish = [f"127.0.0.1:{port_map[comp.name]}:{comp.container_port}"]
        volumes = [f"{rt.volume_name(ctx.dep_id, comp.name, v.split(':', 1)[0])}:"
                   f"{v.split(':', 1)[1]}" for v in comp.volumes]
    rt.ensure_image(comp.image or "")
    cid = rt.create_container(name=name, image=comp.image or "",
                              labels=ctx.labels(comp.name, generation, data_class),
                              env_file=env_file, publish=publish, volumes=volumes,
                              command=command)
    rt.start_container(cid)
    return cid


def prove_health(ctx: Ctx, comp: ComponentSpec, binding: tuple[str, str],
                 port_map: dict[str, int], generation: int) -> tuple[bool, str]:
    kind, identity = binding
    if comp.type == "external":
        host, port = comp.tcp.rsplit(":", 1)
        return health_checks.tcp_ready(host, int(port), 10)
    if comp.type == "postgres":
        if comp.shared_from:
            other_dep, other_comp = comp.shared_from.split("/", 1)
            rows = ctx.db.query("SELECT binding_identity FROM components WHERE"
                                " deployment_id=? AND name=?", (other_dep, other_comp))
            if not rows or not rows[0]["binding_identity"]:
                return False, "shared postgres not deployed"
            identity = rows[0]["binding_identity"]
            creds = st.read_postgres_credentials(ctx.config, other_dep, other_comp) or {}
        else:
            creds = st.read_postgres_credentials(ctx.config, ctx.dep_id, comp.name) or {}
        return health_checks.postgres_ready(identity, creds.get("user", "app"),
                                            creds.get("database", "app"), 120)
    timeout = comp.health.timeout_seconds if comp.health else 30
    terminal = _terminal_binding_check(kind, identity)
    if comp.health and comp.health.kind == "http" and comp.name in port_map:
        return health_checks.http_ready(port_map[comp.name], comp.health.path, timeout,
                                        terminal)
    if comp.health and comp.health.kind == "tcp" and comp.name in port_map:
        return health_checks.tcp_ready("127.0.0.1", port_map[comp.name], timeout,
                                       terminal)
    if kind == "unit":
        time.sleep(1.0)  # let a crash-at-start surface
        state = rt.process_state(identity)
        return state["state"] == "running", f"unit {state['active_state']}"
    if kind == "container":
        return health_checks.container_running(identity)
    if kind == "compose":
        receipts = st.compose_completions(ctx.db, ctx.dep_id, comp.name, generation)
        ok, note, state = rt.compose_ready(
            identity, comp.services, comp.finite_services, receipts,
            comp.compose_timeout_seconds)
        st.record_compose_completions(ctx.db, ctx.dep_id, comp.name, generation,
                                      state.get("completion_candidates", []))
        return ok, note
    return True, "no check"


def _terminal_binding_check(kind: str, identity: str):
    """Return a readiness abort probe after a short launch grace period.

    Systemd's auto-restart state is ``starting`` and remains eligible to
    recover. ``failed`` or an inactive/stopped long-running unit is terminal.
    Containers have equivalent terminal states. Compose has its own service
    role-aware readiness loop.
    """
    grace_until = time.monotonic() + 1.0

    def check() -> str | None:
        if time.monotonic() < grace_until:
            return None
        if kind == "unit":
            state = rt.process_state(identity)
            if state["state"] in ("failed", "stopped"):
                return (f"unit became terminal: {state['active_state']}"
                        f"/{state['sub_state']} ({state['result'] or 'no result'})")
        elif kind == "container":
            state = rt.container_state(identity)
            if state["state"] in ("failed", "stopped", "missing"):
                return f"container became terminal: {state.get('status', state['state'])}"
        return None

    return check


def stop_component_binding(kind: str, identity: str, spec: ComponentSpec | None,
                           path: Path | None, env_file: Path | None) -> None:
    if kind == "unit":
        rt.stop_process(identity)
    elif kind == "container":
        rt.stop_container(identity)
    elif kind == "compose" and spec is not None and path is not None:
        env_files = tuple(filter(None, (
            path / spec.compose_env_file if spec.compose_env_file else None,
            env_file,
        )))
        rt.compose_stop(identity, tuple(path / file for file in spec.compose_files),
                        path, env_files)


def resolve_registration(registry: Registry, path: Path, caller: Caller):
    try:
        return registry.register(path, caller_uid=caller.uid, caller_gid=caller.gid)
    except GitResolveError as exc:
        raise ProtocolError("repository_not_found", str(exc)) from exc


def publish_routes(ctx: Ctx) -> None:
    routes.publish(ctx.db, ctx.config.routes_path, ctx.config.base_domain)
