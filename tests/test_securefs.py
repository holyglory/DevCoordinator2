import os
import stat
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


def test_test_history_is_atomic_bounded_metadata_outside_current(tmp_path: Path):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    payload = b'{"schema":1,"runs":[]}\n'
    securefs.write_test_history(tmp_path, payload, (os.getuid(), os.getgid()))
    assert securefs.read_test_history(tmp_path) == payload
    securefs.remove_test_dir(tmp_path)
    assert securefs.read_test_history(tmp_path) == payload


def test_test_history_refuses_symlink(tmp_path: Path):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    victim = tmp_path / "victim.json"
    victim.write_text("keep me")
    history = tmp_path / ".devcoordinator" / "test" / "history.json"
    history.symlink_to(victim)
    with pytest.raises(securefs.SecureFsError):
        securefs.read_test_history(tmp_path)
    securefs.write_test_history(
        tmp_path, b'{"schema":1,"runs":[]}\n',
        (os.getuid(), os.getgid()))
    assert history.is_file() and not history.is_symlink()
    assert victim.read_text() == "keep me"


def test_test_evidence_is_atomic_bounded_metadata_outside_current(tmp_path: Path):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    payload = b'{"schema":1,"runs":[]}\n'
    securefs.write_test_evidence(tmp_path, payload, (os.getuid(), os.getgid()))
    assert securefs.read_test_evidence(tmp_path) == payload
    securefs.remove_test_dir(tmp_path)
    assert securefs.read_test_evidence(tmp_path) == payload


def test_test_output_tail_never_follows_a_caller_symlink(tmp_path: Path):
    current = securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    check = current / "checks" / "unit"
    check.mkdir(parents=True)
    (check / "stdout.log").write_bytes(b"abcdef")
    assert securefs.tail_test_file(
        tmp_path, ("checks", "unit", "stdout.log"), 3) == (b"def", True)
    victim = tmp_path / "private"
    victim.write_text("must not be returned")
    (check / "stdout.log").unlink()
    (check / "stdout.log").symlink_to(victim)
    with pytest.raises(securefs.SecureFsError):
        securefs.tail_test_file(
            tmp_path, ("checks", "unit", "stdout.log"), 65536)


def test_stable_log_run_is_private_and_removed_by_exact_id(tmp_path: Path):
    current = securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    run_id = "t20260902T120000Z-123abc"
    run = securefs.create_test_log_run_dir(
        tmp_path, run_id, os.getuid(), os.getgid())
    assert run.is_dir()
    assert stat.S_IMODE(run.stat().st_mode) == 0o700
    assert stat.S_IMODE((run / "executor").stat().st_mode) == 0o700
    stdout, stderr = securefs.create_test_executor_log_files(
        tmp_path, run_id, os.getuid(), os.getgid())
    stdout.write(b"complete\n")
    stderr.write(b"diagnostic\n")
    stdout.close()
    stderr.close()
    assert stat.S_IMODE((run / "executor" / "stdout.log").stat().st_mode) == 0o600
    assert stat.S_IMODE((run / "executor" / "stderr.log").stat().st_mode) == 0o600
    with pytest.raises(securefs.SecureFsError):
        securefs.create_test_executor_log_files(
            tmp_path, run_id, os.getuid(), os.getgid())
    securefs.remove_test_dir(tmp_path)
    assert not current.exists()
    assert run.is_dir(), "current cleanup must preserve retained logs"
    securefs.remove_test_log_run_dir(tmp_path, run_id)
    assert not run.exists()


def test_log_run_creation_rejects_invalid_ids_and_symlinked_store(tmp_path: Path):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    with pytest.raises(securefs.SecureFsError, match="invalid"):
        securefs.create_test_log_run_dir(
            tmp_path, "../escape", os.getuid(), os.getgid())

    outside = tmp_path / "outside"
    outside.mkdir()
    logs = tmp_path / ".devcoordinator" / "test" / "logs"
    logs.symlink_to(outside, target_is_directory=True)
    with pytest.raises(securefs.SecureFsError):
        securefs.create_test_log_run_dir(
            tmp_path, "t20260902T120000Z-456def", os.getuid(), os.getgid())
    assert not any(outside.iterdir())


def test_executor_log_creation_does_not_follow_a_substituted_stream(tmp_path: Path):
    securefs.create_test_dir(tmp_path, os.getuid(), os.getgid())
    run_id = "t20260902T120000Z-789abc"
    run = securefs.create_test_log_run_dir(
        tmp_path, run_id, os.getuid(), os.getgid())
    outside = tmp_path / "outside.log"
    outside.write_bytes(b"keep")
    (run / "executor" / "stdout.log").symlink_to(outside)
    with pytest.raises(securefs.SecureFsError):
        securefs.create_test_executor_log_files(
            tmp_path, run_id, os.getuid(), os.getgid())
    assert outside.read_bytes() == b"keep"
