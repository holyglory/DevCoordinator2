"""Command handlers: args validation and dispatch to registry / lifecycle."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from devcoordinator2 import __version__
from devcoordinator2.daemon.db import SCHEMA_VERSION
from devcoordinator2.daemon.gitinfo import GitResolveError
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller, Handler
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError


def _require_path(args: dict[str, Any], allowed: set[str]) -> Path:
    unknown = set(args) - allowed
    if unknown:
        raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
    raw = args.get("path")
    if not isinstance(raw, str) or not raw:
        raise ProtocolError("args_invalid", "'path' (string) is required")
    path = Path(raw)
    if not path.is_absolute():
        raise ProtocolError("args_invalid", "'path' must be absolute")
    return path


def _no_args(args: dict[str, Any]) -> None:
    if args:
        raise ProtocolError("args_invalid", f"unexpected args: {sorted(args)}")


def _optional_str(args: dict[str, Any], key: str) -> str | None:
    value = args.get(key)
    if value is not None and not isinstance(value, str):
        raise ProtocolError("args_invalid", f"'{key}' must be a string")
    return value


def build_handlers(config: InstanceConfig, registry: Registry,
                   lifecycle=None, deployments=None, db=None) -> dict[str, Handler]:
    def ping(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _no_args(args)
        return {"daemon_version": __version__, "schema_version": SCHEMA_VERSION,
                "socket": str(config.socket_path)}

    def repository_register(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        path = _require_path(args, {"path"})
        try:
            reg = registry.register(path, caller_uid=caller.uid,
                                    caller_gid=caller.gid)
        except GitResolveError as exc:
            raise ProtocolError("repository_not_found", str(exc)) from exc
        return {
            "repository_id": reg.repository_id,
            "worktree_id": reg.worktree_id,
            "root_path": reg.root_path,
            "worktree_path": reg.worktree_path,
            "display_name": reg.display_name,
            "registered": reg.newly_registered,
        }

    def repository_list(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _no_args(args)
        return {"repositories": registry.list_repositories()}

    def repository_status(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        path = _require_path(args, {"path"})
        status = registry.repository_status(path, run_as=(caller.uid, caller.gid))
        if status is None:
            raise ProtocolError("repository_not_found",
                                f"no registered repository contains {path}")
        if lifecycle is not None:
            summary_ref = lifecycle.current_summary_ref(path, caller)
            if summary_ref is not None:
                status["current_test"] = summary_ref
        return status

    handlers: dict[str, Handler] = {
        "ping": ping,
        "repository.register": repository_register,
        "repository.list": repository_list,
        "repository.status": repository_status,
    }

    if lifecycle is not None:
        def test_start(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            unknown = set(args) - {"path", "test"}
            if unknown:
                raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
            path = _require_path({"path": args.get("path")}, {"path"})
            test = args.get("test")
            if test is not None and not isinstance(test, str):
                raise ProtocolError("args_invalid", "'test' must be a string")
            return lifecycle.start(path, test, caller)

        def test_status(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(args, {"path"})
            return lifecycle.status(path, caller)

        def test_output(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(args, {"path", "stream", "tail_bytes"})
            stream = args.get("stream")
            if stream not in ("stdout", "stderr"):
                raise ProtocolError("args_invalid",
                                    "'stream' must be 'stdout' or 'stderr'")
            tail_bytes = args.get("tail_bytes", 16384)
            if not isinstance(tail_bytes, int) or not (1 <= tail_bytes <= 65536):
                raise ProtocolError("args_invalid",
                                    "'tail_bytes' must be an integer in 1..65536")
            return lifecycle.output(path, stream, tail_bytes, caller)

        def test_stop(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(args, {"path"})
            return lifecycle.stop(path, caller)

        handlers.update({
            "test.start": test_start,
            "test.status": test_status,
            "test.output": test_output,
            "test.stop": test_stop,
        })

    if deployments is not None:
        handlers.update(_deployment_handlers(config, deployments, db))
    return handlers


def _deployment_handlers(config: InstanceConfig, deployments, db) -> dict[str, Handler]:
    ref_keys = {"path", "name", "deployment_id"}

    def ref(args: dict[str, Any], extra: set[str] = frozenset()):
        path = _require_path(args, ref_keys | extra)
        return path, _optional_str(args, "name"), _optional_str(args, "deployment_id")

    def dep_list(args, caller):
        unknown = set(args) - {"path"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        path = Path(args["path"]) if args.get("path") else None
        if path is not None and not path.is_absolute():
            raise ProtocolError("args_invalid", "'path' must be absolute")
        return deployments.list(path, caller)

    def dep_apply(args, caller):
        return deployments.apply(*ref(args), caller)

    def dep_status(args, caller):
        return deployments.status(*ref(args), caller)

    def dep_rollback(args, caller):
        return deployments.rollback(*ref(args), caller)

    def make_control(action):
        def handler(args, caller):
            path, name, dep_id = ref(args, {"component"})
            return deployments.control(action, path, name, dep_id,
                                       _optional_str(args, "component"), caller)
        return handler

    def dep_logs(args, caller):
        path, name, dep_id = ref(args, {"component", "tail_lines"})
        component = _optional_str(args, "component")
        if not component:
            raise ProtocolError("args_invalid", "'component' is required")
        tail = args.get("tail_lines", 200)
        if not isinstance(tail, int) or not (1 <= tail <= 5000):
            raise ProtocolError("args_invalid", "'tail_lines' must be 1..5000")
        return deployments.logs(path, name, dep_id, component, tail, caller)

    def dep_remove(args, caller):
        path, name, dep_id = ref(args, {"delete_data"})
        delete = args.get("delete_data", False)
        if not isinstance(delete, bool):
            raise ProtocolError("args_invalid", "'delete_data' must be a boolean")
        return deployments.remove(path, name, dep_id, delete, caller)

    def health_containers(args, caller):
        _no_args(args)
        from devcoordinator2.daemon import docker_cli, inventory
        try:
            rows = inventory.containers(db, config.unit_prefix)
        except docker_cli.DockerError as exc:
            raise ProtocolError("internal_error", f"docker unavailable: {exc}") from exc
        return {"containers": rows, "counts": inventory.summary(rows)}

    return {
        "deployment.list": dep_list, "deployment.apply": dep_apply,
        "deployment.status": dep_status, "deployment.rollback": dep_rollback,
        "deployment.start": make_control("start"), "deployment.stop": make_control("stop"),
        "deployment.restart": make_control("restart"), "deployment.logs": dep_logs,
        "deployment.remove": dep_remove, "health.containers": health_containers,
    }
