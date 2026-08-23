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
