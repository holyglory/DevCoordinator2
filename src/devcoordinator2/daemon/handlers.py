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


def build_handlers(config: InstanceConfig, registry: Registry,
                   lifecycle=None) -> dict[str, Handler]:
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

    return handlers
