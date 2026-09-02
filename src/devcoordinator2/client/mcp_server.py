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
from devcoordinator2.operations import policy_for
from devcoordinator2.paths import load_instance_config

SUPPORTED_PROTOCOL_VERSIONS = ("2025-06-18", "2025-03-26", "2024-11-05")

_PATH = {"type": "string",
         "description": "Absolute path inside the target Git worktree"}
_LOG_SELECTOR = {
    "path": _PATH,
    "run_id": {"type": "string", "description": "Retained run; current when omitted"},
    "check": {"type": "string"},
    "phase": {"type": "string", "enum": ["executor", "check", "discovery", "case"]},
    "case": {"type": "string"},
    "stream": {"type": "string", "enum": ["stdout", "stderr"]},
    "cursor": {"type": "string", "maxLength": 4096},
}

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
                "checks": {"type": "array", "items": {"type": "string"},
                           "description": "Diagnostic check selection; omitted means "
                                          "the complete graph"},
                "tier": {"type": "string",
                         "enum": ["development", "pre-merge", "release"],
                         "default": "release"},
            },
            "required": ["path"],
        },
    },
    {
        "name": "test_retry",
        "description": ("Retry one failed check after its original complete run "
                        "finished. The result is non-readiness evidence."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": _PATH,
                "test": {"type": "string"},
                "run_id": {"type": "string"},
                "check": {"type": "string"},
            },
            "required": ["path", "run_id", "check"],
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
        "name": "test_log_catalog",
        "description": ("List retained check, case, and stream metadata without reading "
                        "log content. Catalogue before requesting a bounded slice."),
        "inputSchema": {"type": "object", "properties": {
            **_LOG_SELECTOR,
            "limit": {"type": "integer", "minimum": 1, "maximum": 100,
                      "default": 100},
        }, "required": ["path"], "additionalProperties": False},
    },
    {
        "name": "test_log_tail",
        "description": "Read a bounded final-line slice from one exact retained stream.",
        "inputSchema": {"type": "object", "properties": {
            **_LOG_SELECTOR,
            "lines": {"type": "integer", "minimum": 1, "maximum": 5000,
                      "default": 50},
            "max_bytes": {"type": "integer", "minimum": 1, "maximum": 49152,
                          "default": 32768},
        }, "required": ["path", "phase", "stream"],
           "additionalProperties": False},
    },
    {
        "name": "test_log_search",
        "description": ("Search one exact stream literally, with bounded matches and "
                        "stable line/byte coordinates. Input is never a regular expression."),
        "inputSchema": {"type": "object", "properties": {
            **_LOG_SELECTOR,
            "text": {"type": "string", "minLength": 1, "maxLength": 4096},
            "max_matches": {"type": "integer", "minimum": 1, "maximum": 100,
                            "default": 20},
            "context_lines": {"type": "integer", "minimum": 0, "maximum": 100,
                              "default": 2},
            "max_bytes": {"type": "integer", "minimum": 1, "maximum": 49152,
                          "default": 32768},
        }, "required": ["path", "phase", "stream", "text"],
           "additionalProperties": False},
    },
    {
        "name": "test_log_range",
        "description": ("Read one exact bounded line or byte interval. Supply both ends "
                        "of exactly one interval kind."),
        "inputSchema": {"type": "object", "properties": {
            **_LOG_SELECTOR,
            "line_start": {"type": "integer", "minimum": 1},
            "line_end": {"type": "integer", "minimum": 1},
            "byte_start": {"type": "integer", "minimum": 0},
            "byte_end": {"type": "integer", "minimum": 0},
            "max_bytes": {"type": "integer", "minimum": 1, "maximum": 49152,
                          "default": 49152},
        }, "required": ["path", "phase", "stream"],
           "additionalProperties": False},
    },
    {
        "name": "test_log_failure_context",
        "description": ("Return deterministically ranked, bounded failure excerpts with "
                        "stable coordinates; no model-generated summary."),
        "inputSchema": {"type": "object", "properties": {
            **_LOG_SELECTOR,
            "limit": {"type": "integer", "minimum": 1, "maximum": 100,
                      "default": 20},
            "context_lines": {"type": "integer", "minimum": 0, "maximum": 100,
                              "default": 2},
            "max_bytes": {"type": "integer", "minimum": 1, "maximum": 49152,
                          "default": 32768},
        }, "required": ["path"], "additionalProperties": False},
    },
    {
        "name": "test_log_retention_show",
        "description": "Show host-wide completed-log age and per-case history depth.",
        "inputSchema": {"type": "object", "properties": {},
                        "additionalProperties": False},
    },
    {
        "name": "test_log_retention_set",
        "description": ("Set both positive retention boundaries. Lowering either value "
                        "schedules immediate irreversible cleanup of eligible completed logs."),
        "inputSchema": {"type": "object", "properties": {
            "max_age_seconds": {"type": "integer", "minimum": 1,
                                "maximum": 315360000},
            "case_depth": {"type": "integer", "minimum": 1, "maximum": 65535},
        }, "required": ["max_age_seconds", "case_depth"],
           "additionalProperties": False},
    },
    {
        "name": "test_stop",
        "description": "Cancel the current test run and prove cleanup.",
        "inputSchema": {"type": "object", "properties": {
            "path": _PATH,
            "reason": {"type": "string", "minLength": 3, "maxLength": 256}},
                        "required": ["path"]},
    },
    {
        "name": "test_list",
        "description": "Current or most recent governed run per worktree.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "test_capacity_show",
        "description": "Show learned, effective, active, waiting, and paused test capacity.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "test_capacity_set",
        "description": "Set the administrator maximum parallel test-leaf count.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "cap": {"type": "integer", "minimum": 1, "maximum": 65535},
            },
            "required": ["cap"],
        },
    },
    {
        "name": "test_capacity_clear",
        "description": "Remove the administrator maximum and return to learned Auto capacity.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "repository_list",
        "description": "Active registered repositories and their worktrees.",
        "inputSchema": {"type": "object", "properties": {
            "include_archived": {"type": "boolean"},
        }},
    },
    {
        "name": "repository_archive",
        "description": (
            "Archive a repository after its open work and live resources are cleared."),
        "inputSchema": {"type": "object", "properties": {
            "repository_id": {"type": "string"},
            "merged_into_repository_id": {"type": "string"},
            "note": {"type": "string", "minLength": 3, "maxLength": 500},
        }, "required": ["repository_id", "merged_into_repository_id", "note"]},
    },
    {
        "name": "repository_unarchive",
        "description": "Restore an archived repository whose checkout exists.",
        "inputSchema": {"type": "object", "properties": {
            "repository_id": {"type": "string"},
            "note": {"type": "string", "minLength": 3, "maxLength": 500},
        }, "required": ["repository_id", "note"]},
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
    {"name": "deployment_status", "description": "Live per-component and declared "
                                                 "Compose-service state, health, bindings, "
                                                 "ports, completions, and generation.",
     "inputSchema": {"type": "object", "properties": _DEP_REF, "required": ["path"]}},
    {"name": "deployment_start", "description": "Start a deployment, one component, or "
                                                   "a reviewed component/service.",
     "inputSchema": {"type": "object", "properties": {**_DEP_REF, "component":
                     {"type": "string"}}, "required": ["path"]}},
    {"name": "deployment_stop", "description": "Stop a deployment, component, or reviewed "
                                               "component/service "
                                               "(never deletes data).",
     "inputSchema": {"type": "object", "properties": {**_DEP_REF, "component":
                     {"type": "string"}}, "required": ["path"]}},
    {"name": "deployment_restart", "description": "Restart a deployment, one component, "
                                                     "or a reviewed component/service.",
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

_TASK_KIND = {"type": "string",
              "enum": ["goal", "stub", "improvement", "user_feedback"],
              "description": "goal = planned work; stub = something fake/empty/"
                             "placeholder you just created or found; improvement = "
                             "something that can and should be better; "
                             "user_feedback = the owner asked for it"}
_ASPECT = {"type": "string",
           "enum": ["ui", "architecture", "algorithms", "business_logic", "data",
                    "testing", "deployment", "security", "performance", "process",
                    "other"]}
_PLAIN = ("Plain language a non-technical manager understands — no hashes, ids,"
          " file paths, or jargon (those go in technical_note).")
TOOLS += [
    {"name": "plan_overview",
     "description": ("The repository's plan: releases, the task tree (sized in "
                     "estimated lines of code), pending owner preview requests, "
                     "owner elaboration requests, and decision-summary state. "
                     "Check it before starting work. Honor preview_requested "
                     "promptly. If elaboration_requests is non-empty, read each "
                     "task, rewrite its title and/or outcome in plain everyday "
                     "language, and clear elaboration_needed in that same update."),
     "inputSchema": {"type": "object", "properties": {"path": _PATH},
                     "required": ["path"]}},
    {"name": "task_create",
     "description": ("Record a work item in the authoritative completion ledger. "
                     "MANDATORY the moment you stub, fake, or skip anything, and "
                     "for every improvement you notice. " + _PLAIN + " Size it in "
                     "estimated lines of code; split large work into subtask "
                     "trees via parent_task_id."),
     "inputSchema": {"type": "object", "properties": {
         "path": _PATH,
         "title": {"type": "string", "description": "One plain sentence, e.g. "
                                                    "'Painting the button red'"},
         "kind": _TASK_KIND,
         "outcome": {"type": "string", "description": "The remaining outcome in "
                                                      "plain language (defaults "
                                                      "to the title)"},
         "impact": {"type": "string", "description": "What users or the product "
                                                     "cannot do while this is open"},
         "unblock_condition": {"type": "string"},
         "verification": {"type": "string", "description": "Observable proof that "
                                                           "will close the gap"},
         "technical_note": {"type": "string", "description": "Agent-facing detail; "
                                                             "never shown as the "
                                                             "plain account"},
         "parent_task_id": {"type": "string"}, "release_id": {"type": "string"},
         "estimated_loc": {"type": "integer", "minimum": 1}},
      "required": ["path", "title", "kind"]}},
    {"name": "task_update",
     "description": ("Append-only task mutation: status change (planned/"
                     "in_progress/done/dropped), edits, new estimate, move to "
                     "another release (release_id; null = backlog), reparent, or "
                     "reorder (position, 0-based), or record/complete an owner "
                     "elaboration request. Every change lands in the permanent "
                     "event history. Every planning response can include "
                     "elaboration_requests; act on them before claiming the "
                     "related work complete."),
     "inputSchema": {"type": "object", "properties": {
         "task_id": {"type": "string"},
         "status": {"type": "string",
                    "enum": ["planned", "in_progress", "done", "dropped"]},
         "title": {"type": "string"}, "outcome": {"type": "string"},
         "impact": {"type": "string"}, "unblock_condition": {"type": "string"},
         "verification": {"type": "string"}, "technical_note": {"type": "string"},
         "estimated_loc": {"type": "integer", "minimum": 1},
         "release_id": {"type": ["string", "null"]},
         "parent_task_id": {"type": ["string", "null"]},
         "position": {"type": "integer", "minimum": 0},
         "elaboration_needed": {
             "type": "boolean",
             "description": ("true records the owner's request for clearer "
                             "wording. false completes it and is accepted only "
                             "with a changed title or outcome in this update")},
         "note": {"type": "string", "description": "Plain note stored on the "
                                                   "event"}},
      "required": ["task_id"]}},
    {"name": "task_history",
     "description": ("One task's full record plus its permanent event history "
                     "and all outstanding elaboration requests for its repository. "
                     "When this task needs elaboration, save clearer owner-facing "
                     "wording and clear the flag atomically."),
     "inputSchema": {"type": "object", "properties": {"task_id": {"type": "string"}},
                     "required": ["task_id"]}},
    {"name": "release_create",
     "description": ("Add a planned release or preliminary release (kind "
                     "'preview') to the repository's chart. " + _PLAIN),
     "inputSchema": {"type": "object", "properties": {
         "path": _PATH, "name": {"type": "string"},
         "kind": {"type": "string", "enum": ["preview", "release"]},
         "note": {"type": "string"}},
      "required": ["path", "name", "kind"]}},
    {"name": "release_deliver",
     "description": ("Mark a requested/planned release delivered by a REAL "
                     "deployment: apply the deployment first (dirty work is "
                     "fine), then call this with its deployment_id. Records "
                     "permanent evidence (commit, dirty flag, URL or host port) "
                     "and notifies the owner."),
     "inputSchema": {"type": "object", "properties": {
         "release_id": {"type": "string"}, "deployment_id": {"type": "string"},
         "note": {"type": "string"}},
      "required": ["release_id", "deployment_id"]}},
    {"name": "decision_record",
     "description": ("Record a consequential product decision in the "
                     "repository's permanent decision history. " + _PLAIN +
                     " body = what was decided, the options, and cost/risk in "
                     "user terms; supersedes = id or ref of the decision this "
                     "replaces."),
     "inputSchema": {"type": "object", "properties": {
         "path": _PATH, "aspect": _ASPECT, "title": {"type": "string"},
         "body": {"type": "string"}, "technical_note": {"type": "string"},
         "ref": {"type": "string", "description": "Optional stable citation key, "
                                                  "e.g. DC2-2026-08-24-TOPIC"},
         "supersedes": {"type": "string"}},
      "required": ["path", "aspect", "title", "body"]}},
    {"name": "decision_tail",
     "description": ("The rolling summary plus the last N decisions (optionally "
                     "one aspect). Load this instead of the full history. When "
                     "summary_due is true, write and store a new rolling summary "
                     "via decision_summarize before continuing."),
     "inputSchema": {"type": "object", "properties": {
         "path": _PATH, "aspect": _ASPECT,
         "n": {"type": "integer", "minimum": 1, "maximum": 50}},
      "required": ["path"]}},
    {"name": "decision_search",
     "description": ("Full-text search over every decision ever recorded "
                     "(titles, bodies, technical notes, refs). Search before "
                     "retrying an approach that may already have failed."),
     "inputSchema": {"type": "object", "properties": {
         "path": _PATH, "query": {"type": "string"}, "aspect": _ASPECT,
         "n": {"type": "integer", "minimum": 1, "maximum": 50}},
      "required": ["path", "query"]}},
    {"name": "decision_summarize",
     "description": ("Store the rolling summary you wrote, covering every "
                     "decision up to covers_through_seq. All summaries are kept; "
                     "the newest becomes 'the story so far'."),
     "inputSchema": {"type": "object", "properties": {
         "path": _PATH, "body": {"type": "string"},
         "covers_through_seq": {"type": "integer", "minimum": 1}},
      "required": ["path", "body", "covers_through_seq"]}},
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
    "test_retry": "test.retry",
    "test_status": "test.status",
    "test_log_catalog": "test.log.catalog",
    "test_log_tail": "test.log.tail",
    "test_log_search": "test.log.search",
    "test_log_range": "test.log.range",
    "test_log_failure_context": "test.log.failure_context",
    "test_log_retention_show": "test.log.retention.get",
    "test_log_retention_set": "test.log.retention.set",
    "test_stop": "test.stop",
    "test_list": "test.list",
    "test_capacity_show": "test.capacity.get",
    "test_capacity_set": "test.capacity.set",
    "test_capacity_clear": "test.capacity.set",
    "repository_list": "repository.list",
    "repository_archive": "repository.archive",
    "repository_unarchive": "repository.unarchive",
    # Planning/ledger/decisions (schema 8). release.request and release.update
    # are owner controls: Console + CLI only, deliberately not agent tools.
    "plan_overview": "plan.overview",
    "task_create": "task.create", "task_update": "task.update",
    "task_history": "task.history",
    "release_create": "release.create", "release_deliver": "release.deliver",
    "decision_record": "decision.record", "decision_tail": "decision.tail",
    "decision_search": "decision.search", "decision_summarize": "decision.summarize",
}

for _tool in TOOLS:
    _command = _TOOL_TO_COMMAND.get(_tool["name"])
    if _command is not None:
        _tool["annotations"] = policy_for(_command).mcp_annotations()


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
        if name == "test_capacity_clear":
            arguments = {"cap": None}
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
