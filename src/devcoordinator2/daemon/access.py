"""Public users, invitations, deployment grants, and role enforcement.

Local Unix-socket callers are trusted and unrestricted. A public identity
exists only when the configured edge asserts it (server.py enforces the peer
uid). Roles are exactly access < viewer < operator < administrator; grants
bind to immutable deployment IDs; revocation is effective on the next
request because every change republishes the route document and the policy
is evaluated per request from the database."""

from __future__ import annotations

import json
import secrets
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from typing import Any

from devcoordinator2.daemon import events, routes
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.server import Caller, Handler
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

ROLES = ("access", "viewer", "operator", "administrator")
RANK = {r: i for i, r in enumerate(ROLES)}
INVITATION_DAYS = 14


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


@dataclass
class Principal:
    local: bool
    identity: str | None = None
    user_id: str | None = None
    administrator: bool = False
    grants: dict[str, str] = field(default_factory=dict)  # deployment_id -> role

    def role_for(self, deployment_id: str) -> str | None:
        if self.local or self.administrator:
            return "administrator"
        return self.grants.get(deployment_id)

    def at_least(self, deployment_id: str, role: str) -> bool:
        have = self.role_for(deployment_id)
        return have is not None and RANK[have] >= RANK[role]


class Access:
    def __init__(self, config: InstanceConfig, db: Database):
        self._config = config
        self._db = db
        self.db = db
        self._bootstrap()

    # -- principals ----------------------------------------------------------

    def _bootstrap(self) -> None:
        for email in self._config.admin_emails:
            if not self._db.query("SELECT 1 FROM users WHERE email=?", (email,)):
                with self._db.transaction() as conn:
                    conn.execute(
                        "INSERT INTO users(user_id, email, administrator, created_at,"
                        " created_by) VALUES(?,?,1,?,'instance-configuration')",
                        ("u" + secrets.token_hex(8), email, _now()))

    def principal(self, caller: Caller) -> Principal:
        if caller.identity is None:
            return Principal(local=True)
        rows = self._db.query("SELECT * FROM users WHERE email=?", (caller.identity,))
        if not rows:
            return Principal(local=False, identity=caller.identity)
        user = rows[0]
        grants = {r["deployment_id"]: r["role"] for r in self._db.query(
            "SELECT deployment_id, role FROM grants WHERE user_id=?", (user["user_id"],))}
        with self._db.transaction() as conn:
            conn.execute("UPDATE users SET last_seen_at=? WHERE user_id=?",
                         (_now(), user["user_id"]))
        return Principal(local=False, identity=caller.identity, user_id=user["user_id"],
                         administrator=bool(user["administrator"]), grants=grants)

    # -- administration -------------------------------------------------------

    def list_users(self) -> dict:
        users = [dict(r) for r in self._db.query("SELECT * FROM users ORDER BY email")]
        grants = [dict(r) for r in self._db.query("SELECT * FROM grants")]
        for u in users:
            u["administrator"] = bool(u["administrator"])
            u["grants"] = [{"deployment_id": g["deployment_id"], "role": g["role"],
                            "granted_at": g["granted_at"]}
                           for g in grants if g["user_id"] == u["user_id"]]
        invitations = [dict(r) for r in self._db.query(
            "SELECT * FROM invitations ORDER BY created_at")]
        for i in invitations:
            i["administrator"] = bool(i["administrator"])
            i["grants"] = json.loads(i.pop("grants_json"))
        return {"users": users, "invitations": invitations,
                "roles": list(ROLES), "owners": self._owners()}

    def invite(self, email: str, administrator: bool, grants: list[dict], by: str) -> dict:
        email = _email(email)
        for g in grants:
            _validate_grant(g)
        if self._db.query("SELECT 1 FROM users WHERE email=?", (email,)):
            raise ProtocolError("args_invalid", f"{email} is already a user")
        expires = (datetime.now(UTC) + timedelta(days=INVITATION_DAYS)
                   ).strftime("%Y-%m-%dT%H:%M:%SZ")
        invitation_id = "i" + secrets.token_hex(8)
        with self._db.transaction() as conn:
            conn.execute("DELETE FROM invitations WHERE email=?", (email,))
            conn.execute(
                "INSERT INTO invitations(invitation_id, email, administrator, grants_json,"
                " created_at, created_by, expires_at) VALUES(?,?,?,?,?,?,?)",
                (invitation_id, email, int(administrator), json.dumps(grants), _now(), by,
                 expires))
        events.publish("user.invited", email=email, administrator=administrator, by=by)
        return {"invitation_id": invitation_id, "email": email, "expires_at": expires}

    def accept_invitation(self, email: str, subject: str | None,
                          display_name: str | None) -> dict:
        """Called by the edge after a verified sign-in. Admits exactly the
        invited identity; anything else is refused."""
        email = _email(email)
        existing = self._db.query("SELECT * FROM users WHERE email=?", (email,))
        if existing:
            return {"email": email, "user_id": existing[0]["user_id"], "accepted": False,
                    "administrator": bool(existing[0]["administrator"])}
        rows = self._db.query("SELECT * FROM invitations WHERE email=?", (email,))
        if not rows:
            raise ProtocolError("permission_denied", "no invitation for this identity")
        inv = rows[0]
        if inv["expires_at"] < _now():
            with self._db.transaction() as conn:
                conn.execute("DELETE FROM invitations WHERE email=?", (email,))
            raise ProtocolError("permission_denied", "invitation expired")
        user_id = "u" + secrets.token_hex(8)
        with self._db.transaction() as conn:
            conn.execute(
                "INSERT INTO users(user_id, email, subject, display_name, administrator,"
                " created_at, created_by, last_seen_at) VALUES(?,?,?,?,?,?,?,?)",
                (user_id, email, subject, display_name, inv["administrator"], _now(),
                 inv["created_by"], _now()))
            for g in json.loads(inv["grants_json"]):
                conn.execute(
                    "INSERT OR REPLACE INTO grants(user_id, deployment_id, role, granted_at,"
                    " granted_by) VALUES(?,?,?,?,?)",
                    (user_id, g["deployment_id"], g["role"], _now(), inv["created_by"]))
            conn.execute("DELETE FROM invitations WHERE email=?", (email,))
        self.republish()
        events.publish("user.accepted", email=email, administrator=bool(inv["administrator"]))
        return {"email": email, "user_id": user_id, "accepted": True,
                "administrator": bool(inv["administrator"])}

    def remove_user(self, email: str, by: str) -> dict:
        email = _email(email)
        rows = self._db.query("SELECT user_id FROM users WHERE email=?", (email,))
        with self._db.transaction() as conn:
            if rows:
                conn.execute("DELETE FROM grants WHERE user_id=?", (rows[0]["user_id"],))
                conn.execute("DELETE FROM users WHERE user_id=?", (rows[0]["user_id"],))
            removed_invite = conn.execute("DELETE FROM invitations WHERE email=?",
                                          (email,)).rowcount
        if not rows and not removed_invite:
            raise ProtocolError("user_not_found", f"no user or invitation for {email}")
        self.republish()
        events.publish("user.removed", email=email, by=by)
        return {"email": email, "removed_user": bool(rows),
                "removed_invitation": bool(removed_invite)}

    def set_grant(self, email: str, deployment_id: str, role: str, by: str) -> dict:
        email = _email(email)
        _validate_grant({"deployment_id": deployment_id, "role": role})
        rows = self._db.query("SELECT user_id FROM users WHERE email=?", (email,))
        if not rows:
            raise ProtocolError("user_not_found", f"no user {email}")
        if not self._db.query("SELECT 1 FROM deployments WHERE deployment_id=?",
                              (deployment_id,)):
            raise ProtocolError("deployment_not_found", f"no deployment {deployment_id}")
        with self._db.transaction() as conn:
            conn.execute(
                "INSERT OR REPLACE INTO grants(user_id, deployment_id, role, granted_at,"
                " granted_by) VALUES(?,?,?,?,?)",
                (rows[0]["user_id"], deployment_id, role, _now(), by))
        self.republish()
        events.publish("grant.set", email=email, deployment_id=deployment_id, role=role, by=by)
        return {"email": email, "deployment_id": deployment_id, "role": role}

    def remove_grant(self, email: str, deployment_id: str, by: str) -> dict:
        email = _email(email)
        rows = self._db.query("SELECT user_id FROM users WHERE email=?", (email,))
        if not rows:
            raise ProtocolError("user_not_found", f"no user {email}")
        with self._db.transaction() as conn:
            removed = conn.execute("DELETE FROM grants WHERE user_id=? AND deployment_id=?",
                                   (rows[0]["user_id"], deployment_id)).rowcount
        self.republish()
        events.publish("grant.removed", email=email, deployment_id=deployment_id, by=by)
        return {"email": email, "deployment_id": deployment_id, "removed": bool(removed)}

    # -- route document access section ---------------------------------------

    def _owners(self) -> list[str]:
        return [r["email"] for r in self._db.query(
            "SELECT email FROM users WHERE administrator=1 ORDER BY email")]

    def access_section(self) -> dict:
        grants = [{"identity": r["email"], "deployment_id": r["deployment_id"],
                   "role": r["role"]} for r in self._db.query(
            "SELECT u.email, g.deployment_id, g.role FROM grants g JOIN users u"
            " ON u.user_id=g.user_id ORDER BY u.email, g.deployment_id")]
        return {"owners": self._owners(), "grants": grants}

    def republish(self) -> None:
        routes.publish(self._db, self._config.routes_path, self._config.base_domain,
                       access=self.access_section())


def _email(value) -> str:
    if not isinstance(value, str) or "@" not in value or len(value) > 254:
        raise ProtocolError("args_invalid", "'email' must be an e-mail address")
    return value.strip().lower()


def _validate_grant(g) -> None:
    if not isinstance(g, dict) or set(g) != {"deployment_id", "role"} \
            or g["role"] not in ROLES or not isinstance(g["deployment_id"], str) \
            or not g["deployment_id"].startswith("d"):
        raise ProtocolError("args_invalid",
                            "grant must be {deployment_id: 'd…', role: access|viewer|"
                            "operator|administrator}")


# -- policy: wrap handlers for public principals ----------------------------

_ADMIN_ONLY_PREFIXES = ("test.", "repository.", "user.", "invitation.", "grant.",
                        "health.summary", "health.containers", "deployment.apply",
                        "deployment.rollback", "deployment.remove")
_OPERATOR = ("deployment.start", "deployment.stop", "deployment.restart")
_VIEWER = ("deployment.status", "deployment.logs", "health.repository", "health.history")
# Commands that enforce their own scope for admitted public users.
_SELF_GUARDED = ("telegram.link", "telegram.subscribe", "telegram.unsubscribe",
                 "telegram.list", "bug.report", "bug.list", "bug.close")


def guard(handlers: dict[str, Handler], access: Access,
          db: Database) -> dict[str, Handler]:
    """Return handlers that enforce roles for public identities and pass
    local callers through untouched."""
    guarded: dict[str, Handler] = {}
    for command, handler in handlers.items():
        guarded[command] = _wrap(command, handler, access, db)
    return guarded


def _wrap(command: str, handler: Handler, access: Access, db: Database) -> Handler:
    def wrapped(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        principal = access.principal(caller)
        if principal.local or principal.administrator:
            return handler(args, caller)
        if command in ("user.whoami", "ping"):
            return handler(args, caller)
        if command == "user.accept_invitation":
            # Only the signed-in identity itself can accept, via the edge.
            email = str(args.get("email", "")).strip().lower()
            if caller.client_kind != "edge" or email != principal.identity:
                raise ProtocolError("permission_denied",
                                    "acceptance is performed by the edge for the signed-in"
                                    " identity")
            return handler(args, caller)
        if principal.user_id is None:
            raise ProtocolError("permission_denied", "identity is not an admitted user")
        if command in _SELF_GUARDED:
            return handler(args, caller)
        if command.startswith(_ADMIN_ONLY_PREFIXES):
            raise ProtocolError("permission_denied", f"{command} requires administrator")
        if command in _OPERATOR or command in _VIEWER:
            dep_id = _deployment_for(command, args, db)
            needed = "operator" if command in _OPERATOR else "viewer"
            if dep_id is None or not principal.at_least(dep_id, needed):
                raise ProtocolError("permission_denied", f"{command} requires {needed} on "
                                                         "the deployment")
            return handler(args, caller)
        if command == "deployment.list":
            result = handler({}, caller)
            allowed = {d for d, r in principal.grants.items() if RANK[r] >= RANK["viewer"]}
            result["deployments"] = [d for d in result["deployments"]
                                     if d["deployment_id"] in allowed]
            result["declared"] = []
            return result
        if command == "health.repositories":
            result = handler(args, caller)
            allowed = {d for d, r in principal.grants.items() if RANK[r] >= RANK["viewer"]}
            repos = set()
            for r in db.query("SELECT deployment_id, repository_id FROM deployments"):
                if r["deployment_id"] in allowed:
                    repos.add(r["repository_id"])
            rows = []
            for row in result["repositories"]:
                if row["repository_id"] in repos:
                    row["deployments"] = [d for d in row["deployments"]
                                          if d["deployment_id"] in allowed]
                    rows.append(row)
            return {"repositories": rows}
        raise ProtocolError("permission_denied", f"{command} is not available to public users")
    return wrapped


def _deployment_for(command: str, args: dict[str, Any], db: Database) -> str | None:
    if command in ("health.repository",):
        return None  # resolved by repository below
    if command == "health.history":
        sid = args.get("subject_id", "")
        kind = args.get("subject_kind")
        if kind == "deployment":
            return sid
        if kind == "component" and "/" in sid:
            return sid.split("/", 1)[0]
        return None
    dep_id = args.get("deployment_id")
    if isinstance(dep_id, str):
        return dep_id
    return None


def public_commands(access: Access) -> dict[str, Handler]:
    def whoami(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        p = access.principal(caller)
        return {"local": p.local, "identity": p.identity, "user_id": p.user_id,
                "administrator": p.administrator or p.local, "grants": p.grants}

    def user_list(args, caller):
        _none(args)
        return access.list_users()

    def user_invite(args, caller):
        unknown = set(args) - {"email", "administrator", "grants"}
        if unknown:
            raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
        admin = args.get("administrator", False)
        grants = args.get("grants", [])
        if not isinstance(admin, bool) or not isinstance(grants, list):
            raise ProtocolError("args_invalid", "administrator: bool, grants: list")
        return access.invite(args.get("email"), admin, grants, _by(caller))

    def user_remove(args, caller):
        _only(args, {"email"})
        return access.remove_user(args.get("email"), _by(caller))

    def user_accept(args, caller):
        _only(args, {"email", "subject", "display_name"})
        if caller.identity is None and caller.client_kind != "edge" and not _is_local(caller):
            raise ProtocolError("permission_denied", "acceptance is performed by the edge")
        return access.accept_invitation(args.get("email"), args.get("subject"),
                                        args.get("display_name"))

    def grant_set(args, caller):
        _only(args, {"email", "deployment_id", "role"})
        return access.set_grant(args.get("email"), args.get("deployment_id", ""),
                                args.get("role", ""), _by(caller))

    def grant_remove(args, caller):
        _only(args, {"email", "deployment_id"})
        return access.remove_grant(args.get("email"), args.get("deployment_id", ""),
                                   _by(caller))

    return {"user.whoami": whoami, "user.list": user_list, "user.invite": user_invite,
            "user.remove": user_remove, "user.accept_invitation": user_accept,
            "grant.set": grant_set, "grant.remove": grant_remove}


def _none(args):
    if args:
        raise ProtocolError("args_invalid", f"unexpected args: {sorted(args)}")


def _only(args, allowed: set[str]):
    unknown = set(args) - allowed
    if unknown:
        raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")


def _by(caller: Caller) -> str:
    return caller.identity or f"uid:{caller.uid}"


def _is_local(caller: Caller) -> bool:
    return caller.identity is None

