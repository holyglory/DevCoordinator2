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


def build_notification_handlers(config: InstanceConfig, telegram, access) -> dict[str, Handler]:
    """telegram.* (scoped by what the identity may view) and bug.* (shared
    independent store; the daemon path only adds event emission)."""
    from devcoordinator2 import bugs
    from devcoordinator2.daemon import events

    def _chat_for(args: dict[str, Any], caller: Caller) -> int:
        chat_id = args.get("chat_id")
        if not isinstance(chat_id, int) or isinstance(chat_id, bool):
            raise ProtocolError("args_invalid", "'chat_id' (integer) is required")
        principal = access.principal(caller)
        if not principal.local and not principal.administrator \
                and telegram.chat_email(chat_id) != principal.identity:
            raise ProtocolError("permission_denied", "chat is linked to another identity")
        return chat_id

    def tg_link(args, caller):
        _only(args, {"code", "email"})
        principal = access.principal(caller)
        email = args.get("email") or principal.identity
        if not isinstance(email, str) or "@" not in email:
            raise ProtocolError("args_invalid", "'email' is required")
        if not principal.local and not principal.administrator and email != principal.identity:
            raise ProtocolError("permission_denied", "may only link chats to yourself")
        return telegram.link(str(args.get("code", "")), email.lower())

    def tg_subscribe(args, caller):
        _only(args, {"chat_id", "scope"})
        chat_id = _chat_for(args, caller)
        scope = str(args.get("scope", ""))
        from devcoordinator2.daemon.telegram import parse_scope
        kind, ident = parse_scope(scope)
        principal = access.principal(caller)
        if not principal.local and not principal.administrator:
            if kind == "server":
                raise ProtocolError("permission_denied", "server scope requires administrator")
            if kind == "deployment" and not principal.at_least(ident, "viewer"):
                raise ProtocolError("permission_denied", "viewer on the deployment required")
            if kind == "repository":
                allowed = set()
                if principal.grants:
                    marks = ",".join("?" * len(principal.grants))
                    allowed = {r["repository_id"] for r in access.db.query(
                        "SELECT repository_id FROM deployments WHERE deployment_id IN"
                        f" ({marks})", tuple(principal.grants))}
                if ident not in allowed:
                    raise ProtocolError("permission_denied",
                                        "no viewable deployment in that repository")
        return telegram.subscribe(chat_id, scope)

    def tg_unsubscribe(args, caller):
        _only(args, {"chat_id", "scope"})
        chat_id = _chat_for(args, caller)
        return telegram.unsubscribe(chat_id, str(args.get("scope", "")))

    def tg_list(args, caller):
        _no_args(args)
        principal = access.principal(caller)
        email = None if principal.local or principal.administrator else principal.identity
        return telegram.listing(email)

    def bug_report(args, caller):
        _only(args, {"component", "summary", "expected", "actual", "steps", "correlations"})
        principal = access.principal(caller)
        try:
            record = bugs.report(component=args.get("component"), summary=args.get("summary"),
                                 expected=args.get("expected"), actual=args.get("actual"),
                                 steps=args.get("steps"), correlations=args.get("correlations"),
                                 reporter=principal.identity or f"uid:{caller.uid}",
                                 directory=config.bugs_dir)
        except bugs.BugError as exc:
            raise ProtocolError("args_invalid", str(exc)) from exc
        if not record["duplicate"]:
            events.publish("bug.opened", bug_id=record["bug_id"], component=record["component"],
                           summary=record["summary"],
                           repository_id=record["correlations"].get("repository_id"),
                           deployment_id=record["correlations"].get("deployment_id"))
        return record

    def bug_list(args, caller):
        _no_args(args)
        return {"bugs": bugs.list_open(config.bugs_dir), "store": str(config.bugs_dir)}

    def bug_close(args, caller):
        _only(args, {"bug_id"})
        try:
            result = bugs.close(str(args.get("bug_id", "")), config.bugs_dir)
        except bugs.BugError as exc:
            raise ProtocolError("args_invalid", str(exc)) from exc
        events.publish("bug.closed", bug_id=result["bug_id"], component=result["component"],
                       summary=result["summary"])
        return result

    return {"telegram.link": tg_link, "telegram.subscribe": tg_subscribe,
            "telegram.unsubscribe": tg_unsubscribe, "telegram.list": tg_list,
            "bug.report": bug_report, "bug.list": bug_list, "bug.close": bug_close}


def _only(args: dict[str, Any], allowed: set[str]) -> None:
    unknown = set(args) - allowed
    if unknown:
        raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")


def _deployment_handlers(config: InstanceConfig, deployments, db) -> dict[str, Handler]:
    ref_keys = {"path", "name", "deployment_id"}

    def ref(args: dict[str, Any], extra: set[str] = frozenset()):
        unknown = set(args) - (ref_keys | extra)
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        dep_id = _optional_str(args, "deployment_id")
        if dep_id is not None and not args.get("path"):
            return None, _optional_str(args, "name"), dep_id
        path = _require_path({k: v for k, v in args.items() if k == "path"}, {"path"})
        return path, _optional_str(args, "name"), dep_id

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
