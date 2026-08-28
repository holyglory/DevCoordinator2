import os
import subprocess

import pytest

from devcoordinator2.daemon import deploy_engine
from devcoordinator2.daemon.deploy_config import ComponentSpec, DeploymentSpec
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError


class _Db:
    def query(self, _sql, _params=()):
        return [{"repository_id": "r" + "a" * 16}]


def _ctx(tmp_path, *, authorized=True):
    pair = frozenset({("r" + "a" * 16, "compose.env")}) if authorized else frozenset()
    config = InstanceConfig(
        socket_path=tmp_path / "daemon.sock",
        state_dir=tmp_path / "state",
        unit_prefix="test",
        slice_name="test.slice",
        client_group="",
        compose_env_authorizations=pair,
    )
    spec = DeploymentSpec(
        name="d", sources=("worktree",), domains={}, build=(), ttl_seconds=None)
    return deploy_engine.Ctx(
        config=config, db=_Db(), dep_id="d" + "1" * 16, spec=spec,
        source="worktree", caller_uid=os.getuid(), caller_gid=os.getgid(),
        client="codex", session=None)


def _component():
    return ComponentSpec(
        name="stack", type="compose", order=0, independent_control=True,
        depends_on=(), env={}, compose_files=("compose.yml",),
        compose_env_file="compose.env")


def test_compose_env_file_requires_private_instance_authority(monkeypatch, tmp_path):
    (tmp_path / "compose.env").write_text("VALUE=private\n")
    monkeypatch.setattr(
        deploy_engine, "_as_caller",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0))
    with pytest.raises(ProtocolError, match="not authorized"):
        _ctx(tmp_path, authorized=False).compose_env_files(_component(), tmp_path, 1)


def test_compose_env_file_must_remain_ignored(monkeypatch, tmp_path):
    (tmp_path / "compose.env").write_text("VALUE=private\n")
    monkeypatch.setattr(
        deploy_engine, "_as_caller",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 1))
    with pytest.raises(ProtocolError, match="must remain ignored"):
        _ctx(tmp_path).compose_env_files(_component(), tmp_path, 1)


def test_compose_env_file_rejects_symlink_even_inside_repository(monkeypatch, tmp_path):
    (tmp_path / "actual.env").write_text("VALUE=private\n")
    (tmp_path / "compose.env").symlink_to(tmp_path / "actual.env")
    monkeypatch.setattr(
        deploy_engine, "_as_caller",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0))
    with pytest.raises(ProtocolError, match="unsafe"):
        _ctx(tmp_path).compose_env_files(_component(), tmp_path, 1)


def test_compose_env_file_returns_only_validated_paths(monkeypatch, tmp_path):
    (tmp_path / "compose.env").write_text("VALUE=private\n")
    monkeypatch.setattr(
        deploy_engine, "_as_caller",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0))
    assert _ctx(tmp_path).compose_env_files(_component(), tmp_path, 1) == (
        (tmp_path / "compose.env").resolve(),)
