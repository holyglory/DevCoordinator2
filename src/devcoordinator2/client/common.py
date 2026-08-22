"""Client-side socket call: one request, one response, verbatim JSON."""

from __future__ import annotations

import json
import socket
import uuid
from pathlib import Path
from typing import Any

from devcoordinator2 import protocol

CONNECT_TIMEOUT = 5.0
RESPONSE_TIMEOUT = 30.0


class DaemonUnavailable(Exception):
    pass


def call(socket_path: Path, command: str, args: dict[str, Any],
         client_kind: str = "other",
         client_session: str | None = None) -> dict[str, Any]:
    request = {
        "protocol": protocol.PROTOCOL_VERSION,
        "id": uuid.uuid4().hex[:12],
        "command": command,
        "args": args,
        "client": {"kind": client_kind, "session": client_session},
    }
    raw = (json.dumps(request, separators=(",", ":")) + "\n").encode()
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(CONNECT_TIMEOUT)
            sock.connect(str(socket_path))
            sock.sendall(raw)
            sock.shutdown(socket.SHUT_WR)
            sock.settimeout(RESPONSE_TIMEOUT)
            data = b""
            while len(data) <= protocol.MAX_RESPONSE_BYTES:
                part = sock.recv(65536)
                if not part:
                    break
                data += part
    except OSError as exc:
        raise DaemonUnavailable(
            f"cannot reach daemon at {socket_path}: {exc}") from exc
    if not data:
        raise DaemonUnavailable(f"empty response from daemon at {socket_path}")
    return json.loads(data)
