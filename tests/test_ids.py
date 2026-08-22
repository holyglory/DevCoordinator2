import re
from pathlib import Path

from devcoordinator2 import ids


def test_repository_id_deterministic(tmp_path: Path):
    a = ids.repository_id(tmp_path)
    b = ids.repository_id(tmp_path)
    assert a == b
    assert re.fullmatch(r"r[0-9a-f]{16}", a)


def test_symlinked_path_yields_same_id(tmp_path: Path):
    real = tmp_path / "repo"
    real.mkdir()
    link = tmp_path / "link"
    link.symlink_to(real)
    assert ids.repository_id(real) == ids.repository_id(link)


def test_prefixes_disjoint(tmp_path: Path):
    assert ids.repository_id(tmp_path)[0] == "r"
    assert ids.worktree_id(tmp_path)[0] == "w"
    assert ids.run_id()[0] == "t"
    assert ids.repository_id(tmp_path)[1:] != ids.worktree_id(tmp_path)[1:]


def test_run_id_shape_and_uniqueness():
    a, b = ids.run_id(), ids.run_id()
    assert re.fullmatch(r"t\d{8}T\d{6}Z-[0-9a-f]{6}", a)
    assert a != b


def test_unit_name_and_glob():
    name = ids.unit_name("devcoordinator2-test", "wabc", "0001")
    assert name == "devcoordinator2-test-wabc-0001.service"
    glob = ids.unit_glob("devcoordinator2-test", "wabc")
    assert glob == "devcoordinator2-test-wabc-*.service"
