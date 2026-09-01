"""Shared wire protocol: envelope, limits, error model (docs/protocol.md)."""

from __future__ import annotations

import json
from typing import Any

PROTOCOL_VERSION = 1
MAX_REQUEST_BYTES = 65536
MAX_RESPONSE_BYTES = 262144
MAX_ERROR_DETAIL_BYTES = 4096
READ_TIMEOUT_SECONDS = 5.0
WRITE_TIMEOUT_SECONDS = 10.0

CLIENT_KINDS = ("codex", "claude", "cursor", "antigravity", "human", "other", "edge")

ERROR_CODES = (
    "protocol_invalid",
    "request_too_large",
    "command_unknown",
    "args_invalid",
    "repository_not_found",
    "repository_archived",
    "repository_archive_blocked",
    "repository_config_invalid",
    "worktree_busy",
    "test_not_found",
    "test_start_failed",
    "tests_draining",
    "unit_stop_failed",
    "deployment_not_found",
    "busy",
    "deployment_apply_failed",
    "deployment_action_failed",
    "observed_only",
    "rollback_unavailable",
    "permission_denied",
    "user_not_found",
    "task_not_found",
    "release_not_found",
    "decision_not_found",
    "internal_error",
)


class ProtocolError(Exception):
    def __init__(self, code: str, message: str, detail: str = ""):
        assert code in ERROR_CODES, code
        super().__init__(message)
        self.code = code
        self.message = message
        self.detail = detail[:MAX_ERROR_DETAIL_BYTES]


def parse_request(raw: bytes) -> dict[str, Any]:
    """Validate the envelope; command args are validated by their handler."""
    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ProtocolError("protocol_invalid", "request is not valid JSON",
                            str(exc)) from exc
    if not isinstance(data, dict):
        raise ProtocolError("protocol_invalid", "request must be a JSON object")
    if data.get("protocol") != PROTOCOL_VERSION:
        raise ProtocolError("protocol_invalid",
                            f"unsupported protocol version {data.get('protocol')!r}")
    allowed = {"protocol", "id", "command", "args", "client"}
    unknown = set(data) - allowed
    if unknown:
        raise ProtocolError("protocol_invalid",
                            f"unknown envelope fields: {sorted(unknown)}")
    if not isinstance(data.get("id"), str) or not data["id"]:
        raise ProtocolError("protocol_invalid", "missing request id")
    if not isinstance(data.get("command"), str) or not data["command"]:
        raise ProtocolError("protocol_invalid", "missing command")
    args = data.get("args", {})
    if not isinstance(args, dict):
        raise ProtocolError("protocol_invalid", "args must be an object")
    client = data.get("client", {})
    if not isinstance(client, dict):
        raise ProtocolError("protocol_invalid", "client must be an object")
    kind = client.get("kind", "other")
    if kind not in CLIENT_KINDS:
        kind = "other"
    session = client.get("session")
    if session is not None and not isinstance(session, str):
        session = None
    identity = client.get("identity")
    if identity is not None and (not isinstance(identity, str) or "@" not in identity
                                 or len(identity) > 254):
        raise ProtocolError("protocol_invalid", "client.identity must be an e-mail")
    data["args"] = args
    data["client"] = {"kind": kind, "session": session,
                      "identity": identity.lower() if identity else None}
    return data


def success_response(request_id: str, result: dict[str, Any]) -> bytes:
    return _encode({"protocol": PROTOCOL_VERSION, "id": request_id,
                    "ok": True, "result": result})


def error_response(request_id: str, error: ProtocolError) -> bytes:
    return _encode({
        "protocol": PROTOCOL_VERSION, "id": request_id, "ok": False,
        "error": {"code": error.code, "message": error.message,
                  "detail": error.detail},
    })


def _encode(payload: dict[str, Any]) -> bytes:
    raw = (json.dumps(payload, separators=(",", ":"), ensure_ascii=False) + "\n").encode()
    if len(raw) > MAX_RESPONSE_BYTES:
        fallback = {
            "protocol": PROTOCOL_VERSION, "id": payload.get("id", ""), "ok": False,
            "error": {"code": "internal_error",
                      "message": "response exceeded size cap", "detail": ""},
        }
        raw = (json.dumps(fallback, separators=(",", ":")) + "\n").encode()
    return raw
