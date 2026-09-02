"""Canonical operation authorization and effect metadata.

The daemon, MCP adapter, and Console consume this vocabulary so a command's
authority and side-effect description cannot drift between surfaces.  The
registry is intentionally data-only; handler-specific target resolution stays
with the daemon authorization layer.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

Scope = Literal["public", "self", "server", "repository", "deployment"]
Role = Literal["anonymous", "self", "viewer", "operator", "administrator"]
Effect = Literal["read", "append", "reversible", "destructive", "external"]


@dataclass(frozen=True)
class OperationPolicy:
    scope: Scope
    role: Role
    effect: Effect
    idempotent: bool = False

    @property
    def read_only(self) -> bool:
        return self.effect == "read"

    @property
    def destructive(self) -> bool:
        return self.effect == "destructive"

    @property
    def open_world(self) -> bool:
        return self.effect == "external"

    def mcp_annotations(self) -> dict[str, bool]:
        """Return the standard MCP tool-effect hints for a trusted server."""

        return {
            "readOnlyHint": self.read_only,
            "destructiveHint": self.destructive,
            "idempotentHint": self.idempotent,
            "openWorldHint": self.open_world,
        }


def _policy(scope: Scope, role: Role, effect: Effect, *, idempotent: bool = False) \
        -> OperationPolicy:
    return OperationPolicy(scope=scope, role=role, effect=effect, idempotent=idempotent)


OPERATIONS: dict[str, OperationPolicy] = {
    "ping": _policy("public", "anonymous", "read", idempotent=True),
    "user.whoami": _policy("public", "anonymous", "read", idempotent=True),
    "user.accept_invitation": _policy("self", "self", "append"),
    "user.list": _policy("server", "administrator", "read", idempotent=True),
    "user.invite": _policy("server", "administrator", "append"),
    "user.remove": _policy("server", "administrator", "destructive", idempotent=True),
    "grant.set": _policy("deployment", "administrator", "reversible", idempotent=True),
    "grant.remove": _policy("deployment", "administrator", "destructive", idempotent=True),

    "repository.register": _policy("server", "administrator", "append", idempotent=True),
    "repository.list": _policy("server", "administrator", "read", idempotent=True),
    "repository.status": _policy("repository", "administrator", "read", idempotent=True),
    "repository.archive": _policy("server", "administrator", "reversible", idempotent=True),
    "repository.unarchive": _policy("server", "administrator", "reversible", idempotent=True),

    "test.start": _policy("repository", "administrator", "destructive"),
    "test.retry": _policy("repository", "administrator", "destructive"),
    "test.status": _policy("repository", "administrator", "read", idempotent=True),
    "test.log.catalog": _policy("repository", "administrator", "read", idempotent=True),
    "test.log.tail": _policy("repository", "administrator", "read", idempotent=True),
    "test.log.search": _policy("repository", "administrator", "read", idempotent=True),
    "test.log.range": _policy("repository", "administrator", "read", idempotent=True),
    "test.log.failure_context": _policy(
        "repository", "administrator", "read", idempotent=True),
    "test.log.retention.get": _policy(
        "server", "administrator", "read", idempotent=True),
    "test.log.retention.set": _policy(
        "server", "administrator", "destructive", idempotent=True),
    "test.stop": _policy("repository", "administrator", "destructive", idempotent=True),
    "test.list": _policy("server", "administrator", "read", idempotent=True),
    "test.capacity.get": _policy("server", "administrator", "read", idempotent=True),
    "test.capacity.set": _policy("server", "administrator", "reversible", idempotent=True),

    "deployment.list": _policy("deployment", "viewer", "read", idempotent=True),
    "deployment.status": _policy("deployment", "viewer", "read", idempotent=True),
    "deployment.logs": _policy("deployment", "viewer", "read", idempotent=True),
    "deployment.apply": _policy("deployment", "administrator", "reversible"),
    "deployment.rollback": _policy("deployment", "administrator", "reversible"),
    "deployment.start": _policy("deployment", "operator", "reversible", idempotent=True),
    "deployment.stop": _policy("deployment", "operator", "reversible", idempotent=True),
    "deployment.restart": _policy("deployment", "operator", "reversible"),
    "deployment.set_domain": _policy(
        "deployment", "administrator", "reversible", idempotent=True),
    "deployment.remove": _policy("deployment", "administrator", "destructive", idempotent=True),

    "health.summary": _policy("server", "administrator", "read", idempotent=True),
    "health.repositories": _policy("repository", "viewer", "read", idempotent=True),
    "health.repository": _policy("repository", "viewer", "read", idempotent=True),
    "health.history": _policy("repository", "viewer", "read", idempotent=True),
    "health.containers": _policy("server", "administrator", "read", idempotent=True),
    "health.container_remove": _policy(
        "server", "administrator", "destructive", idempotent=True),

    "plan.overview": _policy("repository", "viewer", "read", idempotent=True),
    "task.history": _policy("repository", "viewer", "read", idempotent=True),
    "task.create": _policy("repository", "administrator", "append"),
    "task.update": _policy("repository", "administrator", "destructive"),
    "release.create": _policy("repository", "administrator", "append"),
    "release.update": _policy("repository", "administrator", "destructive"),
    "release.request": _policy("repository", "administrator", "append"),
    "release.deliver": _policy("repository", "administrator", "append"),
    "decision.tail": _policy("repository", "viewer", "read", idempotent=True),
    "decision.search": _policy("repository", "viewer", "read", idempotent=True),
    "decision.record": _policy("repository", "administrator", "append"),
    "decision.summarize": _policy("repository", "administrator", "append"),

    "usage.repositories": _policy("repository", "operator", "read", idempotent=True),
    "usage.repository": _policy("repository", "operator", "read", idempotent=True),
    "progress.repositories": _policy("repository", "operator", "read", idempotent=True),
    "progress.repository": _policy("repository", "operator", "read", idempotent=True),

    "telegram.link": _policy("self", "self", "external"),
    "telegram.subscribe": _policy("self", "self", "external"),
    "telegram.unsubscribe": _policy("self", "self", "external", idempotent=True),
    "telegram.list": _policy("self", "self", "read", idempotent=True),
    "bug.report": _policy("self", "self", "append"),
    "bug.list": _policy("self", "self", "read", idempotent=True),
    "bug.close": _policy("self", "self", "destructive", idempotent=True),
}


def policy_for(command: str) -> OperationPolicy:
    try:
        return OPERATIONS[command]
    except KeyError as exc:
        raise KeyError(f"operation {command!r} has no authority/effect policy") from exc
