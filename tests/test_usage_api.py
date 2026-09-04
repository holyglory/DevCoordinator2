from pathlib import Path

import pytest

from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Caller
from devcoordinator2.daemon.usage_api import build_usage_handlers
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError


def _caller() -> Caller:
    return Caller(pid=0, uid=1000, gid=1000, client_kind="other", client_session=None)


def test_usage_handlers_return_truthful_unavailable_rows(tmp_path: Path):
    config = InstanceConfig(
        socket_path=tmp_path / "s", state_dir=tmp_path / "state",
        unit_prefix="test", slice_name="test.slice", client_group="",
    )
    db = Database(config.database_path)
    with db.transaction() as conn:
        conn.execute("INSERT INTO repositories(repository_id,root_path,display_name,"
                     " registered_at,registered_by_uid,last_seen_at)"
                     " VALUES(?,?,?,?,?,?)", (
            "r0123456789abcdef", str(tmp_path / "repo"), "Example", "t", 1000, "t"))
    handlers = build_usage_handlers(config, db, Registry(db))

    overview = handlers["usage.repositories"]({}, _caller())
    detail = handlers["usage.repository"](
        {"repository_id": "r0123456789abcdef", "range": "7d"}, _caller())

    assert overview["range"] == "24h"
    assert overview["repositories"][0]["coverage"]["state"] == "unavailable"
    assert detail["range"] == "7d"
    assert detail["totals"]["total_tokens"] is None
    assert len(detail["series"]) == 28
    db.close()


def test_usage_handlers_validate_range_and_repository(tmp_path: Path):
    config = InstanceConfig(
        socket_path=tmp_path / "s", state_dir=tmp_path / "state",
        unit_prefix="test", slice_name="test.slice", client_group="",
    )
    db = Database(config.database_path)
    handlers = build_usage_handlers(config, db, Registry(db))

    with pytest.raises(ProtocolError, match="range"):
        handlers["usage.repositories"]({"range": "lifetime"}, _caller())
    with pytest.raises(ProtocolError, match="registered repository"):
        handlers["usage.repository"]({"repository_id": "r0"}, _caller())
    db.close()
