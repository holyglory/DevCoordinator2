from pathlib import Path

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
