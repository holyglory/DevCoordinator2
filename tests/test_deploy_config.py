from pathlib import Path

import pytest

from devcoordinator2.daemon.deploy_config import (
    list_deployment_names,
    load_deployment_spec,
)
from devcoordinator2.daemon.repoconfig import ConfigError

FULL = '''
schema = 1
[test.unit]
command = ["true"]

[deployment.web]
source = ["checkout", "worktree"]
domain = { checkout = "app", worktree = "app-dev" }
components = ["db", "api", "worker", "cache", "stack", "smtp"]
build = ["npm", "run", "build"]

[deployment.web.component.db]
type = "postgres"
database = "app"
user = "app"

[deployment.web.component.api]
type = "process"
command = ["npm", "run", "start"]
port = true
route = true
health = { path = "/healthz", timeout_seconds = 30 }
depends_on = ["db"]
persistent_paths = ["var/data"]

[deployment.web.component.worker]
type = "process"
command = ["npm", "run", "worker"]
depends_on = ["db"]
independent_control = false

[deployment.web.component.cache]
type = "docker"
image = "valkey/valkey:9.1.0-alpine"
port = 6379
volumes = ["data:/data"]

[deployment.web.component.stack]
type = "compose"
file = "docker-compose.yml"

[deployment.web.component.smtp]
type = "external"
tcp = "127.0.0.1:25"

[deployment.tool]
components = ["cli"]
domain = "tool"
[deployment.tool.component.cli]
type = "process"
command = ["./run"]
port = true
route = true
'''


def write(tmp_path: Path, text: str) -> Path:
    (tmp_path / ".devcoordinator.toml").write_text(text)
    return tmp_path


def test_full_spec(tmp_path):
    root = write(tmp_path, FULL)
    assert list_deployment_names(root) == ["tool", "web"]
    spec = load_deployment_spec(root, "web")
    assert spec.sources == ("checkout", "worktree")
    assert spec.domain_for("worktree") == "app-dev"
    assert [c.name for c in spec.components] == ["db", "api", "worker", "cache",
                                                  "stack", "smtp"]
    api = spec.component("api")
    assert api.wants_port and api.route and api.health.kind == "http"
    assert spec.route_component is api
    assert spec.component("db").owns_persistent_data
    assert spec.component("cache").owns_persistent_data
    assert not spec.component("worker").owns_persistent_data
    assert not spec.component("worker").independent_control
    stack = spec.component("stack")
    assert stack.compose_files == ("docker-compose.yml",)
    assert stack.finite_services == ()
    canon_a = spec.canonical("checkout")
    canon_b = spec.canonical("worktree")
    assert canon_a != canon_b and canon_a["domain"] == "app"
    tool = load_deployment_spec(root, "tool")
    assert tool.sources == ("worktree",)
    assert tool.domain_for("worktree") == "tool"


BASE = 'schema = 1\n[deployment.d]\ncomponents = ["a"]\n'


@pytest.mark.parametrize("body,fragment", [
    (BASE + '[deployment.d.component.a]\ntype = "process"\ncommand = "sh -c x"\n',
     "argv array"),
    (BASE + '[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\nport = 8080\n',
     "daemon leases it"),
    (BASE + '[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\nroute = true\n',
     "requires port = true"),
    (BASE + 'domain = "x"\n[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\n',
     "requires one component with route"),
    (BASE.replace('["a"]', '["a", "b"]') +
     '[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\n'
     '[deployment.d.component.b]\ntype = "process"\ncommand = ["x"]\ndepends_on = ["c"]\n',
     "earlier component"),
    (BASE + '[deployment.d.component.a]\ntype = "docker"\nimage = "img"\n'
            'volumes = ["/host/path:/data"]\n', "host paths are forbidden"),
    (BASE + '[deployment.d.component.a]\ntype = "docker"\nimage = "img"\n'
            'privileged = true\n', "unknown keys"),
    (BASE + '[deployment.d.component.a]\ntype = "postgres"\nshared_from = "bad"\n',
     "shared_from must be"),
    (BASE + '[deployment.d.component.a]\ntype = "postgres"\n'
            'shared_from = "d0123456789abcdef/db"\ndatabase = "x"\n', "excludes"),
    (BASE + '[deployment.d.component.a]\ntype = "compose"\nfile = "../x.yml"\n',
     "escapes"),
    (BASE + '[deployment.d.component.a]\ntype = "external"\ntcp = "nope"\n',
     "host:port"),
    (BASE + '[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\n'
            'env = { API_TOKEN = "abc" }\n', "literal secret"),
    (BASE + 'source = ["checkout", "worktree"]\ndomain = "x"\n'
            '[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\n'
            'port = true\nroute = true\n', "domain must be a table"),
    (BASE + 'source = ["worktree"]\ndomain = { checkout = "x" }\n'
            '[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\n'
            'port = true\nroute = true\n', "not enabled"),
    (BASE + 'source = ["checkout", "worktree"]\ndomain = { checkout = "x", worktree = "x" }\n'
            '[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\n'
            'port = true\nroute = true\n', "must not share"),
    (BASE + 'ttl_seconds = 5\n[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\n',
     "ttl_seconds"),
    (BASE.replace('["a"]', '["a"]') + '[deployment.d.component.a]\ntype = "process"\n'
     'command = ["x"]\n[deployment.d.component.zz]\ntype = "process"\ncommand = ["x"]\n',
     "unlisted"),
])
def test_rejections(tmp_path, body, fragment):
    root = write(tmp_path, body)
    with pytest.raises(ConfigError) as excinfo:
        load_deployment_spec(root, "d")
    assert fragment in str(excinfo.value), str(excinfo.value)


def test_unknown_deployment(tmp_path):
    root = write(tmp_path, BASE + '[deployment.d.component.a]\ntype = "process"\n'
                                  'command = ["x"]\n')
    with pytest.raises(ConfigError, match="not defined"):
        load_deployment_spec(root, "nope")


def test_domain_with_single_port_component_routes_implicitly(tmp_path):
    (tmp_path / ".devcoordinator.toml").write_text(
        'schema = 1\n[deployment.d]\ncomponents = ["app", "worker"]\n'
        'domain = "para"\n'
        '[deployment.d.component.app]\ntype = "process"\ncommand = ["x"]\nport = true\n'
        '[deployment.d.component.worker]\ntype = "process"\ncommand = ["y"]\n')
    spec = load_deployment_spec(tmp_path, "d")
    assert spec.route_component is not None
    assert spec.route_component.name == "app"


def test_domain_with_two_port_components_still_requires_route(tmp_path):
    (tmp_path / ".devcoordinator.toml").write_text(
        'schema = 1\n[deployment.d]\ncomponents = ["a", "b"]\ndomain = "para"\n'
        '[deployment.d.component.a]\ntype = "process"\ncommand = ["x"]\nport = true\n'
        '[deployment.d.component.b]\ntype = "process"\ncommand = ["y"]\nport = true\n')
    with pytest.raises(ConfigError, match="route = true"):
        load_deployment_spec(tmp_path, "d")


def test_postgres_port_never_becomes_the_implicit_route(tmp_path):
    (tmp_path / ".devcoordinator.toml").write_text(
        'schema = 1\n[deployment.d]\ncomponents = ["db"]\ndomain = "para"\n'
        '[deployment.d.component.db]\ntype = "postgres"\n')
    with pytest.raises(ConfigError, match="route = true"):
        load_deployment_spec(tmp_path, "d")


def test_compose_files_finite_services_and_route(tmp_path):
    for name in ("compose.yml", "compose.build.yml", "dev.env"):
        (tmp_path / name).write_text("")
    (tmp_path / ".devcoordinator.toml").write_text(
        'schema = 1\n[deployment.d]\ncomponents = ["stack"]\ndomain = "app"\n'
        '[deployment.d.component.stack]\ntype = "compose"\n'
        'files = ["compose.yml", "compose.build.yml"]\n'
        'env_file = "dev.env"\nservices = ["db", "bootstrap", "api"]\n'
        'finite_services = ["bootstrap"]\nindependent_services = ["api"]\n'
        'build = true\nport = true\nroute = true\n'
        'timeout_seconds = 900\n')
    stack = load_deployment_spec(tmp_path, "d").component("stack")
    assert stack.compose_files == ("compose.yml", "compose.build.yml")
    assert stack.compose_env_file == "dev.env"
    assert stack.services == ("db", "bootstrap", "api")
    assert stack.finite_services == ("bootstrap",)
    assert stack.independent_services == ("api",)
    assert stack.compose_build
    assert stack.wants_port and stack.route
    assert stack.compose_timeout_seconds == 900


@pytest.mark.parametrize("extra,fragment", [
    ('file = "compose.yml"\nfiles = ["compose.yml"]\n', "mutually exclusive"),
    ('files = ["../compose.yml"]\n', "escapes"),
    ('env_file = "../dev.env"\n', "escapes"),
    ('finite_services = ["bootstrap"]\n', "explicit services"),
    ('services = ["bootstrap"]\nfinite_services = ["bootstrap"]\n', "running service"),
    ('services = ["api"]\nfinite_services = ["bootstrap"]\n', "included"),
    ('independent_services = ["api"]\n', "explicit services"),
    ('services = ["api"]\nindependent_services = ["worker"]\n', "included"),
    ('services = ["bootstrap", "api"]\nfinite_services = ["bootstrap"]\n'
     'independent_services = ["bootstrap"]\n', "cannot be independently"),
    ('build = "yes"\n', "boolean"),
    ('route = true\n', "requires port"),
    ('timeout_seconds = 901\n', "[1, 900]"),
])
def test_compose_lifecycle_rejections(tmp_path, extra, fragment):
    (tmp_path / ".devcoordinator.toml").write_text(
        'schema = 1\n[deployment.d]\ncomponents = ["stack"]\n'
        '[deployment.d.component.stack]\ntype = "compose"\n' + extra)
    with pytest.raises(ConfigError, match=fragment):
        load_deployment_spec(tmp_path, "d")


def test_compose_ignored_env_file_rejects_checkout_source(tmp_path):
    (tmp_path / ".devcoordinator.toml").write_text(
        'schema = 1\n[deployment.d]\nsource = "checkout"\ncomponents = ["stack"]\n'
        '[deployment.d.component.stack]\ntype = "compose"\nenv_file = "dev.env"\n')
    with pytest.raises(ConfigError, match="worktree-only"):
        load_deployment_spec(tmp_path, "d")
