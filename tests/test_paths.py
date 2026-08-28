import json
import os
from pathlib import Path

import pytest

from devcoordinator2 import paths


def test_defaults(monkeypatch):
    for key in list(paths._DEFAULTS):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv("DEVCOORDINATOR2_INSTANCE_ENV", "/nonexistent/instance.env")
    cfg = paths.load_instance_config()
    assert cfg.socket_path == Path("/run/devcoordinator2/daemon.sock")
    assert cfg.database_path == Path("/var/lib/devcoordinator2/authority.sqlite3")
    assert cfg.unit_prefix == "devcoordinator2-test"


def test_env_overrides_file(monkeypatch, tmp_path):
    env_file = tmp_path / "instance.env"
    env_file.write_text(
        "# comment\n"
        "DEVCOORDINATOR2_SOCKET=/from/file.sock\n"
        'DEVCOORDINATOR2_STATE_DIR="/from/file-state"\n'
        "UNRELATED=ignored\n"
    )
    monkeypatch.setenv("DEVCOORDINATOR2_INSTANCE_ENV", str(env_file))
    monkeypatch.setenv("DEVCOORDINATOR2_SOCKET", "/from/env.sock")
    monkeypatch.delenv("DEVCOORDINATOR2_STATE_DIR", raising=False)
    cfg = paths.load_instance_config()
    assert cfg.socket_path == Path("/from/env.sock")
    assert cfg.state_dir == Path("/from/file-state")


def test_test_dir(tmp_path):
    assert paths.test_dir(tmp_path) == tmp_path / ".devcoordinator" / "test" / "current"


def test_compose_env_allowlist_loads_exact_repository_path_pairs(monkeypatch, tmp_path):
    allowlist = tmp_path / "allowlist.json"
    allowlist.write_text(json.dumps({
        "schema": 1,
        "authorizations": [{
            "repository_id": "r" + "a" * 16,
            "path": "deploy/v3/env/dev.env",
        }],
    }))
    os.chmod(allowlist, 0o600)
    monkeypatch.setenv("DEVCOORDINATOR2_INSTANCE_ENV", "/nonexistent")
    monkeypatch.setenv("DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE", str(allowlist))
    cfg = paths.load_instance_config()
    assert cfg.compose_env_authorized(
        "r" + "a" * 16, "deploy/v3/env/dev.env")
    assert not cfg.compose_env_authorized(
        "r" + "b" * 16, "deploy/v3/env/dev.env")


@pytest.mark.parametrize("document", [
    {"schema": 2, "authorizations": []},
    {"schema": 1, "authorizations": [{"repository_id": "bad", "path": "x.env"}]},
    {"schema": 1, "authorizations": [{
        "repository_id": "r" + "a" * 16, "path": "../x.env"}]},
    {"schema": 1, "authorizations": [{
        "repository_id": "r" + "a" * 16, "path": "/x.env"}]},
    {"schema": 1, "authorizations": [
        {"repository_id": "r" + "a" * 16, "path": "x.env"},
        {"repository_id": "r" + "a" * 16, "path": "x.env"},
    ]},
])
def test_compose_env_allowlist_rejects_malformed_authority(monkeypatch, tmp_path,
                                                            document):
    allowlist = tmp_path / "allowlist.json"
    allowlist.write_text(json.dumps(document))
    os.chmod(allowlist, 0o600)
    monkeypatch.setenv("DEVCOORDINATOR2_INSTANCE_ENV", "/nonexistent")
    monkeypatch.setenv("DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE", str(allowlist))
    with pytest.raises(ValueError, match="Compose environment"):
        paths.load_instance_config()


def test_compose_env_allowlist_rejects_writable_policy(monkeypatch, tmp_path):
    allowlist = tmp_path / "allowlist.json"
    allowlist.write_text('{"schema":1,"authorizations":[]}')
    os.chmod(allowlist, 0o622)
    monkeypatch.setenv("DEVCOORDINATOR2_INSTANCE_ENV", "/nonexistent")
    monkeypatch.setenv("DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE", str(allowlist))
    with pytest.raises(ValueError, match="writable"):
        paths.load_instance_config()
