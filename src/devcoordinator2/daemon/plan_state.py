"""Planning, completion-ledger, and decision durable state (schema 8).

Append-only permanent history (DC2-2026-08-24-PLANNING-LEDGER): task and
release mutations pair every field change with a plan_events row in the same
transaction; decisions and summaries only accumulate. Nothing in this module
deletes rows. Bounded reads: the overview returns a compact active
projection, full text and event history come from task.history.
"""

from __future__ import annotations

import sqlite3
from typing import Any

from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.deploy_state import now_iso

SUMMARY_DUE_THRESHOLD = 25
OVERVIEW_TASK_CAP = 500       # keeps the worst-case response under the wire cap
HISTORY_EVENT_CAP = 200
TAIL_DEFAULT = 10
TAIL_MAX = 50
IMPACT_CLIP = 160             # overview carries a bounded excerpt; full via task.history
ELABORATION_OUTCOME_CLIP = 600


def _clip(text: str | None, limit: int) -> str | None:
    if text is None or len(text) <= limit:
        return text
    return text[: limit - 1].rstrip() + "…"


# -- sequencing and events ---------------------------------------------------

def next_seq(conn: sqlite3.Connection, table: str, repository_id: str) -> int:
    row = conn.execute(
        f"SELECT COALESCE(MAX(seq), 0) + 1 FROM {table} WHERE repository_id=?",
        (repository_id,)).fetchone()
    return int(row[0])


def append_event(conn: sqlite3.Connection, repository_id: str, subject_kind: str,
                 subject_id: str, event: str, from_value: str | None,
                 to_value: str | None, actor: str, note: str | None = None) -> None:
    conn.execute(
        "INSERT INTO plan_events(repository_id, subject_kind, subject_id, event,"
        " from_value, to_value, actor, at, note) VALUES(?,?,?,?,?,?,?,?,?)",
        (repository_id, subject_kind, subject_id, event, from_value, to_value,
         actor, now_iso(), note))


def set_fields(conn: sqlite3.Connection, table: str, id_column: str, ident: str,
               **fields: Any) -> None:
    fields["updated_at"] = now_iso()
    cols = ", ".join(f"{k}=?" for k in fields)
    conn.execute(f"UPDATE {table} SET {cols} WHERE {id_column}=?",
                 (*fields.values(), ident))


# -- rows --------------------------------------------------------------------

def get_task(db: Database, task_id: str) -> dict | None:
    rows = db.query("SELECT * FROM tasks WHERE task_id=?", (task_id,))
    return dict(rows[0]) if rows else None


def get_release(db: Database, release_id: str) -> dict | None:
    rows = db.query("SELECT * FROM releases WHERE release_id=?", (release_id,))
    return dict(rows[0]) if rows else None


def get_decision(db: Database, decision_id: str) -> dict | None:
    rows = db.query("SELECT * FROM decisions WHERE decision_id=?", (decision_id,))
    return dict(rows[0]) if rows else None


def get_decision_by_ref(db: Database, repository_id: str, ref: str) -> dict | None:
    rows = db.query("SELECT * FROM decisions WHERE repository_id=? AND ref=?",
                    (repository_id, ref))
    return dict(rows[0]) if rows else None


# -- sibling ordering --------------------------------------------------------

def place_task(conn: sqlite3.Connection, repository_id: str,
               parent_task_id: str | None, release_id: str | None,
               task_id: str, index: int | None) -> int:
    """Renumber the (parent, release) sibling group with task_id inserted at
    the requested 0-based index (clamped; None appends). Dropped tasks keep
    their old position and are skipped. Returns the task's new position."""
    siblings = [r["task_id"] for r in conn.execute(
        "SELECT task_id FROM tasks WHERE repository_id=? AND parent_task_id IS ?"
        " AND release_id IS ? AND status != 'dropped' AND task_id != ?"
        " ORDER BY position, seq",
        (repository_id, parent_task_id, release_id, task_id))]
    if index is None:
        index = len(siblings)
    index = max(0, min(index, len(siblings)))
    siblings.insert(index, task_id)
    position = 0
    for pos, ident in enumerate(siblings, start=1):
        conn.execute("UPDATE tasks SET position=? WHERE task_id=?", (pos, ident))
        if ident == task_id:
            position = pos
    return position


def is_ancestor(db: Database, candidate: str, task_id: str) -> bool:
    """True when candidate is task_id itself or one of its descendants
    (reparenting task_id under candidate would create a cycle)."""
    frontier = {task_id}
    while frontier:
        if candidate in frontier:
            return True
        marks = ",".join("?" * len(frontier))
        frontier = {r["task_id"] for r in db.query(
            f"SELECT task_id FROM tasks WHERE parent_task_id IN ({marks})",
            tuple(frontier))}
    return False


# -- overview projection -----------------------------------------------------

def _leaf_aggregates(tasks: list[dict]) -> tuple[dict[str | None, dict], set[str]]:
    """Per-release {loc_total, loc_done, tasks_total, tasks_done} over leaf
    tasks (a parent's own estimate is presentation, not additional work)."""
    parents = {t["parent_task_id"] for t in tasks if t["parent_task_id"]}
    aggregates: dict[str | None, dict] = {}
    for t in tasks:
        if t["task_id"] in parents:
            continue
        agg = aggregates.setdefault(
            t["release_id"],
            {"tasks_total": 0, "tasks_done": 0, "loc_total": 0, "loc_done": 0})
        loc = t["estimated_loc"] or 0
        agg["tasks_total"] += 1
        agg["loc_total"] += loc
        if t["status"] == "done":
            agg["tasks_done"] += 1
            agg["loc_done"] += loc
    return aggregates, parents


def _compact_task(row: dict) -> dict:
    return {"task_id": row["task_id"], "parent_task_id": row["parent_task_id"],
            "release_id": row["release_id"], "seq": row["seq"],
            "position": row["position"], "title": row["title"],
            "impact": _clip(row["impact"], IMPACT_CLIP), "status": row["status"],
            "kind": row["kind"], "estimated_loc": row["estimated_loc"],
            "elaboration_needed": bool(row["elaboration_needed"])}


def _release_public(row: dict, aggregates: dict[str | None, dict]) -> dict:
    agg = aggregates.get(row["release_id"],
                         {"tasks_total": 0, "tasks_done": 0,
                          "loc_total": 0, "loc_done": 0})
    return {"release_id": row["release_id"], "name": row["name"],
            "kind": row["kind"], "status": row["status"], "seq": row["seq"],
            "note": row["note"], "requested_at": row["requested_at"],
            "delivered_at": row["delivered_at"], "url": row["url"],
            "port": row["port"], **agg}


def preview_requested_rows(db: Database, repository_id: str) -> list[dict]:
    return [{"release_id": r["release_id"], "name": r["name"],
             "requested_at": r["requested_at"], "note": r["note"]}
            for r in db.query(
                "SELECT release_id, name, requested_at, note FROM releases"
                " WHERE repository_id=? AND status='requested' ORDER BY seq",
                (repository_id,))]


def has_requested(db: Database, repository_id: str) -> bool:
    return bool(db.query(
        "SELECT 1 FROM releases WHERE repository_id=? AND status='requested' LIMIT 1",
        (repository_id,)))


def elaboration_requests(db: Database, repository_id: str) -> list[dict]:
    """Outstanding owner requests, independent of the overview task cap."""
    rows = db.query(
        "SELECT t.task_id, t.title, t.outcome, t.status, t.kind,"
        " (SELECT e.at FROM plan_events e"
        "  WHERE e.subject_kind='task' AND e.subject_id=t.task_id"
        "    AND e.event='elaboration_requested'"
        "  ORDER BY e.event_id DESC LIMIT 1) AS requested_at"
        " FROM tasks t WHERE t.repository_id=? AND t.elaboration_needed=1"
        " ORDER BY t.seq",
        (repository_id,))
    return [{"task_id": row["task_id"], "title": row["title"],
             "outcome": _clip(row["outcome"], ELABORATION_OUTCOME_CLIP),
             "status": row["status"], "kind": row["kind"],
             "requested_at": row["requested_at"]}
            for row in rows]


def overview(db: Database, repository: dict) -> dict:
    repository_id = repository["repository_id"]
    releases = [dict(r) for r in db.query(
        "SELECT * FROM releases WHERE repository_id=? AND status != 'dropped'"
        " ORDER BY seq", (repository_id,))]
    tasks = [dict(r) for r in db.query(
        "SELECT task_id, parent_task_id, release_id, seq, position, title, impact,"
        " status, kind, estimated_loc, elaboration_needed"
        " FROM tasks WHERE repository_id=?"
        " AND status != 'dropped' ORDER BY seq", (repository_id,))]
    aggregates, _parents = _leaf_aggregates(tasks)
    truncated = len(tasks) > OVERVIEW_TASK_CAP
    if truncated:
        # The active plan is never cut: keep every unfinished task, then the
        # most recent finished ones up to the cap.
        open_tasks = [t for t in tasks if t["status"] != "done"]
        done = [t for t in tasks if t["status"] == "done"]
        keep = open_tasks + done[max(0, len(done) - (OVERVIEW_TASK_CAP - len(open_tasks))):]
        tasks = sorted(keep, key=lambda t: t["seq"])[:OVERVIEW_TASK_CAP]
    return {
        "repository_id": repository_id,
        "display_name": repository["display_name"],
        "releases": [_release_public(r, aggregates) for r in releases],
        "tasks": [_compact_task(t) for t in tasks],
        "tasks_truncated": truncated,
        "elaboration_requests": elaboration_requests(db, repository_id),
        "preview_requested": preview_requested_rows(db, repository_id),
        "decisions": {"unsummarized_count": unsummarized_count(db, repository_id),
                      "summary_due": summary_due(db, repository_id)},
    }


def picker(db: Database) -> list[dict]:
    """One row per registered repository with plan aggregates; the access
    layer filters rows for public identities."""
    repos = db.query("SELECT repository_id, display_name FROM repositories"
                     " ORDER BY display_name, repository_id")
    rows = []
    for repo in repos:
        repository_id = repo["repository_id"]
        tasks = [dict(r) for r in db.query(
            "SELECT task_id, parent_task_id, release_id, status, estimated_loc"
            " FROM tasks WHERE repository_id=? AND status != 'dropped'",
            (repository_id,))]
        aggregates, _ = _leaf_aggregates(tasks)
        totals = {"tasks_total": 0, "tasks_done": 0, "loc_total": 0, "loc_done": 0}
        for agg in aggregates.values():
            for key in totals:
                totals[key] += agg[key]
        releases = [dict(r) for r in db.query(
            "SELECT release_id, name, kind, status FROM releases WHERE repository_id=?"
            " AND status != 'dropped' ORDER BY seq", (repository_id,))]
        current = next((r for r in releases if r["status"] in ("planned", "requested")),
                       releases[-1] if releases else None)
        elaboration_count = db.query(
            "SELECT COUNT(*) AS count FROM tasks"
            " WHERE repository_id=? AND elaboration_needed=1",
            (repository_id,))[0]["count"]
        rows.append({
            "repository_id": repository_id,
            "display_name": repo["display_name"],
            "open_tasks": totals["tasks_total"] - totals["tasks_done"],
            "loc_done": totals["loc_done"], "loc_total": totals["loc_total"],
            "current_release": ({"name": current["name"], "kind": current["kind"],
                                 "status": current["status"]} if current else None),
            "preview_requested": has_requested(db, repository_id),
            "elaboration_request_count": elaboration_count,
        })
    return rows


def task_events(db: Database, task_id: str) -> tuple[list[dict], bool]:
    rows = db.query(
        "SELECT event, from_value, to_value, actor, at, note FROM plan_events"
        " WHERE subject_kind='task' AND subject_id=? ORDER BY event_id DESC LIMIT ?",
        (task_id, HISTORY_EVENT_CAP + 1))
    truncated = len(rows) > HISTORY_EVENT_CAP
    kept = list(reversed(rows[:HISTORY_EVENT_CAP]))
    return [{"event": r["event"], "from": r["from_value"], "to": r["to_value"],
             "actor": r["actor"], "at": r["at"], "note": r["note"]}
            for r in kept], truncated


# -- decisions ---------------------------------------------------------------

def decision_public(row: dict) -> dict:
    return {"decision_id": row["decision_id"], "seq": row["seq"], "ref": row["ref"],
            "aspect": row["aspect"], "title": row["title"], "body": row["body"],
            "technical_note": row["technical_note"],
            "superseded_by": row["superseded_by"], "created_at": row["created_at"],
            "created_by": row["created_by"]}


def latest_summary(db: Database, repository_id: str) -> dict | None:
    rows = db.query(
        "SELECT body, covers_through_seq, created_at FROM decision_summaries"
        " WHERE repository_id=? ORDER BY covers_through_seq DESC LIMIT 1",
        (repository_id,))
    if not rows:
        return None
    return {"body": rows[0]["body"],
            "covers_through_seq": rows[0]["covers_through_seq"],
            "created_at": rows[0]["created_at"]}


def max_decision_seq(db: Database, repository_id: str) -> int:
    rows = db.query("SELECT COALESCE(MAX(seq), 0) AS m FROM decisions"
                    " WHERE repository_id=?", (repository_id,))
    return int(rows[0]["m"])


def covered_through(db: Database, repository_id: str) -> int:
    rows = db.query(
        "SELECT COALESCE(MAX(covers_through_seq), 0) AS c FROM decision_summaries"
        " WHERE repository_id=?", (repository_id,))
    return int(rows[0]["c"])


def unsummarized_count(db: Database, repository_id: str) -> int:
    covered = covered_through(db, repository_id)
    rows = db.query("SELECT COUNT(*) AS c FROM decisions WHERE repository_id=?"
                    " AND seq > ?", (repository_id, covered))
    return int(rows[0]["c"])


def summary_due(db: Database, repository_id: str) -> bool:
    return unsummarized_count(db, repository_id) >= SUMMARY_DUE_THRESHOLD


def decision_tail(db: Database, repository_id: str, aspect: str | None,
                  n: int, before_seq: int | None = None) -> tuple[list[dict], bool]:
    sql = "SELECT * FROM decisions WHERE repository_id=?"
    params: list[Any] = [repository_id]
    if aspect:
        sql += " AND aspect=?"
        params.append(aspect)
    if before_seq is not None:
        sql += " AND seq < ?"
        params.append(before_seq)
    sql += " ORDER BY seq DESC LIMIT ?"
    params.append(n + 1)
    rows = db.query(sql, tuple(params))
    has_more = len(rows) > n
    kept = list(reversed(rows[:n]))
    return [decision_public(dict(r)) for r in kept], has_more


def fts_query(query: str) -> str:
    """Quote every whitespace-separated term so user text can never inject
    FTS5 operators; terms are implicitly ANDed."""
    terms = [t.replace('"', '""') for t in query.split()]
    return " ".join(f'"{t}"' for t in terms)


def decision_search(db: Database, repository_id: str, query: str,
                    aspect: str | None, n: int) -> tuple[list[dict], bool]:
    sql = ("SELECT d.* FROM decisions_fts f JOIN decisions d ON d.rowid = f.rowid"
           " WHERE decisions_fts MATCH ? AND d.repository_id=?")
    params: list[Any] = [fts_query(query), repository_id]
    if aspect:
        sql += " AND d.aspect=?"
        params.append(aspect)
    sql += " ORDER BY bm25(decisions_fts) LIMIT ?"
    params.append(n + 1)
    rows = db.query(sql, tuple(params))
    has_more = len(rows) > n
    return [decision_public(dict(r)) for r in rows[:n]], has_more
