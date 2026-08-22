"""Unix-socket server: accept loop, SO_PEERCRED, framing, dispatch."""

from __future__ import annotations

import grp
import logging
import os
import socket
import struct
import threading
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from devcoordinator2 import protocol
from devcoordinator2.protocol import ProtocolError

log = logging.getLogger("devcoordinator2.server")

_UCRED = struct.Struct("iII")  # pid, uid, gid


@dataclass(frozen=True)
class Caller:
    pid: int
    uid: int
    gid: int
    client_kind: str
    client_session: str | None


Handler = Callable[[dict[str, Any], Caller], dict[str, Any]]


def peer_credentials(conn: socket.socket) -> tuple[int, int, int]:
    data = conn.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, _UCRED.size)
    pid, uid, gid = _UCRED.unpack(data)
    return pid, uid, gid


class Server:
    def __init__(self, socket_path: Path, handlers: dict[str, Handler],
                 client_group: str | None = None):
        self._path = socket_path
        self._handlers = handlers
        self._client_group = client_group
        self._sock: socket.socket | None = None
        self._stop = threading.Event()
        self._threads: set[threading.Thread] = set()

    def bind(self) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True)
        if self._path.exists():
            self._path.unlink()
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.bind(str(self._path))
        os.chmod(self._path, 0o660)
        if self._client_group:
            try:
                gid = grp.getgrnam(self._client_group).gr_gid
                os.chown(self._path, -1, gid)
            except (KeyError, PermissionError):
                log.warning("client group %r not applied to socket", self._client_group)
        sock.listen(64)
        sock.settimeout(0.5)
        self._sock = sock

    def serve_forever(self) -> None:
        assert self._sock is not None, "bind() first"
        while not self._stop.is_set():
            try:
                conn, _ = self._sock.accept()
            except TimeoutError:
                continue
            except OSError:
                break
            thread = threading.Thread(
                target=self._serve_connection, args=(conn,), daemon=True
            )
            self._threads.add(thread)
            thread.start()
            self._threads = {t for t in self._threads if t.is_alive()}

    def shutdown(self) -> None:
        self._stop.set()
        if self._sock is not None:
            self._sock.close()
        for thread in list(self._threads):
            thread.join(timeout=2)

    def _serve_connection(self, conn: socket.socket) -> None:
        request_id = ""
        try:
            pid, uid, gid = peer_credentials(conn)
            conn.settimeout(protocol.READ_TIMEOUT_SECONDS)
            raw = self._read_frame(conn)
            request = protocol.parse_request(raw)
            request_id = request["id"]
            caller = Caller(
                pid=pid, uid=uid, gid=gid,
                client_kind=request["client"]["kind"],
                client_session=request["client"]["session"],
            )
            handler = self._handlers.get(request["command"])
            if handler is None:
                raise ProtocolError("command_unknown",
                                    f"unknown command {request['command']!r}")
            result = handler(request["args"], caller)
            response = protocol.success_response(request_id, result)
        except ProtocolError as exc:
            response = protocol.error_response(request_id, exc)
        except Exception as exc:  # never leak a traceback to the wire unbounded
            log.exception("handler failure")
            response = protocol.error_response(
                request_id,
                ProtocolError("internal_error", "unexpected daemon fault",
                              f"{type(exc).__name__}: {exc}"),
            )
        try:
            conn.settimeout(protocol.WRITE_TIMEOUT_SECONDS)
            conn.sendall(response)
        except OSError:
            pass  # lost reply: client re-queries; mutations remain observable
        finally:
            conn.close()

    @staticmethod
    def _read_frame(conn: socket.socket) -> bytes:
        chunks = bytearray()
        while True:
            chunk = conn.recv(8192)
            if not chunk:
                break
            chunks.extend(chunk)
            if len(chunks) > protocol.MAX_REQUEST_BYTES:
                raise ProtocolError("request_too_large",
                                    "request exceeds 64 KiB frame cap")
            if chunks.endswith(b"\n"):
                break
        if not chunks:
            raise ProtocolError("protocol_invalid", "empty request")
        return bytes(chunks)
