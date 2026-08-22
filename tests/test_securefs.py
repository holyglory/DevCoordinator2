import os
from pathlib import Path

import pytest

from devcoordinator2.daemon import securefs


def test_create_and_remove(tmp_path: Path):
    current = securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    assert (current / "artifacts").is_dir()
    assert (current / "scratch").is_dir()
    (current / "stdout.log").write_text("x")
    securefs.remove_test_dir(tmp_path)
    assert not current.exists()
    assert (tmp_path / ".devcoordinator" / "test").is_dir()  # only current/ deleted


def test_create_refuses_existing_current(tmp_path: Path):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    with pytest.raises(securefs.SecureFsError, match="prior test directory"):
        securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())


def test_symlinked_devcoordinator_rejected(tmp_path: Path):
    victim = tmp_path / "victim"
    victim.mkdir()
    (victim / "precious.txt").write_text("keep me")
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".devcoordinator").symlink_to(victim)
    with pytest.raises(securefs.SecureFsError):
        securefs.create_test_dir(repo, os.getuid(), os.getgid())
    assert (victim / "precious.txt").exists()


def test_symlinked_current_not_followed_on_remove(tmp_path: Path):
    victim = tmp_path / "victim"
    victim.mkdir()
    (victim / "precious.txt").write_text("keep me")
    repo = tmp_path / "repo"
    (repo / ".devcoordinator" / "test").mkdir(parents=True)
    (repo / ".devcoordinator" / "test" / "current").symlink_to(victim)
    securefs.remove_test_dir(repo)
    # The symlink itself is unlinked; the target and its content survive.
    assert not (repo / ".devcoordinator" / "test" / "current").exists()
    assert (victim / "precious.txt").exists()


def test_symlink_inside_current_not_followed(tmp_path: Path):
    victim = tmp_path / "victim"
    victim.mkdir()
    (victim / "precious.txt").write_text("keep me")
    repo = tmp_path / "repo"
    repo.mkdir()
    current = securefs.create_test_dir(repo, os.getuid(), os.getgid())
    (current / "artifacts" / "trap").symlink_to(victim)
    securefs.remove_test_dir(repo)
    assert (victim / "precious.txt").exists()
    assert not current.exists()


def test_remove_missing_is_noop(tmp_path: Path):
    securefs.remove_test_dir(tmp_path)  # nothing exists; no error
