from pathlib import Path

import pytest

from devcoordinator2.daemon.repoconfig import (
    ConfigError,
    load_test_spec,
    validate_test_config,
)
from devcoordinator2.daemon.tests_lifecycle import TestLifecycle as GovernedTestLifecycle

GOOD = """
schema = 2
[test]
default = "unit"
[test.unit]
timeout_seconds = 30
env = { CI = "1" }
[[test.unit.check]]
name = "main"
tier = "release"
command = ["echo", "hello"]
[test.slow]
cwd = "sub"
[[test.slow.check]]
name = "main"
tier = "development"
command = ["sleep", "5"]
"""


def write(tmp_path: Path, text: str) -> Path:
    (tmp_path / ".devcoordinator.toml").write_text(text)
    return tmp_path


def test_good_default_and_named(tmp_path):
    (tmp_path / "sub").mkdir()
    root = write(tmp_path, GOOD)
    spec = load_test_spec(root, None)
    assert spec.name == "unit"
    assert spec.checks[0].command == ("echo", "hello")
    assert spec.timeout_seconds == 30
    assert spec.env == {"CI": "1"}
    slow = load_test_spec(root, "slow")
    assert slow.cwd == (tmp_path / "sub").resolve()
    assert slow.timeout_seconds == 600
    assert [(row.name, row.tiers, row.default)
            for row in validate_test_config(root)] == [
        ("unit", ("release",), True),
        ("slow", ("development",), False),
    ]


def test_single_test_needs_no_default(tmp_path):
    root = write(
        tmp_path,
        'schema = 2\n[test.only]\n[[test.only.check]]\nname="main"\n'
        'tier = "release"\ncommand = ["true"]\n')
    assert load_test_spec(root, None).name == "only"


def test_invalid_nondefault_target_blocks_the_valid_default(tmp_path):
    root = write(tmp_path, '''
schema=2
[test]
default="unit"
[test.unit]
[[test.unit.check]]
name="main"
tier="release"
command=["true"]
[test.hidden]
[[test.hidden.check]]
name="main"
command=["false"]
''')
    with pytest.raises(ConfigError, match=r"hidden.*tier"):
        load_test_spec(root, "unit")
    with pytest.raises(ConfigError, match=r"hidden.*tier"):
        validate_test_config(root)


@pytest.mark.parametrize("text,fragment", [
    ("", "'schema' must be 2"),  # empty file fails schema check
    ("schema = 1\n[test.u]\ncommand=[\"x\"]", "schema 1 is no longer supported"),
    ("schema = 2\n", "[test.<name>] section is required"),
    ('schema = 2\n[test.u]\ncommand=["x"]\ntier="release"\n',
     "rejects direct test commands"),
    ('schema = 2\n[test.u]\n[[test.u.check]]\nname="main"\ntier="release"\n'
     'command = "sh -c evil"\n', "never a shell string"),
    ('schema = 2\n[test.u]\n[[test.u.check]]\nname="main"\ntier="release"\n'
     'command = []\n', "1..256"),
    ('schema = 2\n[test.u]\ncwd="/etc"\n[[test.u.check]]\nname="main"\n'
     'tier="release"\ncommand=["x"]\n',
     "repository-relative"),
    ('schema = 2\n[test.u]\ncwd="../out"\n[[test.u.check]]\nname="main"\n'
     'tier="release"\ncommand=["x"]\n',
     "escapes"),
    ('schema = 2\n[test.u]\ntimeout_seconds=0\n[[test.u.check]]\nname="main"\n'
     'tier="release"\ncommand=["x"]\n',
     "timeout_seconds"),
    ('schema = 2\n[test.u]\ntimeout_seconds=99999\n[[test.u.check]]\nname="main"\n'
     'tier="release"\ncommand=["x"]\n',
     "timeout_seconds"),
    ('schema = 2\n[test.u]\nretry=3\n[[test.u.check]]\nname="main"\n'
     'tier="release"\ncommand=["x"]\n',
     "unknown keys"),
    ('schema = 2\nqueue=true\n[test.u]\n[[test.u.check]]\nname="main"\n'
     'tier="release"\ncommand=["x"]\n',
     "unknown top-level"),
    ('schema = 2\n[test.u]\nenv={API_TOKEN="abc123"}\n[[test.u.check]]\n'
     'name="main"\ntier="release"\ncommand=["x"]\n', "secret"),
    ('schema = 2\n[test]\ndefault="nope"\n[test.u]\n[[test.u.check]]\n'
     'name="main"\ntier="release"\ncommand=["x"]\n', "not defined"),
    ('schema = 2\n[test.BadName]\n[[test.BadName.check]]\nname="main"\n'
     'tier="release"\ncommand=["x"]\n',
     "invalid test name"),
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
    write(repo, 'schema = 2\n[test.u]\ncwd = "link"\n[[test.u.check]]\n'
                'name="main"\ntier="release"\ncommand = ["x"]\n')
    with pytest.raises(ConfigError, match="escapes"):
        load_test_spec(repo, None)


def test_unknown_test_name(tmp_path):
    root = write(tmp_path, 'schema = 2\n[test.u]\n[[test.u.check]]\nname="main"\n'
                           'tier="release"\ncommand=["x"]\n')
    with pytest.raises(ConfigError, match="not defined"):
        load_test_spec(root, "missing")


def test_postgres_section_defaults_and_overrides(tmp_path):
    root = write(tmp_path, 'schema = 2\n[test.u]\n[[test.u.check]]\nname="main"\n'
                           'tier="release"\ncommand = ["x"]\n'
                           '[test.u.postgres]\n')
    spec = load_test_spec(root, None)
    assert spec.postgres is not None
    assert spec.postgres.image == "postgres:16-alpine"
    assert spec.postgres.database == "test"
    root = write(tmp_path, 'schema = 2\n[test.u]\n[[test.u.check]]\nname="main"\n'
                           'tier="release"\ncommand = ["x"]\n'
                           '[test.u.postgres]\nimage = "postgres:17.10-alpine"\n'
                           'database = "app_db"\nuser = "app"\n')
    spec = load_test_spec(root, None)
    assert spec.postgres.image == "postgres:17.10-alpine"
    assert spec.postgres.database == "app_db"


@pytest.mark.parametrize("image", [
    "postgres@sha256:" + "1" * 64,
    "postgis/postgis@sha256:" + "a" * 64,
    "registry.example.test/team/postgres:16-postgis@sha256:" + "f" * 64,
])
def test_postgres_section_accepts_immutable_compatible_images(tmp_path, image):
    root = write(tmp_path, 'schema = 2\n[test.u]\n[[test.u.check]]\nname="main"\n'
                           'tier="release"\ncommand = ["x"]\n'
                           f'[test.u.postgres]\nimage = "{image}"\n')
    assert load_test_spec(root, None).postgres.image == image


@pytest.mark.parametrize("body,fragment", [
    ('image = "mysql:8"', "pinned by sha256 digest"),
    ('image = "postgis/postgis:16-3.5"', "pinned by sha256 digest"),
    ('image = "evil/postgres:16"', "pinned by sha256 digest"),
    ('image = "postgis/postgis@sha256:abc"', "pinned by sha256 digest"),
    ('database = "Bad-Name"', "database must match"),
    ('user = "1abc"', "user must match"),
    ('persistent = true', "unknown keys"),
])
def test_postgres_section_rejections(tmp_path, body, fragment):
    root = write(tmp_path, 'schema = 2\n[test.u]\n[[test.u.check]]\nname="main"\n'
                           'tier="release"\ncommand = ["x"]\n'
                           f'[test.u.postgres]\n{body}\n')
    with pytest.raises(ConfigError) as excinfo:
        load_test_spec(root, None)
    assert fragment in str(excinfo.value)


def test_file_with_both_tests_and_deployments(tmp_path):
    """One file declares both; the test parser must tolerate deployment sections."""
    root = write(tmp_path, 'schema = 2\n[test.unit]\n[[test.unit.check]]\n'
                           'name="main"\ntier="release"\ncommand = ["true"]\n'
                           '[deployment.svc]\ncomponents = ["api"]\n'
                           '[deployment.svc.component.api]\ntype = "process"\n'
                           'command = ["x"]\n')
    assert load_test_spec(root, None).name == "unit"


def test_governed_check_graph_accepts_dependencies_events_and_artifacts(tmp_path):
    root = write(tmp_path, '''
schema = 2
[test.complete]
timeout_seconds = 3600
[[test.complete.check]]
name = "build"
tier = "development"
command = ["make", "build"]
produces = ["dist/app"]
[[test.complete.check]]
name = "server"
tier = "development"
command = ["./scripts/server"]
requires = ["build"]
completion = "event"
on_failure = "stop"
[[test.complete.check]]
name = "browser"
tier = "pre-merge"
command = ["node", "verify.mjs"]
requires = ["server"]
env = { CI = "1" }
[[test.complete.check]]
name = "report"
tier = "release"
command = ["./scripts/report"]
after = ["browser"]
''')
    spec = load_test_spec(root, None)
    assert [check.name for check in spec.checks] == [
        "build", "server", "browser", "report"]
    assert spec.checks[1].completion == "event"
    assert spec.checks[1].on_failure == "stop"
    assert spec.checks[2].requires == ("server",)
    assert spec.checks[0].produces == ("dist/app",)
    assert len(spec.config_digest) == 64


@pytest.mark.parametrize("body,fragment", [
    ('max_parallel = 4\n[[test.g.check]]\nname="a"\ntier="release"\ncommand=["true"]',
     "unknown keys"),
    ('[[test.g.check]]\nname="a"\ntier="release"\ncommand=["true"]\nresources=["db"]',
     "unknown keys"),
    ('[[test.g.check]]\nname="a"\ntier="release"\ncommand=["true"]\nafter=["missing"]',
     "unknown dependencies"),
    ('[[test.g.check]]\nname="a"\ntier="release"\ncommand=["true"]\nafter=["b"]\n'
     '[[test.g.check]]\nname="b"\ntier="release"\ncommand=["true"]\nafter=["a"]',
     "contain a cycle"),
    ('[[test.g.check]]\nname="a"\ntier="release"\ncommand=["true"]\n'
     'produces=["../secret"]',
     "normalized repository-relative"),
    ('[[test.g.check]]\nname="a"\ntier="release"\ncommand=["true"]\n'
     'env={DEVCOORDINATOR_RUN_ID="x"}',
     "reserved internal prefix"),
    ('tier="release"\ncommand=["true"]\n[[test.g.check]]\nname="a"\n'
     'tier="release"\ncommand=["true"]',
     "rejects direct test commands"),
])
def test_governed_check_graph_rejections(tmp_path, body, fragment):
    root = write(tmp_path, f"schema = 2\n[test.g]\n{body}\n")
    with pytest.raises(ConfigError) as excinfo:
        load_test_spec(root, None)
    assert fragment in str(excinfo.value)


def test_schema_two_requires_every_direct_or_graph_tier(tmp_path):
    root = write(tmp_path, 'schema=2\n[test.u]\ncommand=["true"]\n')
    with pytest.raises(ConfigError, match="rejects direct test commands"):
        load_test_spec(root, None)
    root = write(tmp_path, '''
schema=2
[test.u]
[[test.u.check]]
name="unit"
command=["true"]
''')
    with pytest.raises(ConfigError, match="must be 'development'"):
        load_test_spec(root, None)


def test_preflight_invalidates_compile_to_success_dependencies(tmp_path):
    root = write(tmp_path, '''
schema=2
[test.complete]
[[test.complete.check]]
name="source"
tier="development"
role="preflight"
command=["./scripts/source-check"]
invalidates=["browser","release"]
[[test.complete.check]]
name="unit"
tier="development"
command=["pytest"]
[[test.complete.check]]
name="browser"
tier="pre-merge"
command=["node","verify.mjs"]
requires=["unit"]
[[test.complete.check]]
name="release"
tier="release"
command=["./scripts/release"]
requires=["browser"]
''')
    spec = load_test_spec(root, None)
    checks = {check.name: check for check in spec.checks}
    assert checks["source"].role == "preflight"
    assert checks["source"].invalidates == ("browser", "release")
    assert checks["browser"].requires == ("unit", "source")
    assert checks["release"].requires == ("browser", "source")
    selected = GovernedTestLifecycle._selected_closure(
        spec.checks, (), "development")
    plan = GovernedTestLifecycle._build_plan(
        spec, spec.checks, selected, "trun", tmp_path, tmp_path / "current",
        "a" * 64, (), None, None, "development")
    assert [row["name"] for row in plan["checks"]] == ["source", "unit"]
    assert plan["checks"][0]["invalidates"] == []


@pytest.mark.parametrize("body,fragment", [
    ('''[[test.g.check]]
name="work"
tier="release"
command=["true"]
invalidates=["target"]
[[test.g.check]]
name="target"
tier="release"
command=["true"]''', "requires role = 'preflight'"),
    ('''[[test.g.check]]
name="gate"
tier="release"
role="preflight"
command=["true"]
invalidates=["target"]
[[test.g.check]]
name="target"
tier="development"
command=["true"]''', "tier inversion"),
    ('''[[test.g.check]]
name="gate"
tier="development"
role="preflight"
command=["true"]
invalidates=["target"]
[[test.g.check]]
name="target"
tier="release"
command=["true"]
requires=["gate"]''', "duplicate dependency edge"),
])
def test_preflight_rejections(tmp_path, body, fragment):
    root = write(tmp_path, f"schema=2\n[test.g]\n{body}\n")
    with pytest.raises(ConfigError, match=fragment):
        load_test_spec(root, None)


def test_dynamic_and_static_case_expansion_contracts(tmp_path):
    root = write(tmp_path, '''
schema=2
[test.g]
[[test.g.check]]
name="dynamic"
tier="pre-merge"
discover=["python3","discover.py"]
case_command=["python3","run_case.py"]
timeout_seconds=45
[[test.g.check]]
name="static"
tier="release"
cases=[{id="one",args=["--id","1"]},{id="two",args=["--id","2"]}]
case_command=["node","case.mjs"]
requires=["dynamic"]
''')
    checks = {check.name: check for check in load_test_spec(root, None).checks}
    assert checks["dynamic"].discover == ("python3", "discover.py")
    assert checks["dynamic"].timeout_seconds == 45
    assert [case.id for case in checks["static"].cases] == ["one", "two"]
    assert checks["static"].cases[1].args == ("--id", "2")


@pytest.mark.parametrize("body,fragment", [
    ('command=["true"]\ndiscover=["find"]\ncase_command=["run"]',
     "exactly one"),
    ('discover=["find"]', "case_command"),
    ('cases=[{id="bad/id",args=[]}]\ncase_command=["run"]', "id is invalid"),
    ('role="preflight"\ncommand=["true"]\ninvalidates=[]',
     "must invalidate at least one"),
    ('completion="event"\ndiscover=["find"]\ncase_command=["run"]',
     "unavailable for preflights or fan-out"),
])
def test_execution_form_rejections(tmp_path, body, fragment):
    root = write(tmp_path, f'''schema=2
[test.g]
[[test.g.check]]
name="case"
tier="release"
{body}
''')
    with pytest.raises(ConfigError) as excinfo:
        load_test_spec(root, None)
    assert fragment in str(excinfo.value)
