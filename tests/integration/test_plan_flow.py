"""Schema 8 end to end: plan a task tree, postpone a task, request a preview,
deliver it from a real dirty worktree deployment, and keep decisions."""

from __future__ import annotations

import subprocess

from integration.helpers import ROOT_ONLY, _call, _write_config

pytestmark = ROOT_ONLY

SERVER = '''import os
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"ok")
    def log_message(self, *a):
        pass
HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
'''

TOML = '''schema = 2
[deployment.app]
source = ["worktree"]
domain = { worktree = "planflow" }
components = ["api"]

[deployment.app.component.api]
type = "process"
command = ["python3", "server.py"]
port = true
route = true
health = { path = "/", timeout_seconds = 30 }
'''


def _git(world, *args):
    subprocess.run(["setpriv", f"--reuid={world.caller.pw_uid}",
                    f"--regid={world.caller.pw_gid}", "--init-groups", "--",
                    "git", *args], cwd=world.repo, check=True, capture_output=True,
                   env={"PATH": "/usr/bin:/bin", "HOME": str(world.base),
                        "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                        "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"})


def test_plan_ledger_preview_flow(world):
    repo = str(world.repo)
    (world.repo / "server.py").write_text(SERVER)
    _write_config(world.repo, world.caller, TOML)  # also chowns to the caller
    _git(world, "add", "-A")
    _git(world, "commit", "-qm", "app")
    commit = subprocess.run(
        ["setpriv", f"--reuid={world.caller.pw_uid}",
         f"--regid={world.caller.pw_gid}", "--init-groups", "--",
         "git", "rev-parse", "HEAD"], cwd=world.repo, capture_output=True,
        text=True, check=True,
        env={"PATH": "/usr/bin:/bin", "HOME": str(world.base)}).stdout.strip()

    # 1. Plan a small tree with two releases.
    first = _call(world, "release.create",
                  {"path": repo, "name": "First release", "kind": "release"})
    assert first["ok"], first
    second = _call(world, "release.create",
                   {"path": repo, "name": "Second release", "kind": "release"})
    parent = _call(world, "task.create", {
        "path": repo, "title": "People can see the app", "kind": "goal",
        "release_id": first["result"]["release_id"],
        "outcome": "Opening the app in a browser shows a working page."})
    assert parent["ok"], parent
    child = _call(world, "task.create", {
        "path": repo, "title": "A friendly start page", "kind": "goal",
        "parent_task_id": parent["result"]["task_id"],
        "release_id": first["result"]["release_id"], "estimated_loc": 120})
    stub = _call(world, "task.create", {
        "path": repo, "title": "The help page is still empty", "kind": "stub",
        "parent_task_id": parent["result"]["task_id"],
        "release_id": first["result"]["release_id"], "estimated_loc": 80,
        "impact": "Readers find nothing behind the Help link.",
        "technical_note": "help.html renders a bare template"})
    assert stub["ok"], stub

    # 2. Postpone the stub to the second release (an owner move).
    moved = _call(world, "task.update", {
        "task_id": stub["result"]["task_id"],
        "release_id": second["result"]["release_id"],
        "note": "Owner postponed the help page."})
    assert moved["ok"] and moved["result"]["release_id"] == \
        second["result"]["release_id"]

    # 3. The owner requests a preview ASAP; agents see it on their next write.
    requested = _call(world, "release.request",
                      {"path": repo, "note": "Show me the current state."})
    assert requested["ok"] and requested["result"]["status"] == "requested"
    overview = _call(world, "plan.overview", {"path": repo})["result"]
    assert overview["preview_requested"][0]["release_id"] == \
        requested["result"]["release_id"]
    touched = _call(world, "task.update", {"task_id": child["result"]["task_id"],
                                           "status": "in_progress"})
    assert touched["result"]["preview_requested"] is True

    # 4. Deploy the current DIRTY worktree for real.
    (world.repo / "NOTES.txt").write_text("work in progress\n")  # dirty on purpose
    applied = _call(world, "deployment.apply", {"path": repo, "name": "app"})
    assert applied["ok"], applied

    # 5. Deliver the preview and keep permanent reachable evidence.
    delivered = _call(world, "release.deliver", {
        "release_id": requested["result"]["release_id"],
        "deployment_id": applied["result"]["deployment_id"],
        "note": "First look for the owner."})
    assert delivered["ok"], delivered
    result = delivered["result"]
    assert result["status"] == "delivered" and result["dirty"] is True
    assert result["commit_hash"] == commit
    assert result["url"] == "https://planflow"  # no base domain in this world
    assert isinstance(result["port"], int)
    overview = _call(world, "plan.overview", {"path": repo})["result"]
    delivered_row = next(r for r in overview["releases"]
                         if r["release_id"] == requested["result"]["release_id"])
    assert delivered_row["status"] == "delivered" and delivered_row["url"]
    assert overview["preview_requested"] == []

    # 6. Decisions: record, tail, summarize, search.
    _call(world, "decision.record", {
        "path": repo, "aspect": "ui", "title": "The start page speaks plainly",
        "body": "The first page greets the reader instead of showing settings,"
                " because the owner wants a friendly first impression."})
    recorded = _call(world, "decision.record", {
        "path": repo, "aspect": "process", "title": "Previews come from live work",
        "body": "A requested preview deploys the current unfinished work so the"
                " owner can steer early.", "ref": "PLANFLOW-PREVIEWS"})
    assert recorded["ok"] and recorded["result"]["unsummarized_count"] == 2
    summarized = _call(world, "decision.summarize", {
        "path": repo, "covers_through_seq": 1,
        "body": "The story so far: the app greets people plainly."})
    assert summarized["ok"] and summarized["result"]["unsummarized_count"] == 1
    tail = _call(world, "decision.tail", {"path": repo, "n": 5})["result"]
    assert tail["summary"]["covers_through_seq"] == 1
    assert [d["seq"] for d in tail["decisions"]] == [1, 2]
    found = _call(world, "decision.search",
                  {"path": repo, "query": "PLANFLOW-PREVIEWS"})["result"]
    assert [d["seq"] for d in found["decisions"]] == [2]

    # 7. The permanent history tells the postponement story.
    history = _call(world, "task.history",
                    {"task_id": stub["result"]["task_id"]})["result"]
    assert [e["event"] for e in history["events"]] == ["created", "release_move"]
    assert history["events"][1]["note"] == "Owner postponed the help page."
    assert history["task"]["technical_note"] == "help.html renders a bare template"

    # Clean up the deployment's unit.
    removed = _call(world, "deployment.remove",
                    {"path": repo, "name": "app", "delete_data": True})
    assert removed["ok"], removed
