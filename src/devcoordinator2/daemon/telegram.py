"""One server-owned Telegram bot: chat linking, subscriptions, event routing,
and a small bounded durable outbox (delivery reliability, not a queue).

The bot token lives in a private instance file and never enters the
database, logs, or results. Subscribers get only events for scopes they may
view; administrators may subscribe to server-wide events."""

from __future__ import annotations

import json
import logging
import secrets
import threading
import urllib.error
import urllib.parse
import urllib.request
from datetime import UTC, datetime, timedelta

from devcoordinator2.daemon import events
from devcoordinator2.daemon.db import Database
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

log = logging.getLogger("devcoordinator2.telegram")
OUTBOX_MAX_ROWS = 1000
OUTBOX_MAX_ATTEMPTS = 10
OUTBOX_MAX_AGE = timedelta(hours=24)
LINK_CODE_TTL = timedelta(minutes=15)
SCOPE_SERVER = "server"


def _now() -> datetime:
    return datetime.now(UTC)


def _iso(ts: datetime) -> str:
    return ts.strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_scope(scope: str) -> tuple[str, str | None]:
    if scope == SCOPE_SERVER:
        return "server", None
    kind, _, ident = scope.partition(":")
    if kind in ("deployment", "repository") and ident:
        return kind, ident
    raise ProtocolError("args_invalid", "scope must be server, deployment:<id>, or "
                                        "repository:<id>")


class Telegram:
    def __init__(self, config: InstanceConfig, db: Database):
        self._config = config
        self._db = db
        self._token = self._read_token()
        self._stop = threading.Event()
        self._offset = 0
        self.last_poll_at: str | None = None
        self.last_error: str | None = None
        events.subscribe(self.on_event)

    # -- configuration ----------------------------------------------------------

    def _read_token(self) -> str | None:
        path = self._config.telegram_token_file
        if path is None:
            return None
        try:
            token = path.read_text().strip()
        except OSError as exc:
            log.warning("telegram token unreadable: %s", exc)
            return None
        return token or None

    @property
    def configured(self) -> bool:
        return self._token is not None

    def _api(self, method: str, payload: dict, timeout: int = 35) -> dict:
        url = f"{self._config.telegram_api}/bot{self._token}/{method}"
        data = json.dumps(payload).encode()
        req = urllib.request.Request(url, data=data,
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = json.loads(resp.read())
        if not body.get("ok"):
            raise RuntimeError(str(body.get("description", "telegram error"))[:200])
        return body.get("result", {})

    # -- threads -------------------------------------------------------------

    def start(self) -> None:
        if not self.configured:
            log.info("telegram not configured; notifications disabled")
            return
        threading.Thread(target=self._poll_loop, daemon=True, name="telegram-poll").start()
        threading.Thread(target=self._deliver_loop, daemon=True,
                         name="telegram-deliver").start()

    def stop(self) -> None:
        self._stop.set()

    def _poll_loop(self) -> None:
        while not self._stop.is_set():
            try:
                self.poll_once()
            except Exception as exc:  # isolated: never affects mutations
                self.last_error = str(exc)[:200]
                log.warning("telegram poll failed: %s", exc)
                self._stop.wait(10)

    def poll_once(self) -> None:
        updates = self._api("getUpdates", {"timeout": 25, "offset": self._offset,
                                           "allowed_updates": ["message"]})
        self.last_poll_at = _iso(_now())
        for update in updates:
            self._offset = max(self._offset, int(update.get("update_id", 0)) + 1)
            message = update.get("message") or {}
            chat = message.get("chat") or {}
            text = str(message.get("text") or "").strip()
            if not chat.get("id") or not text:
                continue
            self._handle_message(int(chat["id"]), text, chat)

    def _handle_message(self, chat_id: int, text: str, chat: dict) -> None:
        label = " ".join(filter(None, [chat.get("first_name"), chat.get("username")]))[:64]
        if text.startswith("/start"):
            code = self._issue_link_code(chat_id, label)
            self.enqueue(chat_id, f"DevCoordinator2: link code {code} (valid 15 minutes)."
                                  " An administrator links it to your account.")
        elif text.startswith("/stop"):
            with self._db.transaction() as conn:
                conn.execute("DELETE FROM telegram_subscriptions WHERE chat_id=?", (chat_id,))
                conn.execute("DELETE FROM telegram_chats WHERE chat_id=?", (chat_id,))
            self.enqueue(chat_id, "DevCoordinator2: unlinked; no more notifications.")
        else:
            self.enqueue(chat_id, "DevCoordinator2 notifies only. /start to link, /stop to"
                                  " unlink.")

    def _deliver_loop(self) -> None:
        while not self._stop.wait(2):
            try:
                self.deliver_once()
            except Exception as exc:
                log.warning("telegram delivery failed: %s", exc)

    # -- outbox --------------------------------------------------------------

    def enqueue(self, chat_id: int, text: str) -> None:
        now = _now()
        with self._db.transaction() as conn:
            conn.execute(
                "INSERT INTO telegram_outbox(chat_id, text, created_at, attempts,"
                " next_attempt_at) VALUES(?,?,?,0,?)",
                (chat_id, text[:4000], _iso(now), _iso(now)))
            count = conn.execute("SELECT count(*) AS n FROM telegram_outbox").fetchone()["n"]
            if count > OUTBOX_MAX_ROWS:
                conn.execute(
                    "DELETE FROM telegram_outbox WHERE message_id IN (SELECT message_id FROM"
                    " telegram_outbox ORDER BY message_id LIMIT ?)", (count - OUTBOX_MAX_ROWS,))

    def deliver_once(self) -> int:
        now = _now()
        cutoff = _iso(now - OUTBOX_MAX_AGE)
        with self._db.transaction() as conn:
            conn.execute("DELETE FROM telegram_outbox WHERE created_at < ? OR attempts >= ?",
                         (cutoff, OUTBOX_MAX_ATTEMPTS))
        rows = self._db.query(
            "SELECT * FROM telegram_outbox WHERE next_attempt_at <= ? ORDER BY message_id"
            " LIMIT 20", (_iso(now),))
        delivered = 0
        for row in rows:
            try:
                self._api("sendMessage", {"chat_id": row["chat_id"], "text": row["text"]},
                          timeout=15)
                with self._db.transaction() as conn:
                    conn.execute("DELETE FROM telegram_outbox WHERE message_id=?",
                                 (row["message_id"],))
                delivered += 1
            except (urllib.error.URLError, RuntimeError, OSError, ValueError) as exc:
                attempts = row["attempts"] + 1
                delay = min(2 ** attempts, 300)
                with self._db.transaction() as conn:
                    conn.execute(
                        "UPDATE telegram_outbox SET attempts=?, next_attempt_at=?, last_error=?"
                        " WHERE message_id=?",
                        (attempts, _iso(now + timedelta(seconds=delay)), str(exc)[:200],
                         row["message_id"]))
        return delivered

    # -- linking and subscriptions ------------------------------------------

    def _issue_link_code(self, chat_id: int, label: str) -> str:
        code = secrets.token_hex(3).upper()
        with self._db.transaction() as conn:
            conn.execute("DELETE FROM telegram_links WHERE chat_id=? OR expires_at<?",
                         (chat_id, _iso(_now())))
            conn.execute(
                "INSERT INTO telegram_links(code, chat_id, label, created_at, expires_at)"
                " VALUES(?,?,?,?,?)", (code, chat_id, label, _iso(_now()),
                                       _iso(_now() + LINK_CODE_TTL)))
        return code

    def link(self, code: str, email: str) -> dict:
        rows = self._db.query("SELECT * FROM telegram_links WHERE code=? AND expires_at>=?",
                              (str(code).upper(), _iso(_now())))
        if not rows:
            raise ProtocolError("args_invalid", "unknown or expired link code")
        row = rows[0]
        with self._db.transaction() as conn:
            conn.execute(
                "INSERT OR REPLACE INTO telegram_chats(chat_id, email, label, linked_at)"
                " VALUES(?,?,?,?)", (row["chat_id"], email, row["label"], _iso(_now())))
            conn.execute("DELETE FROM telegram_links WHERE code=?", (row["code"],))
        self.enqueue(row["chat_id"], f"DevCoordinator2: linked to {email}.")
        return {"chat_id": row["chat_id"], "email": email}

    def subscribe(self, chat_id: int, scope: str) -> dict:
        parse_scope(scope)
        if not self._db.query("SELECT 1 FROM telegram_chats WHERE chat_id=?", (chat_id,)):
            raise ProtocolError("args_invalid", "chat is not linked")
        with self._db.transaction() as conn:
            conn.execute(
                "INSERT OR IGNORE INTO telegram_subscriptions(chat_id, scope, created_at)"
                " VALUES(?,?,?)", (chat_id, scope, _iso(_now())))
        return {"chat_id": chat_id, "scope": scope}

    def unsubscribe(self, chat_id: int, scope: str) -> dict:
        with self._db.transaction() as conn:
            removed = conn.execute("DELETE FROM telegram_subscriptions WHERE chat_id=?"
                                   " AND scope=?", (chat_id, scope)).rowcount
        return {"chat_id": chat_id, "scope": scope, "removed": bool(removed)}

    def listing(self, email: str | None = None) -> dict:
        chats = [dict(r) for r in self._db.query("SELECT * FROM telegram_chats ORDER BY email")]
        if email is not None:
            chats = [c for c in chats if c["email"] == email]
        subs = [dict(r) for r in self._db.query("SELECT * FROM telegram_subscriptions")]
        for c in chats:
            c["subscriptions"] = sorted(s["scope"] for s in subs
                                        if s["chat_id"] == c["chat_id"])
        outbox = self._db.query("SELECT count(*) AS n FROM telegram_outbox")[0]["n"]
        return {"configured": self.configured, "chats": chats, "outbox_pending": outbox,
                "last_poll_at": self.last_poll_at, "last_error": self.last_error}

    def chat_email(self, chat_id: int) -> str | None:
        rows = self._db.query("SELECT email FROM telegram_chats WHERE chat_id=?", (chat_id,))
        return rows[0]["email"] if rows else None

    # -- event routing -------------------------------------------------------

    def on_event(self, event: dict) -> None:
        if not self.configured:
            return
        scopes, text = route(event)
        if not scopes or not text:
            return
        placeholders = ",".join("?" * len(scopes))
        rows = self._db.query(
            "SELECT DISTINCT chat_id FROM telegram_subscriptions WHERE scope IN"
            f" ({placeholders})", tuple(scopes))
        for row in rows:
            self.enqueue(row["chat_id"], text)


def route(event: dict) -> tuple[list[str], str | None]:
    """Map an event to subscription scopes and a short message. Successful
    tests are silent; secrets never appear because events never carry any."""
    kind = event["kind"]
    scopes: list[str] = []
    dep = event.get("deployment_id")
    repo = event.get("repository_id")
    if dep:
        scopes.append(f"deployment:{dep}")
    if repo:
        scopes.append(f"repository:{repo}")
    name = f"{event.get('name', '')}@{event.get('source', '')}".strip("@")
    if kind == "deployment.applied":
        return scopes, f"deployed {name} generation {event.get('generation')}"
    if kind == "deployment.failed":
        return scopes, f"deployment {name} FAILED: {event.get('message', '')}"
    if kind == "deployment.rolled_back":
        return scopes, f"rolled back {name} to generation {event.get('generation')}"
    if kind in ("deployment.stop", "deployment.start", "deployment.restart"):
        comp = f" ({event['component']})" if event.get("component") else ""
        return scopes, f"{kind.split('.')[1]} {name}{comp}: now {event.get('state')}"
    if kind == "deployment.removed":
        return scopes, f"removed {name} (data deleted: {event.get('data_deleted')})"
    if kind == "component.failed":
        return scopes, f"component {event.get('component')} failed: {event.get('message', '')}"
    if kind == "preview.expired":
        return scopes, f"preview {name} expired and was stopped"
    if kind == "release.requested":
        return [SCOPE_SERVER, *scopes], (f"preview requested for"
                                         f" {event.get('repository_name', '')}:"
                                         f" {event.get('name', '')}")
    if kind == "release.delivered":
        where = event.get("url") or (f"server port {event['port']}"
                                     if event.get("port") else "no route yet")
        draft = " (work in progress)" if event.get("dirty") else ""
        return [SCOPE_SERVER, *scopes], (f"preview delivered{draft}:"
                                         f" {event.get('name', '')} — {where}")
    if kind == "test.finished":
        status = event.get("status")
        if status in ("failed", "timed-out", "superseded", "interrupted"):
            return scopes, (f"test {event.get('test')} {status}"
                            f" (exit {event.get('exit_code')})")
        return [], None
    if kind == "test.cleanup_failed":
        return scopes, f"test cleanup failed: {event.get('message', '')}"
    if kind in ("alert.opened", "alert.recovered"):
        sk, sid = event.get("subject_kind"), str(event.get("subject_id", ""))
        if sk == "component" and "/" in sid:
            scopes = [f"deployment:{sid.split('/', 1)[0]}"]
        else:
            scopes = [SCOPE_SERVER]
        prefix = "ALERT" if kind == "alert.opened" else "recovered"
        return scopes, f"{prefix} [{event.get('alert_kind')}]: {event.get('message', '')}"
    if kind in ("container.unmanaged_seen", "container.orphaned_seen"):
        return [SCOPE_SERVER], (f"new {kind.split('.')[1].replace('_seen', '')} container "
                                f"{event.get('name')} ({event.get('image')})")
    if kind in ("coordinator.started",):
        return [SCOPE_SERVER], "DevCoordinator2 daemon started"
    if kind in ("bug.opened", "bug.closed"):
        return [SCOPE_SERVER, *scopes], (f"bug {kind.split('.')[1]}: [{event.get('component')}]"
                                         f" {event.get('summary')}")
    if kind in ("user.invited", "user.removed", "grant.set", "grant.removed"):
        return [SCOPE_SERVER], f"{kind}: {event.get('email')}"
    return [], None
