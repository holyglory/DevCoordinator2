"""STDIO MCP server: newline-delimited JSON-RPC 2.0, stdlib only.

Implements initialize / notifications/initialized / ping / tools/list /
tools/call. Each tool returns the exact daemon protocol response JSON as
text content, so MCP, CLI, and Console share one result schema.
"""

from __future__ import annotations

import json
import sys
from typing import Any

from devcoordinator2 import __version__
from devcoordinator2.client.common import DaemonUnavailable, call
from devcoordinator2.paths import load_instance_config

SUPPORTED_PROTOCOL_VERSIONS = ("2025-06-18", "2025-03-26", "2024-11-05")

_PATH = {"type": "string",
         "description": "Absolute path inside the target Git worktree"}

TOOLS = [
    {
        "name": "test_start",
        "description": (
            "Start the repository's test immediately (or supersede the "
            "current run for this worktree). Returns running only after the "
            "process exists; otherwise a terminal error. Never queues."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": _PATH,
                "test": {"type": "string",
                         "description": "Named test from .devcoordinator.toml "
                                        "(default: the declared default)"},
            },
            "required": ["path"],
        },
    },
    {
        "name": "test_status",
        "description": ("Current test run summary for a worktree: status, "
                        "timing, exit code, byte counts, and the summary file "
                        "path. Contains no log text."),
        "inputSchema": {"type": "object", "properties": {"path": _PATH},
                        "required": ["path"]},
    },
    {
        "name": "test_output",
        "description": ("Bounded tail of the current run's stdout or stderr "
                        "plus the full log file path."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": _PATH,
                "stream": {"type": "string", "enum": ["stdout", "stderr"]},
                "tail_bytes": {"type": "integer", "minimum": 1,
                               "maximum": 65536, "default": 16384},
            },
            "required": ["path", "stream"],
        },
    },
    {
        "name": "test_stop",
        "description": "Cancel the current test run and prove cleanup.",
        "inputSchema": {"type": "object", "properties": {"path": _PATH},
                        "required": ["path"]},
    },
    {
        "name": "repository_list",
        "description": "All registered repositories and their worktrees.",
        "inputSchema": {"type": "object", "properties": {}},
    },
]

_DEP_REF = {
    "path": _PATH,
    "name": {"type": "string", "description": "Deployment name, or name@source when the "
                                              "declaration enables both sources"},
    "deployment_id": {"type": "string", "description": "Exact deployment identity"},
}
TOOLS += [
    {"name": "deployment_list", "description": "All deployments; with a path, also the "
                                               "declared-but-not-applied ones.",
     "inputSchema": {"type": "object", "properties": {"path": _PATH}}},
    {"name": "deployment_apply",
     "description": "Apply the declared specification immediately: prepare the candidate, "
                    "start components in order, prove health, switch the route, retire the "
                    "previous generation. Concurrent mutation returns busy.",
     "inputSchema": {"type": "object", "properties": _DEP_REF, "required": ["path"]}},
    {"name": "deployment_status", "description": "Live per-component state, health, "
                                                 "bindings, ports, and generation.",
     "inputSchema": {"type": "object", "properties": _DEP_REF, "required": ["path"]}},
    {"name": "deployment_start", "description": "Start a deployment or one component.",
     "inputSchema": {"type": "object", "properties": {**_DEP_REF, "component":
                     {"type": "string"}}, "required": ["path"]}},
    {"name": "deployment_stop", "description": "Stop a deployment or one component "
                                               "(never deletes data).",
     "inputSchema": {"type": "object", "properties": {**_DEP_REF, "component":
                     {"type": "string"}}, "required": ["path"]}},
    {"name": "deployment_restart", "description": "Restart a deployment or one component.",
     "inputSchema": {"type": "object", "properties": {**_DEP_REF, "component":
                     {"type": "string"}}, "required": ["path"]}},
    {"name": "deployment_logs", "description": "Bounded tail of one component's logs.",
     "inputSchema": {"type": "object", "properties": {**_DEP_REF, "component":
                     {"type": "string"}, "tail_lines": {"type": "integer", "minimum": 1,
                                                        "maximum": 5000}},
                     "required": ["path", "component"]}},
    {"name": "deployment_rollback", "description": "Return a checkout deployment to its "
                                                   "previous generation.",
     "inputSchema": {"type": "object", "properties": _DEP_REF, "required": ["path"]}},
    {"name": "deployment_set_domain",
     "description": "Set, change, or clear the routed domain of a deployment "
                    "(administrator). For observed deployments without a route, pass "
                    "port (and component when several containers exist).",
     "inputSchema": {"type": "object", "properties": {
         "deployment_id": _DEP_REF["deployment_id"],
         "domain": {"type": ["string", "null"],
                    "description": "Lowercase DNS label; null clears the domain"},
         "port": {"type": "integer", "minimum": 1, "maximum": 65535},
         "component": {"type": "string"}, "public": {"type": "boolean"}},
      "required": ["deployment_id"]}},
    {"name": "health_containers", "description": "Every container on the host with "
                                                 "ownership classification.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "health_summary", "description": "Host CPU/memory/disk/load, unhealthy "
                                              "deployments, active tests, container counts "
                                              "by ownership, current alerts.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "health_repositories", "description": "Per-repository CPU, memory, storage, "
                                                   "health and compact trends, reconciled "
                                                   "against DevCoordinator and shared usage.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "health_repository", "description": "Every measured component of one "
                                                 "repository with live usage and storage.",
     "inputSchema": {"type": "object", "properties": {"path": _PATH}, "required": ["path"]}},
    {"name": "bug_report", "description": "Open a bounded atomic bug record (or count a "
                                          "recurrence). Independent of the daemon. No secrets, "
                                          "raw logs, or private paths.",
     "inputSchema": {"type": "object", "properties": {
         "component": {"type": "string"}, "summary": {"type": "string"},
         "expected": {"type": "string"}, "actual": {"type": "string"},
         "steps": {"type": "string"},
         "correlations": {"type": "object", "properties": {
             "run_id": {"type": "string"}, "deployment_id": {"type": "string"},
             "repository_id": {"type": "string"}}}},
      "required": ["component", "summary", "expected", "actual", "steps"]}},
    {"name": "bug_list", "description": "All currently open bugs.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "bug_close", "description": "Close (remove) an open bug by id.",
     "inputSchema": {"type": "object", "properties": {"bug_id": {"type": "string"}},
                     "required": ["bug_id"]}},
]

_TOOL_TO_COMMAND = {
    "deployment_list": "deployment.list", "deployment_apply": "deployment.apply",
    "deployment_status": "deployment.status", "deployment_start": "deployment.start",
    "deployment_stop": "deployment.stop", "deployment_restart": "deployment.restart",
    "deployment_logs": "deployment.logs", "deployment_rollback": "deployment.rollback",
    "deployment_set_domain": "deployment.set_domain",
    "health_containers": "health.containers",
    "health_summary": "health.summary", "health_repositories": "health.repositories",
    "health_repository": "health.repository",
    "bug_report": "bug.report", "bug_list": "bug.list", "bug_close": "bug.close",
    "test_start": "test.start",
    "test_status": "test.status",
    "test_output": "test.output",
    "test_stop": "test.stop",
    "test_list": "test.list",
    "repository_list": "repository.list",
}


class McpServer:
    def __init__(self, stdin=None, stdout=None):
        self._in = stdin or sys.stdin
        self._out = stdout or sys.stdout
        self._config = load_instance_config()
        self._client_kind = "other"

    def run(self) -> int:
        for line in self._in:
            line = line.strip()
            if not line:
                continue
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                self._send_error(None, -32700, "parse error")
                continue
            self._dispatch(message)
        return 0

    def _dispatch(self, message: dict[str, Any]) -> None:
        method = message.get("method")
        msg_id = message.get("id")
        if method is None:
            return  # response to a server request; none are sent
        if method == "initialize":
            self._handle_initialize(msg_id, message.get("params") or {})
        elif method == "notifications/initialized":
            pass
        elif method == "ping":
            self._send_result(msg_id, {})
        elif method == "tools/list":
            self._send_result(msg_id, {"tools": TOOLS})
        elif method == "tools/call":
            self._handle_tools_call(msg_id, message.get("params") or {})
        elif method.startswith("notifications/"):
            pass
        elif msg_id is not None:
            self._send_error(msg_id, -32601, f"method not found: {method}")

    def _handle_initialize(self, msg_id, params: dict[str, Any]) -> None:
        requested = params.get("protocolVersion")
        version = requested if requested in SUPPORTED_PROTOCOL_VERSIONS \
            else SUPPORTED_PROTOCOL_VERSIONS[0]
        client_name = str((params.get("clientInfo") or {}).get("name", "")).lower()
        for kind in ("codex", "claude", "cursor", "antigravity"):
            if kind in client_name:
                self._client_kind = kind
                break
        self._send_result(msg_id, {
            "protocolVersion": version,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "devcoordinator2", "version": __version__},
        })

    def _handle_tools_call(self, msg_id, params: dict[str, Any]) -> None:
        name = params.get("name")
        command = _TOOL_TO_COMMAND.get(name)
        if command is None:
            self._send_error(msg_id, -32602, f"unknown tool: {name}")
            return
        arguments = params.get("arguments") or {}
        if name.startswith("bug_"):
            response = self._bug_tool(name, arguments)
        else:
            try:
                response = call(self._config.socket_path, command, arguments,
                                client_kind=self._client_kind)
            except DaemonUnavailable as exc:
                response = {"ok": False, "error": {"code": "daemon_unavailable",
                                                   "message": str(exc)}}
        self._send_result(msg_id, {
            "content": [{"type": "text",
                         "text": json.dumps(response, indent=2)}],
            "isError": not response.get("ok", False),
        })

    def _bug_tool(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        """Bugs go to the independent store directly (daemon-outage safe),
        then the daemon is notified best-effort."""
        import os

        from devcoordinator2 import bugs
        try:
            if name == "bug_report":
                result = bugs.report(reporter=f"uid:{os.getuid()}",
                                     directory=self._config.bugs_dir, **arguments)
            elif name == "bug_list":
                result = {"bugs": bugs.list_open(self._config.bugs_dir)}
            else:
                result = bugs.close(str(arguments.get("bug_id", "")), self._config.bugs_dir)
        except (bugs.BugError, TypeError) as exc:
            return {"ok": False, "error": {"code": "args_invalid", "message": str(exc)}}
        if name != "bug_list":
            try:
                call(self._config.socket_path, _TOOL_TO_COMMAND[name], arguments,
                     client_kind=self._client_kind)
            except DaemonUnavailable:
                result["notified"] = False
        return {"ok": True, "result": result}

    def _send_result(self, msg_id, result: dict[str, Any]) -> None:
        self._write({"jsonrpc": "2.0", "id": msg_id, "result": result})

    def _send_error(self, msg_id, code: int, message: str) -> None:
        self._write({"jsonrpc": "2.0", "id": msg_id,
                     "error": {"code": code, "message": message}})

    def _write(self, payload: dict[str, Any]) -> None:
        self._out.write(json.dumps(payload, separators=(",", ":")) + "\n")
        self._out.flush()


def main() -> int:
    return McpServer().run()


if __name__ == "__main__":
    sys.exit(main())
