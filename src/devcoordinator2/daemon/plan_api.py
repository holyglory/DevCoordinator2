"""plan.* / task.* / release.* / decision.* handlers (schema 8).

DevCoordinator owns agent planning, the completion ledger, and the decision
history (DC2-2026-08-24-PLANNING-LEDGER). Plain language is enforced
structurally: bounded required title/outcome/body, single-line titles, and a
separate technical_note that never substitutes for them; the daemon cannot
detect jargon in prose — the register rule lives in the agent instructions.
"""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path
from typing import Any

from devcoordinator2 import ids
from devcoordinator2.daemon import deploy_state, events, plan_state
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.deploy_state import now_iso
from devcoordinator2.daemon.gitinfo import GitResolveError
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller, Handler
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

TASK_KINDS = ("goal", "stub", "improvement", "user_feedback")
TASK_STATUSES = ("planned", "in_progress", "done", "dropped")
RELEASE_KINDS = ("preview", "release")
RELEASE_STATUSES = ("planned", "requested", "delivered", "dropped")
ASPECTS = ("ui", "architecture", "algorithms", "business_logic", "data", "testing",
           "deployment", "security", "performance", "process", "other")

_TASK_TEXT_FIELDS = {  # field -> (min, max)
    "outcome": (10, 2000), "impact": (1, 2000), "unblock_condition": (1, 2000),
    "verification": (1, 2000), "technical_note": (1, 4000),
}


def _only(args: dict[str, Any], allowed: set[str]) -> None:
    unknown = set(args) - allowed
    if unknown:
        raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")


def _plain_line(args: dict[str, Any], key: str, *, required: bool,
                lo: int = 3, hi: int = 120) -> str | None:
    value = args.get(key)
    if value is None:
        if required:
            raise ProtocolError("args_invalid", f"'{key}' (string) is required")
        return None
    if not isinstance(value, str) or "\n" in value or "\r" in value \
            or not (lo <= len(value.strip()) <= hi):
        raise ProtocolError(
            "args_invalid",
            f"'{key}' must be one plain single-line sentence a non-technical"
            f" reader understands ({lo}..{hi} characters)")
    return value.strip()


def _plain_text(args: dict[str, Any], key: str, *, required: bool,
                lo: int, hi: int) -> str | None:
    value = args.get(key)
    if value is None:
        if required:
            raise ProtocolError("args_invalid", f"'{key}' (string) is required")
        return None
    if not isinstance(value, str) or not (lo <= len(value.strip()) <= hi):
        raise ProtocolError("args_invalid",
                            f"'{key}' must be plain text of {lo}..{hi} characters")
    return value.strip()


def _enum(args: dict[str, Any], key: str, allowed: tuple[str, ...], *,
          required: bool) -> str | None:
    value = args.get(key)
    if value is None:
        if required:
            raise ProtocolError("args_invalid",
                                f"'{key}' is required: one of {', '.join(allowed)}")
        return None
    if value not in allowed:
        raise ProtocolError("args_invalid",
                            f"'{key}' must be one of {', '.join(allowed)}")
    return value


def _int_arg(args: dict[str, Any], key: str, lo: int, hi: int) -> int | None:
    value = args.get(key)
    if value is None:
        return None
    if not isinstance(value, int) or isinstance(value, bool) or not (lo <= value <= hi):
        raise ProtocolError("args_invalid", f"'{key}' must be an integer in {lo}..{hi}")
    return value


def _actor(caller: Caller) -> str:
    return caller.identity or f"uid:{caller.uid}"


def build_plan_handlers(config: InstanceConfig, db: Database,
                        registry: Registry) -> dict[str, Handler]:
    def _repository(args: dict[str, Any], caller: Caller) -> dict:
        """Resolve {repository_id} (Console) or {path} (agents; implicit
        registration like test.start) to a repositories row."""
        rid = args.get("repository_id")
        if rid is not None:
            if not isinstance(rid, str) or not rid.startswith("r"):
                raise ProtocolError("args_invalid", "'repository_id' must be an 'r…' id")
            rows = db.query("SELECT repository_id, display_name FROM repositories"
                            " WHERE repository_id=?", (rid,))
            if not rows:
                raise ProtocolError("repository_not_found", f"no repository {rid}")
            return dict(rows[0])
        raw = args.get("path")
        if not isinstance(raw, str) or not raw:
            raise ProtocolError("args_invalid",
                                "'path' or 'repository_id' is required")
        path = Path(raw)
        if not path.is_absolute():
            raise ProtocolError("args_invalid", "'path' must be absolute")
        try:
            reg = registry.register(path, caller_uid=caller.uid, caller_gid=caller.gid)
        except GitResolveError as exc:
            raise ProtocolError("repository_not_found", str(exc)) from exc
        return {"repository_id": reg.repository_id, "display_name": reg.display_name}

    def _release_in(repo_id: str, release_id: Any, *, open_only: bool) -> dict:
        if not isinstance(release_id, str) or not release_id.startswith("v"):
            raise ProtocolError("args_invalid", "'release_id' must be a 'v…' id")
        release = plan_state.get_release(db, release_id)
        if release is None:
            raise ProtocolError("release_not_found", f"no release {release_id}")
        if release["repository_id"] != repo_id:
            raise ProtocolError("args_invalid",
                                "the release belongs to another repository")
        if open_only and release["status"] in ("delivered", "dropped"):
            raise ProtocolError(
                "args_invalid",
                f"release {release['name']!r} is {release['status']}; tasks can only"
                " be planned into a release that is still open")
        return release

    # -- reads ---------------------------------------------------------------

    def plan_overview(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"path", "repository_id"})
        if args.get("path") is None and args.get("repository_id") is None:
            return {"repositories": plan_state.picker(db)}
        return plan_state.overview(db, _repository(args, caller))

    def task_history(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"task_id"})
        task = _task(args)
        event_rows, truncated = plan_state.task_events(db, task["task_id"])
        return {"task": task, "events": event_rows, "events_truncated": truncated}

    def _task(args: dict[str, Any]) -> dict:
        tid = args.get("task_id")
        if not isinstance(tid, str) or not tid.startswith("p"):
            raise ProtocolError("args_invalid", "'task_id' must be a 'p…' id")
        task = plan_state.get_task(db, tid)
        if task is None:
            raise ProtocolError("task_not_found", f"no task {tid}")
        return task

    # -- tasks ---------------------------------------------------------------

    def task_create(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"path", "repository_id", "title", "kind", "outcome",
                     "parent_task_id", "release_id", "estimated_loc", "impact",
                     "unblock_condition", "verification", "technical_note"})
        repo = _repository(args, caller)
        repo_id = repo["repository_id"]
        title = _plain_line(args, "title", required=True)
        kind = _enum(args, "kind", TASK_KINDS, required=True)
        fields = {key: _plain_text(args, key, required=False, lo=lo, hi=hi)
                  for key, (lo, hi) in _TASK_TEXT_FIELDS.items()}
        # The plain outcome defaults to the (already validated) title so a
        # short owner request never fails on a second required field.
        outcome = fields.pop("outcome") or title
        estimated_loc = _int_arg(args, "estimated_loc", 1, 1_000_000)
        parent_id = args.get("parent_task_id")
        if parent_id is not None:
            parent = plan_state.get_task(db, parent_id) \
                if isinstance(parent_id, str) and parent_id.startswith("p") else None
            if parent is None:
                raise ProtocolError("task_not_found", f"no task {parent_id}")
            if parent["repository_id"] != repo_id:
                raise ProtocolError("args_invalid",
                                    "the parent task belongs to another repository")
        release_id = args.get("release_id")
        if release_id is not None:
            _release_in(repo_id, release_id, open_only=True)
        task_id = ids.task_id()
        now = now_iso()
        with db.transaction() as conn:
            seq = plan_state.next_seq(conn, "tasks", repo_id)
            conn.execute(
                "INSERT INTO tasks(task_id, repository_id, parent_task_id, release_id,"
                " seq, position, title, outcome, impact, unblock_condition,"
                " verification, technical_note, kind, status, estimated_loc,"
                " created_at, created_by, updated_at)"
                " VALUES(?,?,?,?,?,0,?,?,?,?,?,?,?,'planned',?,?,?,?)",
                (task_id, repo_id, parent_id, release_id, seq, title, outcome,
                 fields["impact"], fields["unblock_condition"], fields["verification"],
                 fields["technical_note"], kind, estimated_loc, now,
                 _actor(caller), now))
            position = plan_state.place_task(conn, repo_id, parent_id, release_id,
                                             task_id, None)
            plan_state.append_event(conn, repo_id, "task", task_id, "created",
                                    None, kind, _actor(caller))
        return {"task_id": task_id, "repository_id": repo_id, "seq": seq,
                "position": position, "status": "planned", "release_id": release_id,
                "preview_requested": plan_state.has_requested(db, repo_id)}

    def task_update(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"task_id", "title", "outcome", "impact", "unblock_condition",
                     "verification", "technical_note", "estimated_loc", "status",
                     "release_id", "parent_task_id", "position", "note"})
        task = _task(args)
        repo_id = task["repository_id"]
        actor = _actor(caller)
        note = _plain_text(args, "note", required=False, lo=1, hi=500)
        changes: dict[str, Any] = {}
        pending: list[tuple[str, str | None, str | None]] = []  # (event, from, to)
        edited: list[str] = []
        title = _plain_line(args, "title", required=False)
        if title is not None and title != task["title"]:
            changes["title"] = title
            edited.append("title")
        for key, (lo, hi) in _TASK_TEXT_FIELDS.items():
            value = _plain_text(args, key, required=False, lo=lo, hi=hi)
            if value is not None and value != task[key]:
                changes[key] = value
                edited.append(key)
        loc = _int_arg(args, "estimated_loc", 1, 1_000_000)
        if loc is not None and loc != task["estimated_loc"]:
            changes["estimated_loc"] = loc
            pending.append(("estimate", str(task["estimated_loc"]), str(loc)))
        status = _enum(args, "status", TASK_STATUSES, required=False)
        if status is not None and status != task["status"]:
            changes["status"] = status
            pending.append(("status", task["status"], status))
        regroup = False
        new_release = task["release_id"]
        if "release_id" in args:
            value = args["release_id"]
            if value is not None:
                _release_in(repo_id, value, open_only=True)
            if value != task["release_id"]:
                changes["release_id"] = value
                pending.append(("release_move", task["release_id"], value))
                new_release = value
                regroup = True
        new_parent = task["parent_task_id"]
        if "parent_task_id" in args:
            value = args["parent_task_id"]
            if value is not None:
                if not isinstance(value, str) or not value.startswith("p"):
                    raise ProtocolError("args_invalid",
                                        "'parent_task_id' must be a 'p…' id or null")
                parent = plan_state.get_task(db, value)
                if parent is None:
                    raise ProtocolError("task_not_found", f"no task {value}")
                if parent["repository_id"] != repo_id:
                    raise ProtocolError("args_invalid",
                                        "the parent task belongs to another repository")
                if plan_state.is_ancestor(db, value, task["task_id"]):
                    raise ProtocolError("args_invalid",
                                        "that move would make the task its own ancestor")
            if value != task["parent_task_id"]:
                changes["parent_task_id"] = value
                pending.append(("reparent", task["parent_task_id"], value))
                new_parent = value
                regroup = True
        index = _int_arg(args, "position", 0, 100_000)
        if index is not None:
            regroup = True
        if status is not None and status != task["status"] and task["status"] == "dropped":
            regroup = True  # a revived task rejoins its sibling order
        if not changes and index is None:
            raise ProtocolError("args_invalid", "nothing to change")
        if edited:
            pending.append(("edited", None, ",".join(sorted(edited))))
        with db.transaction() as conn:
            if changes:
                plan_state.set_fields(conn, "tasks", "task_id", task["task_id"],
                                      **changes)
            new_position = task["position"]
            if regroup and changes.get("status", task["status"]) != "dropped":
                new_position = plan_state.place_task(conn, repo_id, new_parent,
                                                     new_release, task["task_id"],
                                                     index)
                if index is not None and new_position != task["position"]:
                    pending.append(("reorder", str(task["position"]),
                                    str(new_position)))
            for event, from_value, to_value in pending:
                plan_state.append_event(conn, repo_id, "task", task["task_id"],
                                        event, from_value, to_value, actor, note)
        updated = plan_state.get_task(db, task["task_id"])
        return {"task_id": updated["task_id"], "repository_id": repo_id,
                "seq": updated["seq"], "position": updated["position"],
                "title": updated["title"], "status": updated["status"],
                "kind": updated["kind"], "release_id": updated["release_id"],
                "parent_task_id": updated["parent_task_id"],
                "estimated_loc": updated["estimated_loc"],
                "preview_requested": plan_state.has_requested(db, repo_id)}

    # -- releases ------------------------------------------------------------

    def release_create(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"path", "repository_id", "name", "kind", "note", "seq"})
        repo = _repository(args, caller)
        name = _plain_line(args, "name", required=True)
        kind = _enum(args, "kind", RELEASE_KINDS, required=True)
        note = _plain_text(args, "note", required=False, lo=1, hi=500)
        seq = _int_arg(args, "seq", 1, 100_000)
        return _insert_release(repo["repository_id"], name, kind, "planned",
                               note, seq, _actor(caller))

    def _insert_release(repo_id: str, name: str, kind: str, status: str,
                        note: str | None, seq: int | None, actor: str) -> dict:
        release_id = ids.release_id()
        now = now_iso()
        requested_at = now if status == "requested" else None
        with db.transaction() as conn:
            final_seq = seq if seq is not None \
                else plan_state.next_seq(conn, "releases", repo_id)
            try:
                conn.execute(
                    "INSERT INTO releases(release_id, repository_id, seq, name, kind,"
                    " status, note, requested_at, created_at, created_by, updated_at)"
                    " VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                    (release_id, repo_id, final_seq, name, kind, status, note,
                     requested_at, now, actor, now))
            except sqlite3.IntegrityError as exc:
                raise ProtocolError("args_invalid",
                                    f"position {final_seq} is already used by another"
                                    " release of this repository") from exc
            plan_state.append_event(conn, repo_id, "release", release_id, "created",
                                    None, kind, actor, note)
            if status == "requested":
                plan_state.append_event(conn, repo_id, "release", release_id,
                                        "requested", "planned", "requested", actor,
                                        note)
        return {"release_id": release_id, "repository_id": repo_id, "seq": final_seq,
                "name": name, "kind": kind, "status": status,
                "requested_at": requested_at}

    def release_update(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"release_id", "name", "seq", "note", "status"})
        release = _release(args)
        actor = _actor(caller)
        changes: dict[str, Any] = {}
        edited: list[str] = []
        pending: list[tuple[str, str | None, str | None]] = []
        name = _plain_line(args, "name", required=False)
        if name is not None and name != release["name"]:
            changes["name"] = name
            edited.append("name")
        note = _plain_text(args, "note", required=False, lo=1, hi=500)
        if note is not None and note != release["note"]:
            changes["note"] = note
            edited.append("note")
        seq = _int_arg(args, "seq", 1, 100_000)
        if seq is not None and seq != release["seq"]:
            changes["seq"] = seq
            edited.append("seq")
        status = _enum(args, "status", RELEASE_STATUSES, required=False)
        if status is not None and status != release["status"]:
            if {release["status"], status} != {"planned", "dropped"}:
                raise ProtocolError(
                    "args_invalid",
                    "status may only change between planned and dropped here;"
                    " requesting and delivering are their own commands")
            changes["status"] = status
            pending.append(("status", release["status"], status))
        if not changes:
            raise ProtocolError("args_invalid", "nothing to change")
        if edited:
            pending.append(("edited", None, ",".join(sorted(edited))))
        with db.transaction() as conn:
            try:
                plan_state.set_fields(conn, "releases", "release_id",
                                      release["release_id"], **changes)
            except sqlite3.IntegrityError as exc:
                raise ProtocolError("args_invalid",
                                    f"position {seq} is already used by another"
                                    " release of this repository") from exc
            for event, from_value, to_value in pending:
                plan_state.append_event(conn, release["repository_id"], "release",
                                        release["release_id"], event, from_value,
                                        to_value, actor)
        updated = plan_state.get_release(db, release["release_id"])
        return {"release_id": updated["release_id"], "seq": updated["seq"],
                "name": updated["name"], "kind": updated["kind"],
                "status": updated["status"], "note": updated["note"]}

    def _release(args: dict[str, Any]) -> dict:
        rid = args.get("release_id")
        if not isinstance(rid, str) or not rid.startswith("v"):
            raise ProtocolError("args_invalid", "'release_id' must be a 'v…' id")
        release = plan_state.get_release(db, rid)
        if release is None:
            raise ProtocolError("release_not_found", f"no release {rid}")
        return release

    def release_request(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"path", "repository_id", "name", "note"})
        repo = _repository(args, caller)
        if plan_state.has_requested(db, repo["repository_id"]):
            raise ProtocolError("args_invalid",
                                "a preview is already requested for this repository")
        name = _plain_line(args, "name", required=False) \
            or f"Preview (requested {now_iso()[:10]})"
        note = _plain_text(args, "note", required=False, lo=1, hi=500)
        result = _insert_release(repo["repository_id"], name, "preview", "requested",
                                 note, None, _actor(caller))
        events.publish("release.requested", repository_id=repo["repository_id"],
                       release_id=result["release_id"], name=name,
                       repository_name=repo["display_name"])
        return result

    def release_deliver(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"release_id", "deployment_id", "note"})
        release = _release(args)
        if release["status"] not in ("planned", "requested"):
            raise ProtocolError("args_invalid",
                                f"release {release['name']!r} is already"
                                f" {release['status']}; deliver the next preview as"
                                " a new release")
        note = _plain_text(args, "note", required=False, lo=1, hi=500)
        dep_id = args.get("deployment_id")
        if not isinstance(dep_id, str) or not dep_id.startswith("d"):
            raise ProtocolError("args_invalid", "'deployment_id' must be a 'd…' id")
        deployment = deploy_state.get_deployment(db, dep_id)
        if deployment is None:
            raise ProtocolError("deployment_not_found", f"no deployment {dep_id}")
        if deployment["repository_id"] != release["repository_id"]:
            raise ProtocolError("args_invalid",
                                "the deployment belongs to another repository")
        if deployment["current_generation"] is None:
            raise ProtocolError("args_invalid",
                                "the deployment has no running generation yet;"
                                " apply it first, then deliver")
        generation = deploy_state.generation(db, dep_id,
                                             deployment["current_generation"]) or {}
        # Reachability evidence: the routed domain when one exists, and the
        # leased host port either way (generations are pruned; this snapshot
        # is permanent).
        route = db.query("SELECT domain, port FROM domain_routes WHERE deployment_id=?",
                         (dep_id,))
        url = None
        port = route[0]["port"] if route else None
        if route and route[0]["domain"]:
            fqdn = f"{route[0]['domain']}.{config.base_domain}" \
                if config.base_domain else route[0]["domain"]
            url = f"https://{fqdn}"
        if port is None:
            ports = db.query(
                "SELECT port FROM port_assignments WHERE deployment_id=?"
                " ORDER BY generation DESC LIMIT 1", (dep_id,))
            port = ports[0]["port"] if ports else None
        now = now_iso()
        evidence = {"deployment_id": dep_id,
                    "generation_number": deployment["current_generation"],
                    "commit_hash": generation.get("commit_hash"),
                    "dirty": bool(generation.get("dirty")), "url": url, "port": port}
        with db.transaction() as conn:
            plan_state.set_fields(conn, "releases", "release_id",
                                  release["release_id"], status="delivered",
                                  delivered_at=now, deployment_id=dep_id,
                                  generation_number=evidence["generation_number"],
                                  commit_hash=evidence["commit_hash"],
                                  dirty=int(evidence["dirty"]),
                                  fingerprint=generation.get("fingerprint"),
                                  url=url, port=port)
            plan_state.append_event(conn, release["repository_id"], "release",
                                    release["release_id"], "delivered",
                                    release["status"], json.dumps(evidence),
                                    _actor(caller), note)
        events.publish("release.delivered", repository_id=release["repository_id"],
                       release_id=release["release_id"], name=release["name"],
                       url=url, port=port, dirty=evidence["dirty"])
        return {"release_id": release["release_id"], "status": "delivered",
                "delivered_at": now, "url": url, "port": port,
                "commit_hash": evidence["commit_hash"], "dirty": evidence["dirty"],
                "generation_number": evidence["generation_number"]}

    # -- decisions -----------------------------------------------------------

    def decision_record(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"path", "repository_id", "aspect", "title", "body",
                     "technical_note", "ref", "supersedes"})
        repo = _repository(args, caller)
        repo_id = repo["repository_id"]
        aspect = _enum(args, "aspect", ASPECTS, required=True)
        title = _plain_line(args, "title", required=True)
        body = _plain_text(args, "body", required=True, lo=10, hi=4000)
        technical_note = _plain_text(args, "technical_note", required=False,
                                     lo=1, hi=4000)
        ref = args.get("ref")
        if ref is not None and (not isinstance(ref, str) or "\n" in ref
                                or " " in ref or not (3 <= len(ref) <= 80)):
            raise ProtocolError("args_invalid",
                                "'ref' must be a short stable key without spaces"
                                " (3..80 characters), e.g. DC2-2026-08-24-TOPIC")
        superseded = None
        supersedes = args.get("supersedes")
        if supersedes is not None:
            if not isinstance(supersedes, str) or not supersedes:
                raise ProtocolError("args_invalid",
                                    "'supersedes' must be a decision id or ref")
            superseded = plan_state.get_decision(db, supersedes) \
                or plan_state.get_decision_by_ref(db, repo_id, supersedes)
            if superseded is None:
                raise ProtocolError("decision_not_found",
                                    f"no decision {supersedes} in this repository")
            if superseded["repository_id"] != repo_id:
                raise ProtocolError("args_invalid",
                                    "the superseded decision belongs to another"
                                    " repository")
            if superseded["superseded_by"] is not None:
                raise ProtocolError("args_invalid",
                                    "that decision is already superseded by"
                                    f" {superseded['superseded_by']}")
        decision_id = ids.decision_id()
        with db.transaction() as conn:
            seq = plan_state.next_seq(conn, "decisions", repo_id)
            try:
                conn.execute(
                    "INSERT INTO decisions(decision_id, repository_id, seq, ref,"
                    " aspect, title, body, technical_note, created_at, created_by)"
                    " VALUES(?,?,?,?,?,?,?,?,?,?)",
                    (decision_id, repo_id, seq, ref, aspect, title, body,
                     technical_note, now_iso(), _actor(caller)))
            except sqlite3.IntegrityError as exc:
                raise ProtocolError("args_invalid",
                                    f"ref {ref!r} is already used in this"
                                    " repository") from exc
            if superseded is not None:
                conn.execute("UPDATE decisions SET superseded_by=? WHERE decision_id=?"
                             " AND superseded_by IS NULL",
                             (decision_id, superseded["decision_id"]))
        return {"decision_id": decision_id, "seq": seq, "ref": ref,
                "unsummarized_count": plan_state.unsummarized_count(db, repo_id),
                "summary_due": plan_state.summary_due(db, repo_id)}

    def decision_tail(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"path", "repository_id", "aspect", "n", "before_seq"})
        repo = _repository(args, caller)
        repo_id = repo["repository_id"]
        aspect = _enum(args, "aspect", ASPECTS, required=False)
        n = _int_arg(args, "n", 1, plan_state.TAIL_MAX) or plan_state.TAIL_DEFAULT
        before = _int_arg(args, "before_seq", 1, 1_000_000_000)
        decisions, has_more = plan_state.decision_tail(db, repo_id, aspect, n, before)
        return {"repository_id": repo_id, "display_name": repo["display_name"],
                "summary": plan_state.latest_summary(db, repo_id),
                "decisions": decisions, "has_more": has_more,
                "unsummarized_count": plan_state.unsummarized_count(db, repo_id),
                "summary_due": plan_state.summary_due(db, repo_id)}

    def decision_search(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"path", "repository_id", "query", "aspect", "n"})
        repo = _repository(args, caller)
        query = _plain_text(args, "query", required=True, lo=1, hi=200)
        aspect = _enum(args, "aspect", ASPECTS, required=False)
        n = _int_arg(args, "n", 1, plan_state.TAIL_MAX) or plan_state.TAIL_DEFAULT
        decisions, has_more = plan_state.decision_search(
            db, repo["repository_id"], query, aspect, n)
        return {"repository_id": repo["repository_id"], "query": query,
                "decisions": decisions, "has_more": has_more}

    def decision_summarize(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        _only(args, {"path", "repository_id", "body", "covers_through_seq"})
        repo = _repository(args, caller)
        repo_id = repo["repository_id"]
        body = _plain_text(args, "body", required=True, lo=10, hi=16000)
        covers = _int_arg(args, "covers_through_seq", 1, 1_000_000_000)
        if covers is None:
            raise ProtocolError("args_invalid",
                                "'covers_through_seq' (integer) is required")
        max_seq = plan_state.max_decision_seq(db, repo_id)
        if covers > max_seq:
            raise ProtocolError("args_invalid",
                                f"covers_through_seq {covers} is beyond the latest"
                                f" decision ({max_seq})")
        covered = plan_state.covered_through(db, repo_id)
        if covers <= covered:
            raise ProtocolError("args_invalid",
                                f"decisions through {covered} are already summarized;"
                                " cover a later sequence")
        with db.transaction() as conn:
            conn.execute(
                "INSERT INTO decision_summaries(repository_id, covers_through_seq,"
                " body, created_at, created_by) VALUES(?,?,?,?,?)",
                (repo_id, covers, body, now_iso(), _actor(caller)))
        return {"repository_id": repo_id, "covers_through_seq": covers,
                "unsummarized_count": plan_state.unsummarized_count(db, repo_id),
                "summary_due": plan_state.summary_due(db, repo_id)}

    return {
        "plan.overview": plan_overview,
        "task.create": task_create, "task.update": task_update,
        "task.history": task_history,
        "release.create": release_create, "release.update": release_update,
        "release.request": release_request, "release.deliver": release_deliver,
        "decision.record": decision_record, "decision.tail": decision_tail,
        "decision.search": decision_search, "decision.summarize": decision_summarize,
    }
