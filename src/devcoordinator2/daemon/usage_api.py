"""Repository-scoped Codex usage reads for the web Console."""

from __future__ import annotations

from typing import Any

from devcoordinator2.daemon.codex_usage import WINDOWS, CodexUsage
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller, Handler
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError


def _range(args: dict[str, Any], allowed: set[str]) -> str:
    unknown = set(args) - allowed
    if unknown:
        raise ProtocolError("args_invalid", f"unknown args: {sorted(unknown)}")
    value = args.get("range", "24h")
    if value not in WINDOWS:
        raise ProtocolError("args_invalid", "'range' must be 24h, 7d, or 30d")
    return value


def build_usage_handlers(config: InstanceConfig, db: Database, registry: Registry,
                         usage: CodexUsage | None = None) -> dict[str, Handler]:
    usage = usage or CodexUsage(config, db)

    def repositories(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        range_key = _range(args, {"range", "_repository_ids"})
        repository_ids = args.get("_repository_ids")
        if repository_ids is not None and (not isinstance(repository_ids, list)
                                           or not all(isinstance(item, str)
                                                      for item in repository_ids)):
            raise ProtocolError("args_invalid", "repository scope is invalid")
        records = registry.list_repositories()
        if repository_ids is not None:
            allowed = set(repository_ids)
            records = [record for record in records
                       if record["repository_id"] in allowed]
        return usage.repositories(records, range_key)

    def repository(args: dict[str, Any], caller: Caller) -> dict[str, Any]:
        range_key = _range(args, {"repository_id", "range"})
        repository_id = args.get("repository_id")
        if not isinstance(repository_id, str):
            raise ProtocolError("args_invalid", "'repository_id' is required")
        record = next((item for item in registry.list_repositories()
                       if item["repository_id"] == repository_id), None)
        if record is None:
            raise ProtocolError("repository_not_found", "no registered repository")
        return usage.repository(record, range_key)

    return {"usage.repositories": repositories, "usage.repository": repository}
