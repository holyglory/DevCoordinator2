"""plan.* / task.* / release.* / decision.* handlers (schema 8).

Covers REQ-PLAN-01..10: append-only ledger with events, tree ordering,
release lifecycle with delivery evidence, decisions tail/search/summaries.
"""

from pathlib import Path

import pytest

from devcoordinator2.daemon import plan_state
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.deploy_state import delete_deployment_rows
from devcoordinator2.daemon.plan_api import build_plan_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

CALLER = Caller(pid=0, uid=1000, gid=1000, client_kind="other", client_session=None)


@pytest.fixture
def world(tmp_path: Path):
    config = InstanceConfig(socket_path=tmp_path / "s", state_dir=tmp_path / "state",
                            unit_prefix="devcoordinator2-dev", slice_name="x.slice",
                            client_group="", base_domain="example.test",
                            admin_emails=("owner@example.test",))
    db = Database(config.database_path)
    with db.transaction() as conn:
        conn.execute("INSERT INTO repositories VALUES('r1','/x','repo-one','t',1,'t')")
        conn.execute("INSERT INTO repositories VALUES('r2','/y','repo-two','t',1,'t')")
        conn.execute("INSERT INTO worktrees VALUES('w1','r1','/x','t','t')")
    handlers = build_plan_handlers(config, db, Registry(db))
    yield type("W", (), {"config": config, "db": db, "handlers": handlers})
    db.close()


def call(world, command, **args):
    return world.handlers[command](args, CALLER)


def events_for(world, subject_id):
    return [dict(r) for r in world.db.query(
        "SELECT * FROM plan_events WHERE subject_id=? ORDER BY event_id",
        (subject_id,))]


# -- tasks -------------------------------------------------------------------

def test_task_tree_create_positions_and_overview(world):
    release = call(world, "release.create", repository_id="r1",
                   name="First release", kind="release")
    parent = call(world, "task.create", repository_id="r1", title="Sign-in works",
                  kind="goal", release_id=release["release_id"],
                  outcome="A person can sign in with e-mail and password.")
    child1 = call(world, "task.create", repository_id="r1", title="Sign-in form",
                  kind="goal", parent_task_id=parent["task_id"],
                  release_id=release["release_id"], estimated_loc=300,
                  impact="Nobody can sign in yet." + "x" * 300)
    child2 = call(world, "task.create", repository_id="r1", title="Wrong password message",
                  kind="stub", parent_task_id=parent["task_id"],
                  release_id=release["release_id"], estimated_loc=200)
    backlog = call(world, "task.create", repository_id="r1",
                   title="Faster start-up", kind="improvement")
    assert (parent["seq"], child1["seq"], child2["seq"], backlog["seq"]) == (1, 2, 3, 4)
    assert child1["position"] == 1 and child2["position"] == 2
    assert parent["status"] == "planned" and parent["preview_requested"] is False
    assert parent["elaboration_needed"] is False

    overview = call(world, "plan.overview", repository_id="r1")
    assert overview["display_name"] == "repo-one"
    (rel,) = overview["releases"]
    # Aggregates count leaf tasks only; the parent is a summary row.
    assert (rel["tasks_total"], rel["loc_total"], rel["loc_done"]) == (2, 500, 0)
    tasks = {t["task_id"]: t for t in overview["tasks"]}
    assert tasks[backlog["task_id"]]["release_id"] is None
    assert tasks[child1["task_id"]]["impact"].endswith("…")  # bounded excerpt
    assert len(tasks[child1["task_id"]]["impact"]) <= plan_state.IMPACT_CLIP
    assert "outcome" not in tasks[parent["task_id"]]  # compact projection
    assert overview["tasks_truncated"] is False
    assert overview["elaboration_requests"] == []
    assert overview["decisions"] == {"unsummarized_count": 0, "summary_due": False}

    picker = call(world, "plan.overview")
    row = next(r for r in picker["repositories"] if r["repository_id"] == "r1")
    assert row["open_tasks"] == 3 and row["loc_total"] == 500
    assert row["elaboration_request_count"] == 0
    assert row["current_release"]["name"] == "First release"


def test_task_create_validations(world):
    release = call(world, "release.create", repository_id="r2",
                   name="Other repo release", kind="release")
    other = call(world, "task.create", repository_id="r2", title="Other repo task",
                 kind="goal")
    with pytest.raises(ProtocolError, match="one of"):
        call(world, "task.create", repository_id="r1", title="Valid title", kind="nope")
    with pytest.raises(ProtocolError, match="single-line"):
        call(world, "task.create", repository_id="r1", title="a\nb", kind="goal")
    with pytest.raises(ProtocolError, match="single-line"):
        call(world, "task.create", repository_id="r1", title="ab", kind="goal")
    with pytest.raises(ProtocolError, match="unknown args"):
        call(world, "task.create", repository_id="r1", title="Valid title",
             kind="goal", bogus=1)
    with pytest.raises(ProtocolError, match="no repository"):
        call(world, "task.create", repository_id="rmissing", title="Valid title",
             kind="goal")
    with pytest.raises(ProtocolError, match="another repository"):
        call(world, "task.create", repository_id="r1", title="Valid title",
             kind="goal", parent_task_id=other["task_id"])
    with pytest.raises(ProtocolError, match="another repository"):
        call(world, "task.create", repository_id="r1", title="Valid title",
             kind="goal", release_id=release["release_id"])
    with pytest.raises(ProtocolError, match="estimated_loc"):
        call(world, "task.create", repository_id="r1", title="Valid title",
             kind="goal", estimated_loc=0)
    # The plain outcome defaults to the title; technical_note never replaces it.
    created = call(world, "task.create", repository_id="r1", title="Paint the button red",
                   kind="user_feedback", technical_note="css var --accent")
    row = plan_state.get_task(world.db, created["task_id"])
    assert row["outcome"] == "Paint the button red"
    assert row["technical_note"] == "css var --accent"


def test_task_update_fields_status_and_events(world):
    task = call(world, "task.create", repository_id="r1", title="Export to file",
                kind="goal", estimated_loc=100)
    call(world, "task.update", task_id=task["task_id"], status="in_progress")
    call(world, "task.update", task_id=task["task_id"], estimated_loc=250,
         outcome="Exporting the table to a file works end to end.",
         impact="People cannot save their data.")
    done = call(world, "task.update", task_id=task["task_id"], status="done",
                note="Verified in the app.")
    assert done["status"] == "done"
    reopened = call(world, "task.update", task_id=task["task_id"], status="in_progress")
    assert reopened["status"] == "in_progress"
    with pytest.raises(ProtocolError, match="nothing to change"):
        call(world, "task.update", task_id=task["task_id"])
    with pytest.raises(ProtocolError, match="no task"):
        call(world, "task.update", task_id="p" + "0" * 16, status="done")
    history = call(world, "task.history", task_id=task["task_id"])
    kinds = [e["event"] for e in history["events"]]
    assert kinds == ["created", "status", "estimate", "edited", "status", "status"]
    estimate = next(e for e in history["events"] if e["event"] == "estimate")
    assert (estimate["from"], estimate["to"]) == ("100", "250")
    edited = next(e for e in history["events"] if e["event"] == "edited")
    assert edited["to"] == "impact,outcome"
    assert any(e["note"] == "Verified in the app." for e in history["events"])
    assert history["task"]["outcome"].startswith("Exporting")
    assert history["events_truncated"] is False


def test_task_elaboration_request_requires_an_atomic_plain_language_rewrite(world):
    task = call(world, "task.create", repository_id="r1",
                title="Complete downstream compatibility merge", kind="goal",
                outcome="The downstream compatibility merge is complete.")
    requested = call(world, "task.update", task_id=task["task_id"],
                     elaboration_needed=True)
    assert requested["elaboration_needed"] is True
    assert [row["task_id"] for row in requested["elaboration_requests"]] == \
        [task["task_id"]]

    overview = call(world, "plan.overview", repository_id="r1")
    projected = next(row for row in overview["tasks"]
                     if row["task_id"] == task["task_id"])
    assert projected["elaboration_needed"] is True
    assert overview["elaboration_requests"][0]["requested_at"] is not None
    assert call(world, "plan.overview")["repositories"][0][
        "elaboration_request_count"] == 1

    history = call(world, "task.history", task_id=task["task_id"])
    assert history["task"]["elaboration_needed"] is True
    assert history["elaboration_requests"][0]["task_id"] == task["task_id"]
    with pytest.raises(ProtocolError, match="changed title or outcome"):
        call(world, "task.update", task_id=task["task_id"],
             elaboration_needed=False)
    with pytest.raises(ProtocolError, match="true or false"):
        call(world, "task.update", task_id=task["task_id"],
             elaboration_needed=1)
    release_while_open = call(world, "release.create", repository_id="r1",
                              name="Current release", kind="release")
    assert release_while_open["elaboration_requests"][0]["task_id"] == \
        task["task_id"]
    decisions_while_open = call(world, "decision.tail", repository_id="r1")
    assert decisions_while_open["elaboration_requests"][0]["task_id"] == \
        task["task_id"]

    completed = call(
        world, "task.update", task_id=task["task_id"],
        title="Keep the new version working with connected tools",
        outcome=("The new version works with every connected tool that still"
                 " depends on the earlier format."),
        elaboration_needed=False)
    assert completed["elaboration_needed"] is False
    assert completed["elaboration_requests"] == []
    kinds = [event["event"] for event in
             call(world, "task.history", task_id=task["task_id"])["events"]]
    assert kinds == ["created", "elaboration_requested", "edited",
                     "elaboration_completed"]

    release = call(world, "release.create", repository_id="r1",
                   name="Later release", kind="release")
    assert release["elaboration_requests"] == []
    decisions = call(world, "decision.tail", repository_id="r1")
    assert decisions["elaboration_requests"] == []


def test_task_move_reorder_reparent(world):
    rel1 = call(world, "release.create", repository_id="r1", name="Release one",
                kind="release")
    rel2 = call(world, "release.create", repository_id="r1", name="Release two",
                kind="release")
    a = call(world, "task.create", repository_id="r1", title="Task aaa", kind="goal",
             release_id=rel1["release_id"], estimated_loc=10)
    b = call(world, "task.create", repository_id="r1", title="Task bbb", kind="goal",
             release_id=rel1["release_id"], estimated_loc=10)
    c = call(world, "task.create", repository_id="r1", title="Task ccc", kind="goal",
             release_id=rel1["release_id"], estimated_loc=10)
    # Reorder within the release: c to the front.
    moved = call(world, "task.update", task_id=c["task_id"], position=0)
    assert moved["position"] == 1
    order = {t["task_id"]: t["position"] for t in
             call(world, "plan.overview", repository_id="r1")["tasks"]}
    assert order[c["task_id"]] < order[a["task_id"]] < order[b["task_id"]]
    reorder = [e for e in events_for(world, c["task_id"]) if e["event"] == "reorder"]
    assert reorder and (reorder[0]["from_value"], reorder[0]["to_value"]) == ("3", "1")
    # Postpone to the second release.
    postponed = call(world, "task.update", task_id=b["task_id"],
                     release_id=rel2["release_id"])
    assert postponed["release_id"] == rel2["release_id"] and postponed["position"] == 1
    move = [e for e in events_for(world, b["task_id"]) if e["event"] == "release_move"]
    assert (move[0]["from_value"], move[0]["to_value"]) == (rel1["release_id"],
                                                            rel2["release_id"])
    # To the backlog with an explicit null.
    call(world, "task.update", task_id=b["task_id"], release_id=None)
    assert plan_state.get_task(world.db, b["task_id"])["release_id"] is None
    # Reparent, and reject cycles.
    call(world, "task.update", task_id=a["task_id"], parent_task_id=c["task_id"])
    with pytest.raises(ProtocolError, match="ancestor"):
        call(world, "task.update", task_id=c["task_id"], parent_task_id=a["task_id"])
    with pytest.raises(ProtocolError, match="ancestor"):
        call(world, "task.update", task_id=c["task_id"], parent_task_id=c["task_id"])


def test_task_drop_and_revive(world):
    rel = call(world, "release.create", repository_id="r1", name="Release one",
               kind="release")
    keep = call(world, "task.create", repository_id="r1", title="Keep this task",
                kind="goal", release_id=rel["release_id"], estimated_loc=50)
    drop = call(world, "task.create", repository_id="r1", title="Drop this task",
                kind="goal", release_id=rel["release_id"], estimated_loc=70)
    call(world, "task.update", task_id=drop["task_id"], status="dropped",
         note="Out of scope for now.")
    overview = call(world, "plan.overview", repository_id="r1")
    assert [t["task_id"] for t in overview["tasks"]] == [keep["task_id"]]
    assert overview["releases"][0]["loc_total"] == 50  # dropped is off the axis
    history = call(world, "task.history", task_id=drop["task_id"])
    assert history["task"]["status"] == "dropped"
    revived = call(world, "task.update", task_id=drop["task_id"], status="planned")
    assert revived["position"] == 2  # rejoins the end of its sibling group


def test_nothing_is_ever_deleted(world):
    task = call(world, "task.create", repository_id="r1", title="Some ledger task",
                kind="stub", estimated_loc=10)
    call(world, "task.update", task_id=task["task_id"], status="in_progress")
    call(world, "task.update", task_id=task["task_id"], status="done")
    call(world, "task.update", task_id=task["task_id"], status="dropped")
    counts = {t: world.db.query(f"SELECT COUNT(*) AS c FROM {t}")[0]["c"]
              for t in ("tasks", "plan_events")}
    assert counts["tasks"] == 1 and counts["plan_events"] == 4


def test_overview_truncation_keeps_open_tasks(world, monkeypatch):
    monkeypatch.setattr(plan_state, "OVERVIEW_TASK_CAP", 5)
    ids = [call(world, "task.create", repository_id="r1", title=f"Task number {i}",
                kind="goal", estimated_loc=10)["task_id"] for i in range(8)]
    for tid in ids[:6]:
        call(world, "task.update", task_id=tid, status="done")
    call(world, "task.update", task_id=ids[0], elaboration_needed=True)
    overview = call(world, "plan.overview", repository_id="r1")
    assert overview["tasks_truncated"] is True
    kept = {t["task_id"] for t in overview["tasks"]}
    assert set(ids[6:]) <= kept  # every open task survives the cut
    assert len(overview["tasks"]) == 5
    assert ids[0] not in kept
    assert [row["task_id"] for row in overview["elaboration_requests"]] == [ids[0]]
    assert overview["releases"] == []


# -- releases ----------------------------------------------------------------

def _seed_deployment(world, dep_id="d1", repo="r1", generation=3, domain="app",
                     port=41001):
    with world.db.transaction() as conn:
        conn.execute(
            "INSERT INTO deployments(deployment_id, repository_id, worktree_id, name,"
            " source, domain, spec_fingerprint, spec_json, state, created_at,"
            " created_by_uid, client, updated_at, current_generation)"
            " VALUES(?,?, 'w1',?, 'worktree',?, 'f','{}','running','t',1,'other','t',?)",
            (dep_id, repo, dep_id, domain, generation))
        conn.execute(
            "INSERT INTO generations VALUES(?,?,'abc123',1,'/x','fp','t','current')",
            (dep_id, generation))
        if domain:
            conn.execute("INSERT INTO domain_routes VALUES(?,?,'app',?,?,'t')",
                         (domain, dep_id, port, generation))
        conn.execute("INSERT INTO port_assignments VALUES(?,?,'app',?,'t')",
                     (port, dep_id, generation))


def test_release_lifecycle_request_and_update(world):
    rel = call(world, "release.create", repository_id="r1", name="Release one",
               kind="release")
    with pytest.raises(ProtocolError, match="already used"):
        call(world, "release.create", repository_id="r1", name="Clashing seq",
             kind="release", seq=rel["seq"])
    updated = call(world, "release.update", release_id=rel["release_id"],
                   name="Release one point one", seq=5, note="Owner reordered.")
    assert updated["seq"] == 5 and updated["name"] == "Release one point one"
    with pytest.raises(ProtocolError, match="planned and dropped"):
        call(world, "release.update", release_id=rel["release_id"], status="delivered")
    call(world, "release.update", release_id=rel["release_id"], status="dropped")
    call(world, "release.update", release_id=rel["release_id"], status="planned")

    requested = call(world, "release.request", repository_id="r1")
    assert requested["status"] == "requested"
    assert requested["name"].startswith("Preview (requested ")
    with pytest.raises(ProtocolError, match="already requested"):
        call(world, "release.request", repository_id="r1")
    overview = call(world, "plan.overview", repository_id="r1")
    assert overview["preview_requested"][0]["release_id"] == requested["release_id"]
    task = call(world, "task.create", repository_id="r1", title="Any ledger task",
                kind="goal")
    assert task["preview_requested"] is True
    with pytest.raises(ProtocolError, match="still open"):
        call(world, "task.create", repository_id="r1", title="Too late for this one",
             kind="goal", release_id=_deliver(world, requested)["release_id"])


def _deliver(world, requested):
    _seed_deployment(world)
    return call(world, "release.deliver", release_id=requested["release_id"],
                deployment_id="d1", note="First look.")


def test_release_deliver_snapshots_evidence(world):
    requested = call(world, "release.request", repository_id="r1")
    with pytest.raises(ProtocolError, match="no deployment"):
        call(world, "release.deliver", release_id=requested["release_id"],
             deployment_id="d" + "0" * 16)
    delivered = _deliver(world, requested)
    assert delivered["status"] == "delivered"
    assert delivered["url"] == "https://app.example.test"
    assert delivered["port"] == 41001
    assert delivered["commit_hash"] == "abc123" and delivered["dirty"] is True
    assert delivered["generation_number"] == 3
    with pytest.raises(ProtocolError, match="already delivered"):
        call(world, "release.deliver", release_id=requested["release_id"],
             deployment_id="d1")
    # The snapshot outlives the deployment rows (generations are pruned).
    delete_deployment_rows(world.db, "d1")
    row = plan_state.get_release(world.db, requested["release_id"])
    assert row["commit_hash"] == "abc123" and row["url"] == "https://app.example.test"
    assert row["fingerprint"] == "fp" and row["dirty"] == 1
    overview = call(world, "plan.overview", repository_id="r1")
    assert overview["releases"][0]["url"] == "https://app.example.test"
    assert overview["preview_requested"] == []


def test_release_deliver_port_fallback_and_cross_repo(world):
    _seed_deployment(world, dep_id="d2", repo="r1", domain=None, port=41005)
    _seed_deployment(world, dep_id="d3", repo="r2", domain="other", port=41006)
    requested = call(world, "release.request", repository_id="r1")
    with pytest.raises(ProtocolError, match="another repository"):
        call(world, "release.deliver", release_id=requested["release_id"],
             deployment_id="d3")
    delivered = call(world, "release.deliver", release_id=requested["release_id"],
                     deployment_id="d2")
    assert delivered["url"] is None and delivered["port"] == 41005

    with world.db.transaction() as conn:
        conn.execute("UPDATE deployments SET current_generation=NULL"
                     " WHERE deployment_id='d2'")
    second = call(world, "release.create", repository_id="r1", name="Second preview",
                  kind="preview")
    with pytest.raises(ProtocolError, match="apply it first"):
        call(world, "release.deliver", release_id=second["release_id"],
             deployment_id="d2")


# -- decisions ---------------------------------------------------------------

def test_decisions_record_tail_search_supersede(world):
    first = call(world, "decision.record", repository_id="r1", aspect="ui",
                 title="Buttons are blue", ref="DC2-2026-08-24-BLUE",
                 body="All primary buttons use the blue accent so actions are easy"
                      " to spot.")
    call(world, "decision.record", repository_id="r1", aspect="testing",
         title="Every page gets a browser test",
         body="Each console page is exercised by the browser harness before"
              " release.")
    with pytest.raises(ProtocolError, match="already used"):
        call(world, "decision.record", repository_id="r1", aspect="ui",
             title="Duplicate ref", ref="DC2-2026-08-24-BLUE",
             body="This must be rejected because the ref exists.")
    superseding = call(world, "decision.record", repository_id="r1", aspect="ui",
                       title="Buttons are green", supersedes="DC2-2026-08-24-BLUE",
                       body="The owner prefers green after seeing the preview.",
                       technical_note="--accent: #3fae5a")
    old = plan_state.get_decision(world.db, first["decision_id"])
    assert old["superseded_by"] == superseding["decision_id"]
    with pytest.raises(ProtocolError, match="already superseded"):
        call(world, "decision.record", repository_id="r1", aspect="ui",
             title="Buttons are pink", supersedes="DC2-2026-08-24-BLUE",
             body="Superseding an already superseded decision must fail.")

    tail = call(world, "decision.tail", repository_id="r1")
    assert [d["seq"] for d in tail["decisions"]] == [1, 2, 3]
    assert tail["summary"] is None and tail["has_more"] is False
    assert tail["unsummarized_count"] == 3 and tail["summary_due"] is False
    ui_only = call(world, "decision.tail", repository_id="r1", aspect="ui", n=1)
    assert [d["seq"] for d in ui_only["decisions"]] == [3]
    assert ui_only["has_more"] is True  # an older ui decision exists
    older = call(world, "decision.tail", repository_id="r1", n=2, before_seq=3)
    assert [d["seq"] for d in older["decisions"]] == [1, 2]
    assert older["has_more"] is False

    found = call(world, "decision.search", repository_id="r1", query="green owner")
    assert [d["seq"] for d in found["decisions"]] == [3]
    by_ref = call(world, "decision.search", repository_id="r1",
                  query="DC2-2026-08-24-BLUE")
    assert by_ref["decisions"][0]["seq"] == 1
    # FTS operators in user text are literal terms, never syntax.
    safe = call(world, "decision.search", repository_id="r1", query='blue AND "x')
    assert safe["decisions"] == []
    other_repo = call(world, "decision.search", repository_id="r2", query="green")
    assert other_repo["decisions"] == []


def test_decision_summaries_due_and_accumulate(world):
    for i in range(plan_state.SUMMARY_DUE_THRESHOLD):
        result = call(world, "decision.record", repository_id="r1", aspect="process",
                      title=f"Decision number {i}",
                      body="A recorded choice with enough words to pass validation.")
    assert result["summary_due"] is True
    assert result["unsummarized_count"] == plan_state.SUMMARY_DUE_THRESHOLD
    with pytest.raises(ProtocolError, match="beyond the latest"):
        call(world, "decision.summarize", repository_id="r1", body="Too far ahead"
             " to be honest about coverage.", covers_through_seq=999)
    stored = call(world, "decision.summarize", repository_id="r1",
                  covers_through_seq=20,
                  body="The story so far: twenty process decisions were made.")
    assert stored["unsummarized_count"] == 5 and stored["summary_due"] is False
    with pytest.raises(ProtocolError, match="already summarized"):
        call(world, "decision.summarize", repository_id="r1", body="Going backwards"
             " is not allowed at all.", covers_through_seq=19)
    call(world, "decision.summarize", repository_id="r1", covers_through_seq=25,
         body="The story so far: all twenty-five process decisions were made.")
    summaries = world.db.query(
        "SELECT covers_through_seq FROM decision_summaries WHERE repository_id='r1'"
        " ORDER BY covers_through_seq")
    assert [r["covers_through_seq"] for r in summaries] == [20, 25]
    tail = call(world, "decision.tail", repository_id="r1", n=2)
    assert tail["summary"]["covers_through_seq"] == 25
    assert tail["unsummarized_count"] == 0


def test_task_history_event_cap(world, monkeypatch):
    monkeypatch.setattr(plan_state, "HISTORY_EVENT_CAP", 5)
    task = call(world, "task.create", repository_id="r1", title="Busy little task",
                kind="goal", estimated_loc=1)
    for i in range(2, 9):
        call(world, "task.update", task_id=task["task_id"], estimated_loc=i)
    history = call(world, "task.history", task_id=task["task_id"])
    assert history["events_truncated"] is True
    assert len(history["events"]) == 5
    assert history["events"][-1]["to"] == "8"  # newest kept, oldest cut
