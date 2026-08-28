"""[deployment.<name>] parsing and strict validation (docs/repository-config.md).

Shares the file with [test.*]; this module validates only deployment
sections. Unknown keys are rejected everywhere.
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from devcoordinator2.daemon.repoconfig import (
    CONFIG_NAME,
    MAX_CONFIG_BYTES,
    PG_IDENT_RE,
    POSTGRES_IMAGE_DEFAULT,
    POSTGRES_IMAGE_RE,
    ConfigError,
)

NAME_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,31}$")
COMPOSE_SERVICE_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
DOMAIN_LABEL_RE = re.compile(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$")
IMAGE_RE = re.compile(r"[a-z0-9][a-z0-9._/-]{0,200}(:[A-Za-z0-9][A-Za-z0-9._-]{0,127})?"
                      r"(@sha256:[0-9a-f]{64})?$")
VOLUME_RE = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}:(/[^:\s]*)$")
TCP_RE = re.compile(r"(127\.0\.0\.1|localhost|[a-z0-9.-]+):(\d{1,5})$")
SHARED_FROM_RE = re.compile(r"d[0-9a-f]{16}/[a-z0-9][a-z0-9-]{0,31}$")
_SECRET_KEY_RE = re.compile(r"(token|secret|password|passwd|credential|api_?key)",
                            re.IGNORECASE)
COMPONENT_TYPES = ("process", "docker", "compose", "postgres", "external")
SOURCE_MODES = ("worktree", "checkout")
HEALTH_TIMEOUT_DEFAULT = 60
COMPOSE_READINESS_TIMEOUT_DEFAULT = 300


@dataclass(frozen=True)
class HealthSpec:
    kind: str  # "http" | "tcp"
    path: str | None
    timeout_seconds: int


@dataclass(frozen=True)
class ComponentSpec:
    name: str
    type: str
    order: int
    independent_control: bool
    depends_on: tuple[str, ...]
    env: dict[str, str]
    # process
    command: tuple[str, ...] = ()
    cwd: str = "."
    wants_port: bool = False
    route: bool = False
    health: HealthSpec | None = None
    persistent_paths: tuple[str, ...] = ()
    # docker
    image: str | None = None
    container_port: int | None = None
    volumes: tuple[str, ...] = ()
    # compose
    compose_files: tuple[str, ...] = ()
    compose_env_file: str | None = None
    compose_build: bool = False
    services: tuple[str, ...] = ()
    finite_services: tuple[str, ...] = ()
    independent_services: tuple[str, ...] = ()
    compose_timeout_seconds: int = COMPOSE_READINESS_TIMEOUT_DEFAULT
    # postgres
    database: str | None = None
    user: str | None = None
    shared_from: str | None = None
    # external
    tcp: str | None = None

    @property
    def owns_persistent_data(self) -> bool:
        return (self.type == "postgres" and self.shared_from is None) \
            or bool(self.volumes) or bool(self.persistent_paths) \
            or self.type == "compose"


@dataclass(frozen=True)
class DeploymentSpec:
    """One declaration; each enabled source is an independent instance."""
    name: str
    sources: tuple[str, ...]            # enabled sources, declaration order
    domains: dict[str, str]             # source -> domain label
    build: tuple[str, ...]
    ttl_seconds: int | None
    public: bool = False                 # route without sign-in at the edge
    components: tuple[ComponentSpec, ...] = field(default_factory=tuple)

    def domain_for(self, source: str) -> str | None:
        return self.domains.get(source)

    def component(self, name: str) -> ComponentSpec | None:
        return next((c for c in self.components if c.name == name), None)

    @property
    def route_component(self) -> ComponentSpec | None:
        """Explicit route = true wins; otherwise a deployment with exactly one
        port-leasing process/docker/compose component routes to it implicitly, so a
        single-service deployment can receive a domain without ceremony."""
        explicit = next((c for c in self.components if c.route), None)
        if explicit is not None:
            return explicit
        candidates = [c for c in self.components
                      if c.wants_port and c.type in ("process", "docker", "compose")]
        return candidates[0] if len(candidates) == 1 else None

    def canonical(self, source: str) -> dict:
        """Secret-free, order-stable representation for fingerprinting one
        source instance (the other source's domain must not affect it)."""
        return {
            "name": self.name, "source": source, "domain": self.domain_for(source),
            "build": list(self.build), "ttl_seconds": self.ttl_seconds,
            "public": self.public,
            "components": [
                {k: (list(v) if isinstance(v, tuple) else
                     (v.__dict__ if isinstance(v, HealthSpec) else v))
                 for k, v in sorted(c.__dict__.items())}
                for c in self.components
            ],
        }


def _read(worktree_root: Path) -> dict:
    config_path = worktree_root / CONFIG_NAME
    try:
        if config_path.stat().st_size > MAX_CONFIG_BYTES:
            raise ConfigError(f"{CONFIG_NAME} exceeds {MAX_CONFIG_BYTES} bytes")
        raw = config_path.read_bytes()
    except FileNotFoundError:
        raise ConfigError(f"{CONFIG_NAME} not found in {worktree_root}") from None
    except OSError as exc:
        raise ConfigError(f"cannot read {CONFIG_NAME}: {exc}") from exc
    try:
        data = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise ConfigError(f"invalid TOML: {exc}") from exc
    unknown = set(data) - {"schema", "test", "deployment"}
    if unknown:
        raise ConfigError(f"unknown top-level keys: {sorted(unknown)}")
    if data.get("schema") != 1:
        raise ConfigError("'schema' must be 1")
    return data


def list_deployment_names(worktree_root: Path) -> list[str]:
    section = _read(worktree_root).get("deployment") or {}
    if not isinstance(section, dict):
        raise ConfigError("[deployment] must contain named tables")
    return sorted(section)


def load_deployment_spec(worktree_root: Path, name: str) -> DeploymentSpec:
    section = _read(worktree_root).get("deployment") or {}
    available = sorted(section) if isinstance(section, dict) else []
    if not isinstance(section, dict) or name not in section:
        raise ConfigError(f"deployment {name!r} is not defined (available: {available})")
    if not NAME_RE.fullmatch(name):
        raise ConfigError(f"invalid deployment name {name!r}")
    body = section[name]
    if not isinstance(body, dict):
        raise ConfigError(f"[deployment.{name}] must be a table")
    return _validate_deployment(worktree_root, name, body)


def _validate_deployment(root: Path, name: str, body: dict) -> DeploymentSpec:
    prefix = f"[deployment.{name}]"
    unknown = set(body) - {"source", "domain", "components", "build",
                           "ttl_seconds", "component", "public"}
    if unknown:
        raise ConfigError(f"{prefix} unknown keys: {sorted(unknown)}")
    sources = _validate_sources(prefix, body.get("source", "worktree"))
    domains = _validate_domains(prefix, body.get("domain"), sources)
    build = body.get("build", [])
    if isinstance(build, str) or not isinstance(build, list) \
            or not all(isinstance(a, str) and a for a in build):
        raise ConfigError(f"{prefix} build must be an argv array")
    public = body.get("public", False)
    if not isinstance(public, bool):
        raise ConfigError(f"{prefix} public must be a boolean")
    ttl = body.get("ttl_seconds")
    if ttl is not None and (not isinstance(ttl, int) or isinstance(ttl, bool)
                            or not (60 <= ttl <= 30 * 86400)):
        raise ConfigError(f"{prefix} ttl_seconds must be an integer in [60, 2592000]")
    order = body.get("components")
    if not isinstance(order, list) or not order \
            or not all(isinstance(n, str) and NAME_RE.fullmatch(n) for n in order):
        raise ConfigError(f"{prefix} components must be a non-empty list of names")
    if len(set(order)) != len(order):
        raise ConfigError(f"{prefix} components contains duplicates")
    tables = body.get("component", {})
    if not isinstance(tables, dict):
        raise ConfigError(f"{prefix} component must be a table of tables")
    missing = [n for n in order if n not in tables]
    extra = [n for n in tables if n not in order]
    if missing or extra:
        raise ConfigError(f"{prefix} components and component tables differ: "
                          f"missing {missing}, unlisted {extra}")
    components = []
    for index, cname in enumerate(order):
        components.append(_validate_component(root, name, cname, index,
                                              tables[cname], order[:index]))
    if "checkout" in sources and any(
            component.type == "compose" and component.compose_env_file
            for component in components):
        raise ConfigError(
            f"{prefix} ignored Compose env_file requires worktree-only source")
    routes = [c for c in components if c.route]
    if len(routes) > 1:
        raise ConfigError(f"{prefix} at most one component may set route = true")
    if domains and not routes:
        implicit = [c for c in components
                    if c.wants_port and c.type in ("process", "docker", "compose")]
        if len(implicit) != 1:
            raise ConfigError(
                f"{prefix} domain requires one component with route = true"
                " (implicit only when exactly one process/docker/compose component"
                " leases a port)")
    if routes and not routes[0].wants_port:
        raise ConfigError(f"{prefix} the route component must lease a port")
    return DeploymentSpec(name=name, sources=sources, domains=domains,
                          build=tuple(build), ttl_seconds=ttl, public=public,
                          components=tuple(components))


def _validate_sources(prefix: str, raw) -> tuple[str, ...]:
    values = [raw] if isinstance(raw, str) else raw
    if not isinstance(values, list) or not values \
            or not all(v in SOURCE_MODES for v in values) \
            or len(set(values)) != len(values):
        raise ConfigError(f"{prefix} source must be one of {SOURCE_MODES} "
                          "or a list enabling both")
    return tuple(values)


def _validate_domains(prefix: str, raw, sources: tuple[str, ...]) -> dict[str, str]:
    if raw is None:
        return {}
    if isinstance(raw, str):
        if len(sources) != 1:
            raise ConfigError(f"{prefix} with two sources, domain must be a table "
                              "{ checkout = ..., worktree = ... }")
        raw = {sources[0]: raw}
    if not isinstance(raw, dict):
        raise ConfigError(f"{prefix} domain must be a label or a per-source table")
    domains: dict[str, str] = {}
    for source, label in raw.items():
        if source not in sources:
            raise ConfigError(f"{prefix} domain.{source} names a source that is not enabled")
        if not isinstance(label, str) or not DOMAIN_LABEL_RE.fullmatch(label):
            raise ConfigError(f"{prefix} domain.{source} must be a DNS label")
        domains[source] = label
    if len(set(domains.values())) != len(domains):
        raise ConfigError(f"{prefix} the two sources must not share a domain")
    return domains


def _validate_component(root: Path, dname: str, cname: str, index: int,
                        body: dict, earlier: list[str]) -> ComponentSpec:
    prefix = f"[deployment.{dname}.component.{cname}]"
    if not isinstance(body, dict):
        raise ConfigError(f"{prefix} must be a table")
    ctype = body.get("type")
    if ctype not in COMPONENT_TYPES:
        raise ConfigError(f"{prefix} type must be one of {COMPONENT_TYPES}")
    common = {"type", "depends_on", "env", "independent_control"}
    allowed = {
        "process": common | {"command", "cwd", "port", "route", "health",
                             "persistent_paths"},
        "docker": common | {"image", "command", "port", "volumes", "health"},
        "compose": common | {"file", "files", "env_file", "services",
                             "finite_services", "independent_services", "build",
                             "port", "route", "timeout_seconds"},
        "postgres": common | {"image", "database", "user", "shared_from"},
        "external": common | {"tcp"},
    }[ctype]
    unknown = set(body) - allowed
    if unknown:
        raise ConfigError(f"{prefix} unknown keys for type {ctype}: {sorted(unknown)}")

    depends = body.get("depends_on", [])
    if not isinstance(depends, list) or not all(isinstance(d, str) for d in depends):
        raise ConfigError(f"{prefix} depends_on must be a list of names")
    for dep in depends:
        if dep not in earlier:
            raise ConfigError(f"{prefix} depends_on {dep!r} must name an earlier component")
    env = _validate_env(prefix, body.get("env", {}))
    independent = body.get("independent_control", True)
    if not isinstance(independent, bool):
        raise ConfigError(f"{prefix} independent_control must be a boolean")
    spec = {"name": cname, "type": ctype, "order": index,
            "independent_control": independent, "depends_on": tuple(depends),
            "env": env}

    if ctype == "process":
        command = body.get("command")
        if isinstance(command, str) or not isinstance(command, list) or not command \
                or not all(isinstance(a, str) and a for a in command):
            raise ConfigError(f"{prefix} command must be a non-empty argv array")
        cwd = body.get("cwd", ".")
        if not isinstance(cwd, str) or Path(cwd).is_absolute():
            raise ConfigError(f"{prefix} cwd must be repository-relative")
        resolved = (root.resolve() / cwd).resolve()
        if resolved != root.resolve() and root.resolve() not in resolved.parents:
            raise ConfigError(f"{prefix} cwd escapes the repository")
        port = body.get("port", False)
        if not isinstance(port, bool):
            raise ConfigError(f"{prefix} port must be true/false (the daemon leases it)")
        route = body.get("route", False)
        if not isinstance(route, bool):
            raise ConfigError(f"{prefix} route must be a boolean")
        if route and not port:
            raise ConfigError(f"{prefix} route = true requires port = true")
        paths = body.get("persistent_paths", [])
        if not isinstance(paths, list) or not all(
                isinstance(p, str) and p and not Path(p).is_absolute()
                and ".." not in Path(p).parts for p in paths):
            raise ConfigError(f"{prefix} persistent_paths must be repository-relative")
        spec.update(command=tuple(command), cwd=cwd, wants_port=port, route=route,
                    health=_validate_health(prefix, body.get("health")),
                    persistent_paths=tuple(paths))
    elif ctype == "docker":
        image = body.get("image")
        if not isinstance(image, str) or not IMAGE_RE.fullmatch(image):
            raise ConfigError(f"{prefix} image must be a plain image reference")
        command = body.get("command", [])
        if isinstance(command, str) or not isinstance(command, list) \
                or not all(isinstance(a, str) for a in command):
            raise ConfigError(f"{prefix} command must be an argv array")
        cport = body.get("port")
        if cport is not None and (not isinstance(cport, int) or isinstance(cport, bool)
                                  or not (1 <= cport <= 65535)):
            raise ConfigError(f"{prefix} port must be a container port number")
        volumes = body.get("volumes", [])
        if not isinstance(volumes, list) or not all(
                isinstance(v, str) and VOLUME_RE.fullmatch(v) for v in volumes):
            raise ConfigError(f"{prefix} volumes must be 'name:/container/path' "
                              "named volumes (host paths are forbidden)")
        spec.update(image=image, command=tuple(command), container_port=cport,
                    wants_port=cport is not None, volumes=tuple(volumes),
                    health=_validate_health(prefix, body.get("health")))
    elif ctype == "compose":
        if "file" in body and "files" in body:
            raise ConfigError(f"{prefix} file and files are mutually exclusive")
        files = body.get("files", [body.get("file", "docker-compose.yml")])
        if not isinstance(files, list) or not files \
                or not all(isinstance(file, str) and file for file in files):
            raise ConfigError(f"{prefix} files must be a non-empty list")
        if len(set(files)) != len(files):
            raise ConfigError(f"{prefix} files contains duplicates")
        for file in files:
            if Path(file).is_absolute():
                raise ConfigError(f"{prefix} files must be repository-relative")
            resolved = (root.resolve() / file).resolve()
            if resolved != root.resolve() and root.resolve() not in resolved.parents:
                raise ConfigError(f"{prefix} file escapes the repository")
        env_file = body.get("env_file")
        if env_file is not None:
            if not isinstance(env_file, str) or not env_file \
                    or Path(env_file).is_absolute():
                raise ConfigError(f"{prefix} env_file must be repository-relative")
            resolved = (root.resolve() / env_file).resolve()
            if resolved != root.resolve() and root.resolve() not in resolved.parents:
                raise ConfigError(f"{prefix} env_file escapes the repository")
        services = body.get("services", [])
        if not isinstance(services, list) or not all(
                isinstance(s, str) and COMPOSE_SERVICE_RE.fullmatch(s)
                for s in services):
            raise ConfigError(f"{prefix} services must be a list of names")
        if len(set(services)) != len(services):
            raise ConfigError(f"{prefix} services contains duplicates")
        finite = body.get("finite_services", [])
        if not isinstance(finite, list) or not all(
                isinstance(s, str) and COMPOSE_SERVICE_RE.fullmatch(s)
                for s in finite):
            raise ConfigError(f"{prefix} finite_services must be a list of names")
        if len(set(finite)) != len(finite):
            raise ConfigError(f"{prefix} finite_services contains duplicates")
        if finite and (not services or not set(finite).issubset(services)):
            raise ConfigError(f"{prefix} finite_services must be included in explicit services")
        if finite and len(finite) == len(services):
            raise ConfigError(f"{prefix} finite_services must leave a running service")
        independent = body.get("independent_services", [])
        if not isinstance(independent, list) or not all(
                isinstance(s, str) and COMPOSE_SERVICE_RE.fullmatch(s)
                for s in independent):
            raise ConfigError(f"{prefix} independent_services must be a list of names")
        if len(set(independent)) != len(independent):
            raise ConfigError(f"{prefix} independent_services contains duplicates")
        if independent and (not services or not set(independent).issubset(services)):
            raise ConfigError(
                f"{prefix} independent_services must be included in explicit services")
        overlap = sorted(set(independent) & set(finite))
        if overlap:
            raise ConfigError(
                f"{prefix} finite services cannot be independently controlled: {overlap}")
        build = body.get("build", False)
        if not isinstance(build, bool):
            raise ConfigError(f"{prefix} build must be a boolean")
        port = body.get("port", False)
        route = body.get("route", False)
        if not isinstance(port, bool) or not isinstance(route, bool):
            raise ConfigError(f"{prefix} port and route must be booleans")
        if route and not port:
            raise ConfigError(f"{prefix} route = true requires port = true")
        timeout = body.get("timeout_seconds", COMPOSE_READINESS_TIMEOUT_DEFAULT)
        if not isinstance(timeout, int) or isinstance(timeout, bool) \
                or not (1 <= timeout <= 900):
            raise ConfigError(f"{prefix} timeout_seconds must be in [1, 900]")
        spec.update(compose_files=tuple(files), compose_env_file=env_file,
                    compose_build=build, services=tuple(services),
                    finite_services=tuple(finite),
                    independent_services=tuple(independent),
                    wants_port=port, route=route,
                    compose_timeout_seconds=timeout)
    elif ctype == "postgres":
        shared = body.get("shared_from")
        if shared is not None:
            if not isinstance(shared, str) or not SHARED_FROM_RE.fullmatch(shared):
                raise ConfigError(f"{prefix} shared_from must be '<deployment_id>/<component>'")
            if any(k in body for k in ("image", "database", "user")):
                raise ConfigError(f"{prefix} shared_from excludes image/database/user")
            spec.update(shared_from=shared)
        else:
            image = body.get("image", POSTGRES_IMAGE_DEFAULT)
            if not isinstance(image, str) or not POSTGRES_IMAGE_RE.fullmatch(image):
                raise ConfigError(f"{prefix} image must be an official 'postgres:<tag>'")
            database = body.get("database", "app")
            user = body.get("user", "app")
            for key, value in (("database", database), ("user", user)):
                if not isinstance(value, str) or not PG_IDENT_RE.fullmatch(value):
                    raise ConfigError(f"{prefix} {key} must match [a-z_][a-z0-9_]{{0,62}}")
            spec.update(image=image, database=database, user=user, wants_port=True)
    else:  # external
        tcp = body.get("tcp")
        if not isinstance(tcp, str) or not TCP_RE.fullmatch(tcp):
            raise ConfigError(f"{prefix} tcp must be 'host:port'")
        spec.update(tcp=tcp)
    return ComponentSpec(**spec)


def _validate_env(prefix: str, raw) -> dict[str, str]:
    if not isinstance(raw, dict):
        raise ConfigError(f"{prefix} env must be a table of strings")
    env: dict[str, str] = {}
    for key, value in raw.items():
        if not isinstance(value, str) or not key.isidentifier():
            raise ConfigError(f"{prefix} env.{key} must be a string with an identifier name")
        if _SECRET_KEY_RE.search(key) and value:
            raise ConfigError(f"{prefix} env.{key} looks like a literal secret; "
                              "reference secrets held outside the repository")
        env[key] = value
    return env


def _validate_health(prefix: str, raw) -> HealthSpec | None:
    if raw is None:
        return None
    if not isinstance(raw, dict):
        raise ConfigError(f"{prefix} health must be a table")
    unknown = set(raw) - {"path", "tcp", "timeout_seconds"}
    if unknown:
        raise ConfigError(f"{prefix} health unknown keys: {sorted(unknown)}")
    timeout = raw.get("timeout_seconds", HEALTH_TIMEOUT_DEFAULT)
    if not isinstance(timeout, int) or isinstance(timeout, bool) or not (1 <= timeout <= 900):
        raise ConfigError(f"{prefix} health.timeout_seconds must be in [1, 900]")
    if "path" in raw:
        path = raw["path"]
        if not isinstance(path, str) or not path.startswith("/"):
            raise ConfigError(f"{prefix} health.path must start with '/'")
        return HealthSpec(kind="http", path=path, timeout_seconds=timeout)
    if raw.get("tcp") is True:
        return HealthSpec(kind="tcp", path=None, timeout_seconds=timeout)
    raise ConfigError(f"{prefix} health needs path = '/...' or tcp = true")
