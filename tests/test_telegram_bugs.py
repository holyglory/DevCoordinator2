import json
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import ClassVar

import pytest

from devcoordinator2 import bugs
from devcoordinator2.daemon import events, telegram
from devcoordinator2.daemon.db import Database
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError


class FakeTelegram(BaseHTTPRequestHandler):
    updates: ClassVar[list] = []
    sent: ClassVar[list] = []
    fail_sends = 0

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0)) or b"{}"))
        method = self.path.rsplit("/", 1)[-1]
        if method == "getUpdates":
            offset = body.get("offset", 0)
            result = [u for u in FakeTelegram.updates if u["update_id"] >= offset]
            payload = {"ok": True, "result": result}
        elif method == "sendMessage":
            if FakeTelegram.fail_sends > 0:
                FakeTelegram.fail_sends -= 1
                payload = {"ok": False, "description": "simulated outage"}
            else:
                FakeTelegram.sent.append(body)
                payload = {"ok": True, "result": {"message_id": len(FakeTelegram.sent)}}
        else:
            payload = {"ok": False, "description": "unknown method"}
        data = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


@pytest.fixture
def tg(tmp_path: Path):
    server = HTTPServer(("127.0.0.1", 0), FakeTelegram)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    FakeTelegram.updates, FakeTelegram.sent, FakeTelegram.fail_sends = [], [], 0
    token = tmp_path / "token"
    token.write_text("123:SECRET-TOKEN\n")
    config = InstanceConfig(socket_path=tmp_path / "s", state_dir=tmp_path / "state",
                            unit_prefix="devcoordinator2-dev", slice_name="x.slice",
                            client_group="", telegram_token_file=token,
                            telegram_api=f"http://127.0.0.1:{server.server_port}")
    db = Database(config.database_path)
    bot = telegram.Telegram(config, db)
    yield type("T", (), {"bot": bot, "db": db, "config": config})
    server.shutdown()
    db.close()


def test_link_subscribe_route_and_outbox(tg):
    assert tg.bot.configured
    FakeTelegram.updates.append({"update_id": 7, "message": {"chat": {"id": 4242,
                                 "first_name": "Dev"}, "text": "/start"}})
    tg.bot.poll_once()
    assert tg.bot.deliver_once() == 1
    code = FakeTelegram.sent[-1]["text"].split("link code ")[1].split(" ")[0]
    assert len(code) == 6
    linked = tg.bot.link(code, "dev@example.test")
    assert linked["chat_id"] == 4242
    with pytest.raises(ProtocolError, match="unknown or expired"):
        tg.bot.link(code, "dev@example.test")
    tg.bot.subscribe(4242, "deployment:d1")
    with pytest.raises(ProtocolError, match="scope must be"):
        tg.bot.subscribe(4242, "weird")
    with pytest.raises(ProtocolError, match="not linked"):
        tg.bot.subscribe(9999, "server")
    # Event routing: a deployment failure reaches the subscriber; a passing
    # test is silent; unrelated deployments do not reach it.
    events.publish("deployment.failed", deployment_id="d1", name="web", source="worktree",
                   message="boom")
    events.publish("deployment.failed", deployment_id="d2", name="other", source="worktree",
                   message="boom")
    events.publish("test.finished", status="passed", repository_id="r1", test="unit")
    tg.bot.deliver_once()
    texts = [m["text"] for m in FakeTelegram.sent if m["chat_id"] == 4242]
    assert any("deployment web@worktree FAILED: boom" in t for t in texts)
    assert not any("other" in t for t in texts)
    assert not any("passed" in t for t in texts)
    # The token never appears in any stored or returned data.
    listing = tg.bot.listing()
    assert "SECRET-TOKEN" not in json.dumps(listing)
    assert listing["chats"][0]["subscriptions"] == ["deployment:d1"]
    # Outbox retry with backoff, bounded attempts.
    FakeTelegram.fail_sends = 2
    tg.bot.enqueue(4242, "retry me")
    assert tg.bot.deliver_once() == 0
    row = tg.db.query("SELECT attempts, next_attempt_at FROM telegram_outbox")[0]
    assert row["attempts"] == 1
    tg.bot.unsubscribe(4242, "deployment:d1")
    assert tg.bot.listing()["chats"][0]["subscriptions"] == []


def test_outbox_bounds(tg):
    for i in range(telegram.OUTBOX_MAX_ROWS + 5):
        tg.bot.enqueue(1, f"m{i}")
    assert tg.db.query("SELECT count(*) AS n FROM telegram_outbox")[0]["n"] == \
        telegram.OUTBOX_MAX_ROWS
    with tg.db.transaction() as conn:
        conn.execute("UPDATE telegram_outbox SET attempts=?", (telegram.OUTBOX_MAX_ATTEMPTS,))
    tg.bot.deliver_once()
    assert tg.db.query("SELECT count(*) AS n FROM telegram_outbox")[0]["n"] == 0


def test_route_table_covers_event_classes():
    cases = {
        "alert.opened": {"subject_kind": "component", "subject_id": "d1/api",
                         "alert_kind": "x", "message": "m"},
        "alert.recovered": {"subject_kind": "host", "subject_id": "host",
                            "alert_kind": "host_cpu", "message": "m"},
        "container.unmanaged_seen": {"name": "n", "image": "i"},
        "preview.expired": {"deployment_id": "d1", "name": "p", "source": "worktree"},
        "bug.opened": {"component": "c", "summary": "s", "repository_id": "r1"},
        "coordinator.started": {},
        "test.finished": {"status": "timed-out", "repository_id": "r1", "test": "t",
                          "exit_code": None},
    }
    for kind, fields in cases.items():
        scopes, text = telegram.route({"kind": kind, **fields})
        assert scopes and text, kind
    assert telegram.route({"kind": "alert.opened", "subject_kind": "component",
                           "subject_id": "d1/api", "alert_kind": "x", "message": "m"})[0] == \
        ["deployment:d1"]
    assert telegram.route({"kind": "test.finished", "status": "passed"}) == ([], None)


def test_bug_registry_independent_store(tmp_path: Path):
    store = tmp_path / "bugs"
    first = bugs.report(component="api", summary="500 on /x", expected="200", actual="500",
                        steps="GET /x", correlations={"deployment_id": "d1"},
                        reporter="uid:1000", directory=store)
    assert first["duplicate"] is False and first["occurrences"] == 1
    again = bugs.report(component="api", summary="500 on /x", expected="200", actual="500",
                        steps="GET /x", directory=store)
    assert again["duplicate"] is True and again["occurrences"] == 2
    assert len(bugs.list_open(store)) == 1
    with pytest.raises(bugs.BugError, match="secret"):
        bugs.report(component="api", summary="leak", expected="x", actual="password=abc",
                    steps="s", directory=store)
    with pytest.raises(bugs.BugError, match="private host path"):
        bugs.report(component="api", summary="p", expected="x", actual="see /home/someone/x",
                    steps="s", directory=store)
    with pytest.raises(bugs.BugError, match="exceeds"):
        bugs.report(component="api", summary="big", expected="x", actual="y" * 3000,
                    steps="s", directory=store)
    closed = bugs.close(first["bug_id"], store)
    assert closed["closed"] and bugs.list_open(store) == []
    with pytest.raises(bugs.BugError, match="no open bug"):
        bugs.close(first["bug_id"], store)
