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
    _add_common(test_sub.add_parser("status", help="current run summary"))
    output = test_sub.add_parser("output", help="bounded log tail")
    _add_common(output)
    output.add_argument("--stream", choices=["stdout", "stderr"],
                        default="stdout")
    output.add_argument("--tail-bytes", type=int, default=16384)
    _add_common(test_sub.add_parser("stop", help="cancel the current run"))

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
        if action in ("start", "stop", "restart", "logs"):
            sp.add_argument("--component", default=None)
        if action == "logs":
            sp.add_argument("--tail-lines", type=int, default=200)
        if action == "remove":
            sp.add_argument("--delete-data", action="store_true")

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

    repo = sub.add_parser("repository", help="repository registry")
    repo_sub = repo.add_subparsers(dest="action", required=True)
    _add_common(repo_sub.add_parser("list", help="all repositories"),
                with_path=False)
    _add_common(repo_sub.add_parser("status", help="one repository"))
    _add_common(repo_sub.add_parser("register", help="register explicitly"))
    return parser


def _to_call(ns: argparse.Namespace) -> tuple[str, dict]:
    path_args = {}
    if hasattr(ns, "path"):
        path_args["path"] = str(Path(ns.path).absolute())
    match (ns.group, getattr(ns, "action", None)):
        case ("ping", None):
            return "ping", {}
        case ("test", "start"):
            args = dict(path_args)
            if ns.test_name:
                args["test"] = ns.test_name
            return "test.start", args
        case ("test", "status"):
            return "test.status", path_args
        case ("test", "output"):
            return "test.output", {**path_args, "stream": ns.stream,
                                   "tail_bytes": ns.tail_bytes}
        case ("test", "stop"):
            return "test.stop", path_args
        case ("deployment", "list"):
            return "deployment.list", ({"path": str(Path(ns.path).absolute())}
                                       if ns.path else {})
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
            return "repository.list", {}
        case ("repository", "status"):
            return "repository.status", path_args
        case ("repository", "register"):
            return "repository.register", path_args
    raise SystemExit(2)


def main(argv: list[str] | None = None) -> int:
    ns = build_parser().parse_args(argv)
    if ns.group == "daemon":
        from devcoordinator2.daemon.__main__ import main as daemon_main
        return daemon_main()
    if ns.group == "mcp":
        from devcoordinator2.client.mcp_server import main as mcp_main
        return mcp_main()
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
