"""JSON CLI: the universal integration surface.

Every command prints the daemon's response JSON verbatim to stdout.
Exit code 0 when ok, 1 on a daemon-reported error, 2 when unreachable.
`devcoordinator2 daemon` runs the daemon; `devcoordinator2 mcp` runs the
STDIO MCP server. Everything else is a thin protocol call.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

from devcoordinator2.client.common import DaemonUnavailable, call
from devcoordinator2.paths import load_instance_config
from devcoordinator2.protocol import CLIENT_KINDS


def _add_common(parser: argparse.ArgumentParser, with_path: bool = True):
    if with_path:
        parser.add_argument("path", nargs="?", default=os.getcwd(),
                            help="path inside the target worktree (default: cwd)")
    parser.add_argument("--client", choices=CLIENT_KINDS, default="other",
                        help="descriptive client kind (attribution only)")
    parser.add_argument("--session", default=None,
                        help="descriptive client session/task id")


def _add_log_selector(parser: argparse.ArgumentParser, *, stream_required: bool) -> None:
    _add_common(parser)
    parser.add_argument("--run-id", default=None,
                        help="retained run (default: current run)")
    parser.add_argument("--check", default=None)
    parser.add_argument("--phase", choices=["executor", "check", "discovery", "case"],
                        required=stream_required)
    parser.add_argument("--case", default=None)
    parser.add_argument("--stream", choices=["stdout", "stderr"],
                        required=stream_required)
    parser.add_argument("--cursor", default=None)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="devcoordinator2")
    sub = parser.add_subparsers(dest="group", required=True)

    sub.add_parser("daemon", help="run the coordinator daemon")
    sub.add_parser("mcp", help="run the STDIO MCP server")

    ping = sub.add_parser("ping", help="daemon liveness and versions")
    _add_common(ping, with_path=False)

    test = sub.add_parser("test", help="immediate test lifecycle")
    test_sub = test.add_subparsers(dest="action", required=True)
    start = test_sub.add_parser("start", help="start (or supersede) the test")
    _add_common(start)
    start.add_argument("--test", dest="test_name", default=None,
                       help="named test from .devcoordinator.toml")
    start.add_argument("--check", dest="checks", action="append", default=[],
                       help="diagnostic check selection; repeat as needed")
    start.add_argument("--tier", choices=["development", "pre-merge", "release"],
                       default="release",
                       help="validation tier (default: release)")
    retry = test_sub.add_parser("retry", help="retry one failed check from a complete run")
    _add_common(retry)
    retry.add_argument("--test", dest="test_name", default=None,
                       help="named test from .devcoordinator.toml")
    retry.add_argument("--run-id", required=True)
    retry.add_argument("--check", required=True)
    _add_common(test_sub.add_parser("status", help="current run summary"))
    logs = test_sub.add_parser("log", help="progressive governed-test log access")
    log_sub = logs.add_subparsers(dest="log_action", required=True)
    catalog = log_sub.add_parser("catalog", help="content-free retained log catalogue")
    _add_log_selector(catalog, stream_required=False)
    catalog.add_argument("--limit", type=int, default=100)
    tail = log_sub.add_parser("tail", help="bounded final lines of one stream")
    _add_log_selector(tail, stream_required=True)
    tail.add_argument("--lines", type=int, default=50)
    tail.add_argument("--max-bytes", type=int, default=32768)
    search = log_sub.add_parser("search", help="bounded literal search of one stream")
    _add_log_selector(search, stream_required=True)
    search.add_argument("--text", required=True)
    search.add_argument("--max-matches", type=int, default=20)
    search.add_argument("--context-lines", type=int, default=2)
    search.add_argument("--max-bytes", type=int, default=32768)
    exact_range = log_sub.add_parser("range", help="one exact bounded line or byte interval")
    _add_log_selector(exact_range, stream_required=True)
    exact_range.add_argument("--line-start", type=int, default=None)
    exact_range.add_argument("--line-end", type=int, default=None)
    exact_range.add_argument("--byte-start", type=int, default=None)
    exact_range.add_argument("--byte-end", type=int, default=None)
    exact_range.add_argument("--max-bytes", type=int, default=65536)
    failure = log_sub.add_parser(
        "failure-context", help="deterministically ranked bounded failure excerpts")
    _add_log_selector(failure, stream_required=False)
    failure.add_argument("--limit", type=int, default=20)
    failure.add_argument("--context-lines", type=int, default=2)
    failure.add_argument("--max-bytes", type=int, default=32768)
    retention = log_sub.add_parser("retention", help="host log retention settings")
    retention_sub = retention.add_subparsers(dest="retention_action", required=True)
    _add_common(retention_sub.add_parser("show", help="show retention boundaries"),
                with_path=False)
    retention_set = retention_sub.add_parser(
        "set", help="set age and history-depth boundaries and schedule cleanup")
    _add_common(retention_set, with_path=False)
    retention_set.add_argument("--max-age-seconds", type=int, required=True)
    retention_set.add_argument("--case-depth", type=int, required=True)
    stop = test_sub.add_parser("stop", help="cancel the current run")
    _add_common(stop)
    stop.add_argument("--reason", default=None,
                      help="bounded operational reason recorded with cancellation")
    event = test_sub.add_parser("event", help="emit this check's exact completion event")
    event.add_argument("status", choices=["passed", "failed", "unsafe"])
    _add_common(test_sub.add_parser("list", help="current run per worktree"), with_path=False)
    capacity = test_sub.add_parser("capacity", help="host-wide adaptive test capacity")
    capacity_sub = capacity.add_subparsers(dest="capacity_action", required=True)
    _add_common(capacity_sub.add_parser("show", help="show learned and effective capacity"),
                with_path=False)
    capacity_set = capacity_sub.add_parser("set", help="set an administrator maximum")
    _add_common(capacity_set, with_path=False)
    capacity_set.add_argument("cap", type=int)
    _add_common(capacity_sub.add_parser("clear", help="remove the administrator maximum"),
                with_path=False)

    dep = sub.add_parser("deployment", help="deployment lifecycle")
    dep_sub = dep.add_subparsers(dest="action", required=True)
    dl = dep_sub.add_parser("list", help="all deployments (+ declared ones for a path)")
    dl.add_argument("path", nargs="?", default=None)
    _add_common(dl, with_path=False)
    for action, help_text in (("apply", "apply the declared specification"),
                              ("status", "live deployment status"),
                              ("rollback", "return to the previous generation (checkout)"),
                              ("start", "start deployment or one component"),
                              ("stop", "stop deployment or one component"),
                              ("restart", "restart deployment or one component"),
                              ("logs", "bounded component logs"),
                              ("remove", "remove the deployment (keeps data unless told)")):
        sp = dep_sub.add_parser(action, help=help_text)
        _add_common(sp)
        sp.add_argument("--name", default=None, help="deployment name[@source]")
        sp.add_argument("--deployment-id", dest="deployment_id", default=None)
        if action in ("start", "stop", "restart"):
            sp.add_argument(
                "--component", default=None,
                help="component name; lifecycle also accepts reviewed component/service")
        elif action == "logs":
            sp.add_argument("--component", default=None, help="component name")
        if action == "logs":
            sp.add_argument("--tail-lines", type=int, default=200)
        if action == "remove":
            sp.add_argument("--delete-data", action="store_true")
    sd = dep_sub.add_parser("set-domain",
                            help="set or clear the routed domain (administrator)")
    _add_common(sd, with_path=False)
    sd.add_argument("--deployment-id", dest="deployment_id", required=True)
    sd.add_argument("--domain", default=None,
                    help="lowercase DNS label; omit with --clear to remove")
    sd.add_argument("--clear", action="store_true", help="remove the routed domain")
    sd.add_argument("--port", type=int, default=None,
                    help="observed deployments without a route: host port to route to")
    sd.add_argument("--component", default=None,
                    help="observed deployments: which service receives traffic")
    sd.add_argument("--public", dest="public", action="store_true", default=None,
                    help="serve without sign-in at the edge")
    sd.add_argument("--authenticated", dest="public", action="store_false",
                    help="require sign-in at the edge")

    health = sub.add_parser("health", help="host and container health")
    health_sub = health.add_subparsers(dest="action", required=True)
    _add_common(health_sub.add_parser("containers", help="every container, classified"),
                with_path=False)
    _add_common(health_sub.add_parser("summary", help="host condition, alerts, counts"),
                with_path=False)
    _add_common(health_sub.add_parser("repositories", help="per-repository usage"),
                with_path=False)
    _add_common(health_sub.add_parser("repository", help="one repository's components"))
    hist = health_sub.add_parser("history", help="bounded metric series")
    _add_common(hist, with_path=False)
    hist.add_argument("--subject-kind", required=True)
    hist.add_argument("--subject-id", required=True)
    hist.add_argument("--metric", required=True)
    hist.add_argument("--minutes", type=int, default=60)

    bug = sub.add_parser("bug", help="open bug registry (works without the daemon)")
    bug_sub = bug.add_subparsers(dest="action", required=True)
    br = bug_sub.add_parser("report", help="open (or count a recurrence of) a bug")
    for field in ("component", "summary", "expected", "actual", "steps"):
        br.add_argument(f"--{field}", required=True)
    br.add_argument("--run-id", default=None)
    br.add_argument("--deployment-id", default=None)
    bug_sub.add_parser("list", help="all open bugs")
    bc = bug_sub.add_parser("close", help="close (remove) an open bug")
    bc.add_argument("bug_id")

    tg = sub.add_parser("telegram", help="notification subscriptions")
    tg_sub = tg.add_subparsers(dest="action", required=True)
    _add_common(tg_sub.add_parser("list", help="linked chats and subscriptions"),
                with_path=False)
    tl = tg_sub.add_parser("link", help="link a chat (code from /start) to an e-mail")
    _add_common(tl, with_path=False)
    tl.add_argument("--code", required=True)
    tl.add_argument("--email", required=True)
    for action in ("subscribe", "unsubscribe"):
        ts = tg_sub.add_parser(action)
        _add_common(ts, with_path=False)
        ts.add_argument("--chat-id", type=int, required=True)
        ts.add_argument("--scope", required=True,
                        help="server | deployment:<id> | repository:<id>")

    repo = sub.add_parser("repository", help="repository registry")
    repo_sub = repo.add_subparsers(dest="action", required=True)
    repo_list = repo_sub.add_parser("list", help="active repositories")
    _add_common(repo_list, with_path=False)
    repo_list.add_argument("--all", action="store_true", help="include archived repositories")
    _add_common(repo_sub.add_parser("status", help="one repository"))
    _add_common(repo_sub.add_parser("register", help="register explicitly"))
    repo_archive = repo_sub.add_parser(
        "archive", help="retire a repository and preserve history")
    _add_common(repo_archive, with_path=False)
    repo_archive.add_argument("repository_id")
    repo_archive.add_argument("--into", dest="merged_into_repository_id", required=True)
    repo_archive.add_argument("--note", required=True)
    repo_unarchive = repo_sub.add_parser("unarchive", help="restore an archived repository")
    _add_common(repo_unarchive, with_path=False)
    repo_unarchive.add_argument("repository_id")
    repo_unarchive.add_argument("--note", required=True)

    plan = sub.add_parser("plan", help="planning and completion ledger")
    plan_sub = plan.add_subparsers(dest="action", required=True)
    po = plan_sub.add_parser("overview", help="releases, task tree, preview requests")
    _add_common(po)
    po.add_argument("--all", action="store_true",
                    help="list every repository's plan summary instead of one plan")

    task = sub.add_parser("task", help="completion-ledger tasks (plain language)")
    task_sub = task.add_subparsers(dest="action", required=True)
    tc = task_sub.add_parser("create", help="record a work item in the ledger")
    _add_common(tc)
    tc.add_argument("--title", required=True, help="one plain sentence, user terms")
    tc.add_argument("--kind", required=True,
                    choices=["goal", "stub", "improvement", "user_feedback"])
    for field in ("outcome", "impact", "unblock-condition", "verification",
                  "technical-note", "parent-task-id", "release-id"):
        tc.add_argument(f"--{field}", default=None)
    tc.add_argument("--estimated-loc", type=int, default=None,
                    help="size in estimated lines of code")
    tu = task_sub.add_parser("update", help="append-only task mutation")
    _add_common(tu, with_path=False)
    tu.add_argument("task_id")
    tu.add_argument("--status", default=None,
                    choices=["planned", "in_progress", "done", "dropped"])
    for field in ("title", "outcome", "impact", "unblock-condition", "verification",
                  "technical-note", "note", "parent-task-id", "release-id"):
        tu.add_argument(f"--{field}", default=None)
    tu.add_argument("--estimated-loc", type=int, default=None)
    tu.add_argument("--position", type=int, default=None,
                    help="0-based order among siblings")
    tu.add_argument("--backlog", action="store_true",
                    help="move out of every release (release_id = null)")
    tu.add_argument("--root", action="store_true",
                    help="detach from the parent task (parent_task_id = null)")
    elaboration = tu.add_mutually_exclusive_group()
    elaboration.add_argument(
        "--elaboration-needed", dest="elaboration_needed", action="store_const",
        const=True, default=None,
        help="record that the owner needs clearer task wording")
    elaboration.add_argument(
        "--elaboration-complete", dest="elaboration_needed", action="store_const",
        const=False,
        help="clear the request; also pass a changed --title or --outcome")
    th = task_sub.add_parser("history", help="one task with its permanent history")
    _add_common(th, with_path=False)
    th.add_argument("task_id")

    release = sub.add_parser("release", help="releases and preview releases")
    release_sub = release.add_subparsers(dest="action", required=True)
    rc = release_sub.add_parser("create", help="plan a release on the chart")
    _add_common(rc)
    rc.add_argument("--name", required=True)
    rc.add_argument("--kind", required=True, choices=["preview", "release"])
    rc.add_argument("--note", default=None)
    rc.add_argument("--seq", type=int, default=None)
    ru = release_sub.add_parser("update", help="rename/reorder/drop (administrator)")
    _add_common(ru, with_path=False)
    ru.add_argument("--release-id", dest="release_id", required=True)
    ru.add_argument("--name", default=None)
    ru.add_argument("--seq", type=int, default=None)
    ru.add_argument("--note", default=None)
    ru.add_argument("--status", default=None, choices=["planned", "dropped"])
    rr = release_sub.add_parser("request",
                                help="ask for a preview of the current work ASAP")
    _add_common(rr)
    rr.add_argument("--name", default=None)
    rr.add_argument("--note", default=None)
    rd = release_sub.add_parser("deliver",
                                help="record the real deployment that delivered it")
    _add_common(rd, with_path=False)
    rd.add_argument("--release-id", dest="release_id", required=True)
    rd.add_argument("--deployment-id", dest="deployment_id", required=True)
    rd.add_argument("--note", default=None)

    decision = sub.add_parser("decision", help="per-repository decision history")
    decision_sub = decision.add_subparsers(dest="action", required=True)
    aspects = ["ui", "architecture", "algorithms", "business_logic", "data",
               "testing", "deployment", "security", "performance", "process", "other"]
    dr = decision_sub.add_parser("record", help="record a decision (plain language)")
    _add_common(dr)
    dr.add_argument("--aspect", required=True, choices=aspects)
    dr.add_argument("--title", required=True)
    dr.add_argument("--body", required=True,
                    help="what was decided and why, in management terms")
    dr.add_argument("--technical-note", default=None)
    dr.add_argument("--ref", default=None, help="stable citation key")
    dr.add_argument("--supersedes", default=None, help="decision id or ref")
    dt = decision_sub.add_parser("tail", help="rolling summary + last N decisions")
    _add_common(dt)
    dt.add_argument("--aspect", default=None, choices=aspects)
    dt.add_argument("-n", type=int, default=None)
    ds = decision_sub.add_parser("search", help="full-text search over decisions")
    _add_common(ds)
    ds.add_argument("--query", required=True)
    ds.add_argument("--aspect", default=None, choices=aspects)
    ds.add_argument("-n", type=int, default=None)
    dz = decision_sub.add_parser("summarize", help="store the rolling summary")
    _add_common(dz)
    dz.add_argument("--body", required=True)
    dz.add_argument("--covers-through-seq", dest="covers_through_seq", type=int,
                    required=True)
    return parser


def _to_call(ns: argparse.Namespace) -> tuple[str, dict]:
    path_args = {}
    if hasattr(ns, "path") and ns.path is not None:
        path_args["path"] = str(Path(ns.path).absolute())
    match (ns.group, getattr(ns, "action", None)):
        case ("ping", None):
            return "ping", {}
        case ("test", "start"):
            args = dict(path_args)
            if ns.test_name:
                args["test"] = ns.test_name
            if ns.checks:
                args["checks"] = ns.checks
            args["tier"] = ns.tier
            return "test.start", args
        case ("test", "retry"):
            args = {**path_args, "run_id": ns.run_id, "check": ns.check}
            if ns.test_name:
                args["test"] = ns.test_name
            return "test.retry", args
        case ("test", "status"):
            return "test.status", path_args
        case ("test", "log"):
            if ns.log_action == "retention":
                if ns.retention_action == "show":
                    return "test.log.retention.get", {}
                return "test.log.retention.set", {
                    "max_age_seconds": ns.max_age_seconds,
                    "case_depth": ns.case_depth,
                }
            args = dict(path_args)
            for key in ("run_id", "check", "phase", "case", "stream", "cursor"):
                value = getattr(ns, key, None)
                if value is not None:
                    args[key] = value
            if ns.log_action == "catalog":
                args["limit"] = ns.limit
            elif ns.log_action == "tail":
                args.update(lines=ns.lines, max_bytes=ns.max_bytes)
            elif ns.log_action == "search":
                args.update(text=ns.text, max_matches=ns.max_matches,
                            context_lines=ns.context_lines, max_bytes=ns.max_bytes)
            elif ns.log_action == "range":
                for key in ("line_start", "line_end", "byte_start", "byte_end"):
                    value = getattr(ns, key)
                    if value is not None:
                        args[key] = value
                args["max_bytes"] = ns.max_bytes
            else:
                args.update(limit=ns.limit, context_lines=ns.context_lines,
                            max_bytes=ns.max_bytes)
            return f"test.log.{ns.log_action.replace('-', '_')}", args
        case ("test", "stop"):
            return "test.stop", ({**path_args, "reason": ns.reason}
                                 if ns.reason else path_args)
        case ("test", "list"):
            return "test.list", {}
        case ("test", "capacity"):
            if ns.capacity_action == "show":
                return "test.capacity.get", {}
            return "test.capacity.set", {
                "cap": ns.cap if ns.capacity_action == "set" else None}
        case ("deployment", "list"):
            return "deployment.list", ({"path": str(Path(ns.path).absolute())}
                                       if ns.path else {})
        case ("deployment", "set-domain"):
            if bool(ns.domain) == bool(ns.clear):
                raise SystemExit("pass exactly one of --domain <label> or --clear")
            args = {"deployment_id": ns.deployment_id, "domain": ns.domain}
            for key in ("port", "component", "public"):
                if getattr(ns, key) is not None:
                    args[key] = getattr(ns, key)
            return "deployment.set_domain", args
        case ("deployment", action):
            args = dict(path_args)
            if ns.name:
                args["name"] = ns.name
            if ns.deployment_id:
                args["deployment_id"] = ns.deployment_id
            if getattr(ns, "component", None):
                args["component"] = ns.component
            if action == "logs":
                args["tail_lines"] = ns.tail_lines
                if not ns.component:
                    raise SystemExit("deployment logs requires --component")
            if action == "remove":
                args["delete_data"] = ns.delete_data
            return f"deployment.{action}", args
        case ("telegram", "list"):
            return "telegram.list", {}
        case ("telegram", "link"):
            return "telegram.link", {"code": ns.code, "email": ns.email}
        case ("telegram", action):
            return f"telegram.{action}", {"chat_id": ns.chat_id, "scope": ns.scope}
        case ("health", "containers"):
            return "health.containers", {}
        case ("health", "summary"):
            return "health.summary", {}
        case ("health", "repositories"):
            return "health.repositories", {}
        case ("health", "repository"):
            return "health.repository", path_args
        case ("health", "history"):
            return "health.history", {"subject_kind": ns.subject_kind,
                                      "subject_id": ns.subject_id, "metric": ns.metric,
                                      "minutes": ns.minutes}
        case ("repository", "list"):
            return "repository.list", ({"include_archived": True} if ns.all else {})
        case ("repository", "status"):
            return "repository.status", path_args
        case ("repository", "register"):
            return "repository.register", path_args
        case ("repository", "archive"):
            return "repository.archive", {
                "repository_id": ns.repository_id,
                "merged_into_repository_id": ns.merged_into_repository_id,
                "note": ns.note,
            }
        case ("repository", "unarchive"):
            return "repository.unarchive", {
                "repository_id": ns.repository_id,
                "note": ns.note,
            }
        case ("plan", "overview"):
            return "plan.overview", ({} if ns.all else path_args)
        case ("task", "create"):
            args = {**path_args, "title": ns.title, "kind": ns.kind}
            for key in ("outcome", "impact", "unblock_condition", "verification",
                        "technical_note", "parent_task_id", "release_id",
                        "estimated_loc"):
                if getattr(ns, key) is not None:
                    args[key] = getattr(ns, key)
            return "task.create", args
        case ("task", "update"):
            args = {"task_id": ns.task_id}
            for key in ("title", "outcome", "impact", "unblock_condition",
                        "verification", "technical_note", "note", "status",
                        "estimated_loc", "position", "elaboration_needed"):
                if getattr(ns, key) is not None:
                    args[key] = getattr(ns, key)
            if ns.backlog and ns.release_id:
                raise SystemExit("pass --release-id or --backlog, not both")
            if ns.backlog:
                args["release_id"] = None
            elif ns.release_id is not None:
                args["release_id"] = ns.release_id
            if ns.root and ns.parent_task_id:
                raise SystemExit("pass --parent-task-id or --root, not both")
            if ns.root:
                args["parent_task_id"] = None
            elif ns.parent_task_id is not None:
                args["parent_task_id"] = ns.parent_task_id
            return "task.update", args
        case ("task", "history"):
            return "task.history", {"task_id": ns.task_id}
        case ("release", "create"):
            args = {**path_args, "name": ns.name, "kind": ns.kind}
            for key in ("note", "seq"):
                if getattr(ns, key) is not None:
                    args[key] = getattr(ns, key)
            return "release.create", args
        case ("release", "update"):
            args = {"release_id": ns.release_id}
            for key in ("name", "seq", "note", "status"):
                if getattr(ns, key) is not None:
                    args[key] = getattr(ns, key)
            return "release.update", args
        case ("release", "request"):
            args = dict(path_args)
            for key in ("name", "note"):
                if getattr(ns, key) is not None:
                    args[key] = getattr(ns, key)
            return "release.request", args
        case ("release", "deliver"):
            args = {"release_id": ns.release_id, "deployment_id": ns.deployment_id}
            if ns.note is not None:
                args["note"] = ns.note
            return "release.deliver", args
        case ("decision", "record"):
            args = {**path_args, "aspect": ns.aspect, "title": ns.title,
                    "body": ns.body}
            for key in ("technical_note", "ref", "supersedes"):
                if getattr(ns, key) is not None:
                    args[key] = getattr(ns, key)
            return "decision.record", args
        case ("decision", "tail"):
            args = dict(path_args)
            if ns.aspect is not None:
                args["aspect"] = ns.aspect
            if ns.n is not None:
                args["n"] = ns.n
            return "decision.tail", args
        case ("decision", "search"):
            args = {**path_args, "query": ns.query}
            if ns.aspect is not None:
                args["aspect"] = ns.aspect
            if ns.n is not None:
                args["n"] = ns.n
            return "decision.search", args
        case ("decision", "summarize"):
            return "decision.summarize", {**path_args, "body": ns.body,
                                          "covers_through_seq": ns.covers_through_seq}
    raise SystemExit(2)


def _bug_command(ns: argparse.Namespace) -> int:
    """Bugs are written to the independent store directly, so intake works
    while the daemon, its database, or the edge is down. The daemon is told
    best-effort afterwards so subscribers get notified."""
    from devcoordinator2 import bugs
    config = load_instance_config()
    try:
        if ns.action == "report":
            correlations = {k: v for k, v in (("run_id", ns.run_id),
                                              ("deployment_id", ns.deployment_id)) if v}
            result = bugs.report(component=ns.component, summary=ns.summary,
                                 expected=ns.expected, actual=ns.actual, steps=ns.steps,
                                 correlations=correlations, reporter=f"uid:{os.getuid()}",
                                 directory=config.bugs_dir)
        elif ns.action == "list":
            result = {"bugs": bugs.list_open(config.bugs_dir), "store": str(config.bugs_dir)}
        else:
            result = bugs.close(ns.bug_id, config.bugs_dir)
    except bugs.BugError as exc:
        print(json.dumps({"ok": False, "error": {"code": "args_invalid", "message": str(exc)}}))
        return 1
    if ns.action != "list":
        try:  # notify subscribers; the record is already durable either way
            call(config.socket_path, "bug.report" if ns.action == "report" else "bug.close",
                 {"component": ns.component, "summary": ns.summary, "expected": ns.expected,
                  "actual": ns.actual, "steps": ns.steps} if ns.action == "report"
                 else {"bug_id": ns.bug_id})
        except DaemonUnavailable:
            result["notified"] = False
    print(json.dumps({"ok": True, "result": result}, indent=2))
    return 0


def main(argv: list[str] | None = None) -> int:
    ns = build_parser().parse_args(argv)
    if ns.group == "daemon":
        from devcoordinator2.daemon.__main__ import main as daemon_main
        return daemon_main()
    if ns.group == "mcp":
        from devcoordinator2.client.mcp_server import main as mcp_main
        return mcp_main()
    if ns.group == "test" and ns.action == "event":
        try:
            fd = int(os.environ["DEVCOORDINATOR_EVENT_FD"])
            payload = {
                "schema": 2,
                "run_id": os.environ["DEVCOORDINATOR_RUN_ID"],
                "check": os.environ["DEVCOORDINATOR_CHECK_NAME"],
                "status": ns.status,
            }
            os.write(fd, (json.dumps(payload, separators=(",", ":")) + "\n").encode())
        except (KeyError, ValueError, OSError) as exc:
            print(f"cannot emit governed-check event: {exc}", file=sys.stderr)
            return 2
        return 0
    if ns.group == "bug":
        return _bug_command(ns)
    command, args = _to_call(ns)
    config = load_instance_config()
    try:
        response = call(config.socket_path, command, args,
                        client_kind=ns.client, client_session=ns.session)
    except DaemonUnavailable as exc:
        print(json.dumps({"ok": False, "error": {
            "code": "daemon_unavailable", "message": str(exc)}}))
        return 2
    print(json.dumps(response, indent=2))
    return 0 if response.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
