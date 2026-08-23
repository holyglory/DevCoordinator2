from pathlib import Path

import pytest

from devcoordinator2.daemon.repoconfig import ConfigError, load_test_spec

GOOD = """
schema = 1
[test]
default = "unit"
[test.unit]
command = ["echo", "hello"]
timeout_seconds = 30
env = { CI = "1" }
[test.slow]
command = ["sleep", "5"]
cwd = "sub"
"""


def write(tmp_path: Path, text: str) -> Path:
    (tmp_path / ".devcoordinator.toml").write_text(text)
    return tmp_path


def test_good_default_and_named(tmp_path):
    (tmp_path / "sub").mkdir()
    root = write(tmp_path, GOOD)
    spec = load_test_spec(root, None)
    assert spec.name == "unit"
    assert spec.command == ("echo", "hello")
    assert spec.timeout_seconds == 30
    assert spec.env == {"CI": "1"}
    slow = load_test_spec(root, "slow")
    assert slow.cwd == (tmp_path / "sub").resolve()
    assert slow.timeout_seconds == 600


def test_single_test_needs_no_default(tmp_path):
    root = write(tmp_path, 'schema = 1\n[test.only]\ncommand = ["true"]\n')
    assert load_test_spec(root, None).name == "only"


@pytest.mark.parametrize("text,fragment", [
    ("", "'schema' must be 1"),  # empty file fails schema check
    ("schema = 2\n[test.u]\ncommand=[\"x\"]", "'schema' must be 1"),
    ("schema = 1\n", "[test.<name>] section is required"),
    ('schema = 1\n[test.u]\ncommand = "sh -c evil"\n', "never a shell string"),
    ('schema = 1\n[test.u]\ncommand = []\n', "non-empty array"),
    ('schema = 1\n[test.u]\ncommand = ["x"]\ncwd = "/etc"\n', "repository-relative"),
    ('schema = 1\n[test.u]\ncommand = ["x"]\ncwd = "../out"\n', "escapes"),
    ('schema = 1\n[test.u]\ncommand = ["x"]\ntimeout_seconds = 0\n', "timeout_seconds"),
    ('schema = 1\n[test.u]\ncommand = ["x"]\ntimeout_seconds = 99999\n', "timeout_seconds"),
    ('schema = 1\n[test.u]\ncommand = ["x"]\nretry = 3\n', "unknown keys"),
    ('schema = 1\nqueue = true\n[test.u]\ncommand = ["x"]\n', "unknown top-level"),
    ('schema = 1\n[test.u]\ncommand = ["x"]\nenv = { API_TOKEN = "abc123" }\n', "secret"),
    ('schema = 1\n[test]\ndefault = "nope"\n[test.u]\ncommand = ["x"]\n', "not defined"),
    ('schema = 1\n[test.BadName]\ncommand = ["x"]\n', "invalid test name"),
])
def test_rejections(tmp_path, text, fragment):
    root = write(tmp_path, text)
    with pytest.raises(ConfigError) as excinfo:
        load_test_spec(root, None)
    assert fragment.lower() in str(excinfo.value).lower()


def test_missing_file(tmp_path):
    with pytest.raises(ConfigError, match="not found"):
        load_test_spec(tmp_path, None)


def test_symlink_cwd_escape(tmp_path):
    outside = tmp_path / "outside"
    outside.mkdir()
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "link").symlink_to(outside)
    write(repo, 'schema = 1\n[test.u]\ncommand = ["x"]\ncwd = "link"\n')
    with pytest.raises(ConfigError, match="escapes"):
        load_test_spec(repo, None)


def test_unknown_test_name(tmp_path):
    root = write(tmp_path, 'schema = 1\n[test.u]\ncommand = ["x"]\n')
    with pytest.raises(ConfigError, match="not defined"):
        load_test_spec(root, "missing")


def test_postgres_section_defaults_and_overrides(tmp_path):
    root = write(tmp_path, 'schema = 1\n[test.u]\ncommand = ["x"]\n'
                           '[test.u.postgres]\n')
    spec = load_test_spec(root, None)
    assert spec.postgres is not None
    assert spec.postgres.image == "postgres:16-alpine"
    assert spec.postgres.database == "test"
    root = write(tmp_path, 'schema = 1\n[test.u]\ncommand = ["x"]\n'
                           '[test.u.postgres]\nimage = "postgres:17.10-alpine"\n'
                           'database = "app_db"\nuser = "app"\n')
    spec = load_test_spec(root, None)
    assert spec.postgres.image == "postgres:17.10-alpine"
    assert spec.postgres.database == "app_db"


@pytest.mark.parametrize("body,fragment", [
    ('image = "mysql:8"', "official 'postgres:<tag>'"),
    ('image = "evil/postgres:16"', "official 'postgres:<tag>'"),
    ('database = "Bad-Name"', "database must match"),
    ('user = "1abc"', "user must match"),
    ('persistent = true', "unknown keys"),
])
def test_postgres_section_rejections(tmp_path, body, fragment):
    root = write(tmp_path, 'schema = 1\n[test.u]\ncommand = ["x"]\n'
                           f'[test.u.postgres]\n{body}\n')
    with pytest.raises(ConfigError) as excinfo:
        load_test_spec(root, None)
    assert fragment in str(excinfo.value)
