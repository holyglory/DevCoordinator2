"""Command handlers: args validation and dispatch to registry / lifecycle."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from devcoordinator2 import __version__
from devcoordinator2.daemon.db import SCHEMA_VERSION
from devcoordinator2.daemon.gitinfo import GitResolveError
from devcoordinator2.daemon.registry import (
    Registry,
    RepositoryArchiveBlocked,
    RepositoryArchived,
)
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
                   lifecycle=None, deployments=None, db=None,
                   capacity=None, test_logs=None, test_evidence=None) -> dict[str, Handler]:
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
        except RepositoryArchived as exc:
            raise ProtocolError("repository_archived", str(exc)) from exc
        return {
            "repository_id": reg.repository_id,
            "worktree_id": reg.worktree_id,
            "root_path": reg.root_path,
            "worktree_path": reg.worktree_path,
            "display_name": reg.display_name,
            "registered": reg.newly_registered,
        }

    def repository_list(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        unknown = set(args) - {"include_archived"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        include_archived = args.get("include_archived", False)
        if not isinstance(include_archived, bool):
            raise ProtocolError("args_invalid", "'include_archived' must be true or false")
        return {"repositories": registry.list_repositories(
            include_archived=include_archived)}

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

    def repository_archive(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        unknown = set(args) - {"repository_id", "merged_into_repository_id", "note"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        repository_id = args.get("repository_id")
        target_id = args.get("merged_into_repository_id")
        note = args.get("note")
        if not isinstance(repository_id, str) or not repository_id.startswith("r"):
            raise ProtocolError("args_invalid", "'repository_id' must be an 'r…' id")
        if not isinstance(target_id, str) or not target_id.startswith("r"):
            raise ProtocolError(
                "args_invalid", "'merged_into_repository_id' must be an 'r…' id")
        if not isinstance(note, str) or not 3 <= len(note) <= 500 or "\n" in note:
            raise ProtocolError("args_invalid", "'note' must be one 3..500 character line")
        try:
            return registry.archive(repository_id, target_id, note, caller.uid)
        except RepositoryArchiveBlocked as exc:
            raise ProtocolError("repository_archive_blocked", str(exc)) from exc

    def repository_unarchive(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        unknown = set(args) - {"repository_id", "note"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        repository_id = args.get("repository_id")
        note = args.get("note")
        if not isinstance(repository_id, str) or not repository_id.startswith("r"):
            raise ProtocolError("args_invalid", "'repository_id' must be an 'r…' id")
        if not isinstance(note, str) or not 3 <= len(note) <= 500 or "\n" in note:
            raise ProtocolError("args_invalid", "'note' must be one 3..500 character line")
        try:
            return registry.unarchive(repository_id, note, caller.uid)
        except RepositoryArchiveBlocked as exc:
            raise ProtocolError("repository_archive_blocked", str(exc)) from exc

    handlers: dict[str, Handler] = {
        "ping": ping,
        "repository.register": repository_register,
        "repository.list": repository_list,
        "repository.status": repository_status,
        "repository.archive": repository_archive,
        "repository.unarchive": repository_unarchive,
    }

    if lifecycle is not None:
        def test_start(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            unknown = set(args) - {"path", "test", "checks", "tier"}
            if unknown:
                raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
            path = _require_path({"path": args.get("path")}, {"path"})
            test = args.get("test")
            if test is not None and not isinstance(test, str):
                raise ProtocolError("args_invalid", "'test' must be a string")
            checks = args.get("checks", [])
            if not isinstance(checks, list) or not all(
                    isinstance(value, str) for value in checks):
                raise ProtocolError("args_invalid", "'checks' must be an array of strings")
            tier = args.get("tier", "release")
            if tier not in ("development", "pre-merge", "release"):
                raise ProtocolError(
                    "args_invalid",
                    "'tier' must be development, pre-merge, or release")
            return lifecycle.start(
                path, test, caller, checks=tuple(checks), tier=tier)

        def test_retry(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            unknown = set(args) - {"path", "test", "run_id", "check"}
            if unknown:
                raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
            path = _require_path({"path": args.get("path")}, {"path"})
            test = args.get("test")
            run_id = args.get("run_id")
            check = args.get("check")
            if test is not None and not isinstance(test, str):
                raise ProtocolError("args_invalid", "'test' must be a string")
            if not isinstance(run_id, str) or not run_id:
                raise ProtocolError("args_invalid", "'run_id' must be a string")
            if not isinstance(check, str) or not check:
                raise ProtocolError("args_invalid", "'check' must be a string")
            return lifecycle.start(
                path, test, caller, retry_run_id=run_id, retry_check=check)

        def test_status(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(args, {"path"})
            return lifecycle.status(path, caller)

        def test_stop(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(args, {"path", "reason"})
            reason = args.get("reason")
            if reason is not None and (
                    not isinstance(reason, str) or not (3 <= len(reason) <= 256)
                    or "\n" in reason or "\r" in reason):
                raise ProtocolError(
                    "args_invalid", "'reason' must be one 3..256 character line")
            return lifecycle.stop(path, caller, reason)

        def test_list(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            _no_args(args)
            return {"runs": lifecycle.list_current()}

        handlers.update({
            "test.start": test_start,
            "test.retry": test_retry,
            "test.status": test_status,
            "test.stop": test_stop,
            "test.list": test_list,
        })

    if test_logs is not None:
        log_args = {
            "catalog": {
                "run_id", "check", "phase", "case", "stream", "cursor", "limit",
            },
            "tail": {
                "run_id", "check", "phase", "case", "stream", "cursor", "lines",
                "max_bytes",
            },
            "search": {
                "run_id", "check", "phase", "case", "stream", "cursor", "text",
                "max_matches", "context_lines", "max_bytes",
            },
            "range": {
                "run_id", "check", "phase", "case", "stream", "cursor",
                "line_start", "line_end", "byte_start", "byte_end", "max_bytes",
            },
            "failure_context": {
                "run_id", "check", "phase", "case", "stream", "cursor", "limit",
                "context_lines", "max_bytes",
            },
        }

        def log_handler(operation: str):
            def handle(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
                allowed = {"path", *log_args[operation]}
                path = _require_path(args, allowed)
                return test_logs.query(
                    operation, path,
                    {key: value for key, value in args.items() if key != "path"},
                    caller,
                )
            return handle

        def test_log_retention_get(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            _no_args(args)
            return test_logs.retention()

        def test_log_retention_set(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            unknown = set(args) - {"max_age_seconds", "case_depth"}
            if unknown:
                raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
            if set(args) != {"max_age_seconds", "case_depth"}:
                raise ProtocolError(
                    "args_invalid", "max_age_seconds and case_depth are required")
            actor = caller.identity or f"uid:{caller.uid}"
            return test_logs.set_retention(
                args["max_age_seconds"], args["case_depth"], actor)

        handlers.update({
            "test.log.catalog": log_handler("catalog"),
            "test.log.tail": log_handler("tail"),
            "test.log.search": log_handler("search"),
            "test.log.range": log_handler("range"),
            "test.log.failure_context": log_handler("failure_context"),
            "test.log.retention.get": test_log_retention_get,
            "test.log.retention.set": test_log_retention_set,
        })

    if test_evidence is not None:
        def evidence_get(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(args, {"path", "run_id"})
            return test_evidence.get(path, args.get("run_id"), caller)

        def evidence_image(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(
                args, {"path", "run_id", "image_id", "offset", "max_bytes"})
            return test_evidence.image(
                path, {key: value for key, value in args.items() if key != "path"}, caller)

        def feedback_create(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(
                args, {"path", "run_id", "image_id", "body", "marks"})
            return test_evidence.create_feedback(
                path, {key: value for key, value in args.items() if key != "path"}, caller)

        def feedback_reply(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(
                args, {"path", "run_id", "feedback_id", "body"})
            return test_evidence.reply(
                path, {key: value for key, value in args.items() if key != "path"}, caller)

        def feedback_edit(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(
                args, {"path", "run_id", "feedback_id", "comment_id", "body"})
            return test_evidence.edit(
                path, {key: value for key, value in args.items() if key != "path"}, caller)

        def feedback_state(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(
                args, {"path", "run_id", "feedback_id", "state"})
            return test_evidence.set_state(
                path, {key: value for key, value in args.items() if key != "path"}, caller)

        def feedback_delete(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            path = _require_path(args, {"path", "run_id", "feedback_id"})
            return test_evidence.delete(
                path, {key: value for key, value in args.items() if key != "path"}, caller)

        handlers.update({
            "test.evidence.get": evidence_get,
            "test.evidence.image": evidence_image,
            "test.evidence.feedback.create": feedback_create,
            "test.evidence.feedback.reply": feedback_reply,
            "test.evidence.feedback.edit": feedback_edit,
            "test.evidence.feedback.state": feedback_state,
            "test.evidence.feedback.delete": feedback_delete,
        })

    if capacity is not None:
        def test_capacity_get(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            _no_args(args)
            return capacity.snapshot()

        def test_capacity_set(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
            unknown = set(args) - {"cap"}
            if unknown:
                raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
            if "cap" not in args:
                raise ProtocolError("args_invalid", "'cap' is required (integer or null)")
            cap = args["cap"]
            if cap is not None and (
                    not isinstance(cap, int) or isinstance(cap, bool)):
                raise ProtocolError("args_invalid", "'cap' must be an integer or null")
            actor = caller.identity or f"uid:{caller.uid}"
            try:
                return capacity.set_cap(cap, actor)
            except ValueError as exc:
                raise ProtocolError("args_invalid", str(exc)) from exc

        handlers.update({
            "test.capacity.get": test_capacity_get,
            "test.capacity.set": test_capacity_set,
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
                    allowed.update(r["repository_id"] for r in access.db.query(
                        "SELECT repository_id FROM observed_deployments"
                        " WHERE observed_deployment_id IN"
                        f" ({marks})", tuple(principal.grants)))
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
    from devcoordinator2.daemon import deploy_state, events, observed, routes

    ref_keys = {"path", "name", "deployment_id"}

    def reject_observed(dep_id: str | None) -> None:
        if observed.exists(db, dep_id):
            raise ProtocolError(
                "observed_only",
                "observed deployments have no configuration authority here (start/stop/"
                "restart and logs work on the exact recorded containers); adopt the"
                " resource through reviewed repository configuration first",
            )

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
        resolved = ref(args)
        reject_observed(resolved[2])
        return deployments.apply(*resolved, caller)

    def dep_status(args, caller):
        resolved = ref(args)
        if resolved[2]:
            imported = observed.status(db, resolved[2])
            if imported is not None:
                return imported
        return deployments.status(*resolved, caller)

    def dep_rollback(args, caller):
        resolved = ref(args)
        reject_observed(resolved[2])
        return deployments.rollback(*resolved, caller)

    def make_control(action):
        def handler(args, caller):
            path, name, dep_id = ref(args, {"component"})
            component = _optional_str(args, "component")
            if observed.exists(db, dep_id):
                # DC2-2026-08-24-OBSERVED-LIFECYCLE: act on the exact recorded
                # containers; never recreate or reconfigure.
                result = observed.control(db, action, dep_id, component)
                events.publish(f"deployment.{action}", deployment_id=dep_id,
                               name=result["name"], source="observed",
                               component=component,
                               repository_id=result["repository_id"],
                               state=result["state"], caller_uid=caller.uid)
                return result
            return deployments.control(action, path, name, dep_id, component, caller)
        return handler

    def dep_logs(args, caller):
        path, name, dep_id = ref(args, {"component", "tail_lines"})
        component = _optional_str(args, "component")
        if not component:
            raise ProtocolError("args_invalid", "'component' is required")
        tail = args.get("tail_lines", 200)
        if not isinstance(tail, int) or not (1 <= tail <= 5000):
            raise ProtocolError("args_invalid", "'tail_lines' must be 1..5000")
        if observed.exists(db, dep_id):
            return observed.logs(db, dep_id, component, tail)
        return deployments.logs(path, name, dep_id, component, tail, caller)

    def dep_set_domain(args, caller):
        unknown = set(args) - {"deployment_id", "domain", "port", "component", "public"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        dep_id = args.get("deployment_id")
        if not isinstance(dep_id, str) or not dep_id.startswith("d"):
            raise ProtocolError("args_invalid", "'deployment_id' is required")
        domain = args.get("domain")
        if domain is not None:
            if not isinstance(domain, str) \
                    or not observed.DOMAIN_LABEL_RE.fullmatch(domain):
                raise ProtocolError("args_invalid",
                                    "'domain' must be a lowercase DNS label"
                                    " (a-z, 0-9, hyphen), or null to clear")
            owner = deploy_state.domain_owner(db, domain)
            if owner is not None and owner != dep_id:
                raise ProtocolError("args_invalid",
                                    f"domain {domain!r} is already routed to {owner}")
        port = args.get("port")
        if port is not None and (not isinstance(port, int) or isinstance(port, bool)
                                 or not (1 <= port <= 65535)):
            raise ProtocolError("args_invalid", "'port' must be 1..65535")
        public = args.get("public")
        if public is not None and not isinstance(public, bool):
            raise ProtocolError("args_invalid", "'public' must be a boolean")
        if observed.exists(db, dep_id):
            result = observed.set_domain(db, dep_id, domain, port,
                                         _optional_str(args, "component"), public)
        else:
            if port is not None or args.get("component") is not None:
                raise ProtocolError("args_invalid",
                                    "'port'/'component' apply only to observed"
                                    " deployments; managed routes come from the"
                                    " declared route component")
            try:
                result = deploy_state.override_domain(db, dep_id, domain)
            except ValueError as exc:
                raise ProtocolError("deployment_not_found" if "no deployment"
                                    in str(exc) else "args_invalid", str(exc)) from exc
            if public is not None:
                deploy_state.set_deployment(db, dep_id, public=int(public))
                result["public"] = public
        routes.publish(db, config.routes_path, config.base_domain)
        events.publish("deployment.domain_changed", deployment_id=dep_id,
                       domain=result.get("domain"), caller_uid=caller.uid)
        return result

    def dep_remove(args, caller):
        path, name, dep_id = ref(args, {"delete_data"})
        reject_observed(dep_id)
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
        "deployment.set_domain": dep_set_domain,
        "deployment.remove": dep_remove, "health.containers": health_containers,
    }
