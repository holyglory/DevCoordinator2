import os
import pwd
from pathlib import Path

from devcoordinator2.daemon.systemd_unit import (
    build_systemd_run_argv,
    supplementary_groups,
)


def test_argv_shape_and_no_shell(tmp_path: Path):
    uid = os.getuid()
    entry = pwd.getpwuid(uid)
    argv = build_systemd_run_argv(
        unit="devcoordinator2-dev-w1-0001.service",
        slice_name="devcoordinator2-tests.slice",
        uid=uid, gid=entry.pw_gid, timeout_seconds=60,
        cwd=tmp_path, env_file=tmp_path / "env",
        command=("echo", "hello world; rm -rf /"),
        scratch_dir=tmp_path / "scratch",
    )
    assert argv[0] == "systemd-run"
    assert "--pipe" in argv
    assert f"--uid={uid}" in argv
    assert "--property=KillMode=control-group" in argv
    assert "--property=RuntimeMaxSec=60s" in argv
    assert "--property=NoNewPrivileges=yes" in argv
    assert f"--property=EnvironmentFile={tmp_path / 'env'}" in argv
    assert not any(a.startswith("--setenv=CI") for a in argv)
    # The command stays an argv tail after "--"; shell metacharacters inert.
    sep = argv.index("--")
    assert argv[sep + 1:] == ["echo", "hello world; rm -rf /"]


def test_supplementary_groups_exclude_root():
    groups = supplementary_groups(os.getuid())
    assert 0 not in groups
    assert groups == sorted(set(groups))
