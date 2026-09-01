#!/usr/bin/env python3
"""Install DevCoordinator2 directly from its canonical checkout (run as root).

Creates the client group and edge system user, state directories, instance
configuration templates (only if absent), daemon and edge units, the
command-line shim, and direct policy/skill links for configured agent roots.
The repository remains the only source tree; this installer never copies or
activates an immutable release.
"""

from __future__ import annotations

import argparse
import contextlib
import grp
import hashlib
import json
import os
import pwd
import secrets
import select
import shutil
import sqlite3
import stat
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ETC = Path("/etc/devcoordinator2")
MANAGED_SKILLS = (
    "dev-coordinator",
    "formal-web-ui-verification",
    "full-repo-audit",
    "full-repo-test-coverage-audit",
    "ui-implementation-audit",
    "user-journey-docs-audit",
)
COMPOSE_ENV_ALLOWLIST = ETC / "compose-env-allowlist.json"
CODEX_USAGE_SOURCES = ETC / "codex-usage-sources.json"
_REPOSITORY_NAMESPACE = b"devcoordinator2.repository\0"
_LEGACY_FENCE_GUARD = """
import os
import sys
sys.stdin.buffer.read()
socket_path, fence_path = sys.argv[1:3]
if os.path.exists(fence_path):
    if os.path.exists(socket_path):
        os.unlink(fence_path)
    else:
        os.replace(fence_path, socket_path)
"""


def run(argv: list[str], check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(argv, check=check, capture_output=True, text=True)


def ensure_group(name: str, members: list[str]) -> None:
    try:
        grp.getgrnam(name)
    except KeyError:
        run(["groupadd", "--system", name])
    for user in members:
        run(["usermod", "-aG", name, user])


def ensure_edge_user(client_group: str) -> tuple[int, int]:
    try:
        entry = pwd.getpwnam("devcoordinator2-edge")
    except KeyError:
        run(["useradd", "--system", "--no-create-home", "--shell", "/usr/sbin/nologin",
             "--home-dir", "/var/lib/devcoordinator2-edge", "-G", client_group,
             "devcoordinator2-edge"])
        entry = pwd.getpwnam("devcoordinator2-edge")
    return entry.pw_uid, entry.pw_gid


def _replace_direct_link(destination: Path, source: Path) -> None:
    if (destination.exists() or destination.is_symlink()) and not destination.is_symlink():
        raise RuntimeError(f"refusing to replace non-symlink managed path: {destination}")
    temporary = destination.with_name(f".{destination.name}.tmp")
    if temporary.exists() or temporary.is_symlink():
        if not temporary.is_symlink():
            raise RuntimeError(f"refusing to replace non-symlink staging path: {temporary}")
        temporary.unlink()
    temporary.symlink_to(source)
    os.replace(temporary, destination)


def _managed_legacy_link(path: Path) -> bool:
    if not path.is_symlink():
        return False
    raw = Path(os.readlink(path))
    return len(raw.parts) >= 2 and raw.parts[-2:] == ("skills", "codex-dev-coordinator")


def install_skill_links(accounts: list[str], skills_root: Path) -> tuple[list[str], list[str]]:
    installed: list[str] = []
    retired: list[str] = []
    for skill in MANAGED_SKILLS:
        source = skills_root / skill
        if not source.is_dir() or source.is_symlink():
            raise RuntimeError(f"canonical skill is unavailable: {source}")
    for name in accounts:
        entry = pwd.getpwnam(name)
        home = Path(entry.pw_dir)
        for agent_root_name in (".codex", ".claude"):
            agent_root = home / agent_root_name
            if not agent_root.is_dir():
                continue
            skills_dir = agent_root / "skills"
            if not skills_dir.exists():
                skills_dir.mkdir(mode=0o755)
                os.chown(skills_dir, entry.pw_uid, entry.pw_gid)
            elif not skills_dir.is_dir():
                raise RuntimeError(f"skill root is not a directory: {skills_dir}")
            for skill in MANAGED_SKILLS:
                link = skills_dir / skill
                _replace_direct_link(link, skills_root / skill)
                installed.append(str(link))
            legacy = skills_dir / "codex-dev-coordinator"
            if _managed_legacy_link(legacy):
                legacy.unlink()
                retired.append(str(legacy))
    return installed, retired


def install_policy_links(accounts: list[str], policy: Path) -> list[str]:
    if not policy.is_file() or policy.is_symlink():
        raise RuntimeError(f"canonical universal policy is unavailable: {policy}")
    installed: list[str] = []
    for name in accounts:
        entry = pwd.getpwnam(name)
        home = Path(entry.pw_dir)
        for agent_root_name, filename in ((".codex", "AGENTS.md"), (".claude", "CLAUDE.md")):
            agent_root = home / agent_root_name
            if not agent_root.is_dir():
                continue
            destination = agent_root / filename
            _replace_direct_link(destination, policy)
            installed.append(str(destination))
    return installed


def _coordinator_runtime():
    source = str(ROOT / "src")
    if source not in sys.path:
        sys.path.insert(0, source)
    from devcoordinator2.daemon import test_admission
    from devcoordinator2.paths import load_instance_config
    return test_admission, load_instance_config()


def _test_units(unit_prefix: str) -> list[str]:
    proc = run([
        "systemctl", "list-units", "--all", "--plain", "--no-legend",
        "--no-pager", f"{unit_prefix}-*.service",
    ], check=False)
    return [line.split()[0] for line in proc.stdout.splitlines() if line.split()]


def _unit_cgroup(unit: str) -> Path | None:
    proc = run([
        "systemctl", "show", unit, "-p", "ControlGroup", "--value", "--no-pager",
    ], check=False)
    value = proc.stdout.strip()
    return Path("/sys/fs/cgroup") / value.lstrip("/") if value else None


def _wait_cgroup_empty(path: Path | None) -> None:
    if path is None:
        return
    events_path = path / "cgroup.events"
    try:
        fd = os.open(events_path, os.O_RDONLY | os.O_CLOEXEC)
    except FileNotFoundError:
        return
    try:
        watcher = select.poll()
        watcher.register(fd, select.POLLPRI | select.POLLERR)
        while True:
            os.lseek(fd, 0, os.SEEK_SET)
            payload = os.read(fd, 4096).decode("ascii", errors="replace")
            populated = next((line.split()[1] for line in payload.splitlines()
                              if line.startswith("populated ")), "0")
            if populated == "0":
                return
            watcher.poll()
    finally:
        os.close(fd)


def _cgroup_is_populated(path: Path | None) -> bool:
    if path is None:
        return False
    try:
        payload = (path / "cgroup.events").read_text()
    except FileNotFoundError:
        return False
    return any(line == "populated 1" for line in payload.splitlines())


def _wait_legacy_tests(unit_prefix: str) -> None:
    """Wait on cgroup completion events after the old socket is fenced."""
    while True:
        units = [unit for unit in _test_units(unit_prefix)
                 if _cgroup_is_populated(_unit_cgroup(unit))]
        if not units:
            return
        for unit in units:
            _wait_cgroup_empty(_unit_cgroup(unit))


def _registered_test_directories(database_path: Path) -> list[Path]:
    try:
        connection = sqlite3.connect(
            f"file:{database_path}?mode=ro", uri=True, timeout=5)
        connection.execute("PRAGMA query_only=ON")
        rows = connection.execute(
            "SELECT worktree_path FROM worktrees ORDER BY worktree_path").fetchall()
    except sqlite3.Error as exc:
        raise RuntimeError(f"cannot read registered test worktrees: {exc}") from exc
    finally:
        if "connection" in locals():
            connection.close()
    return [Path(row[0]) / ".devcoordinator" / "test" / "current"
            for row in rows]


def _summary_status(directory: Path) -> str | None:
    try:
        directory_fd = os.open(
            directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except (FileNotFoundError, OSError):
        return None
    try:
        try:
            summary_fd = os.open(
                "summary.json", os.O_RDONLY | os.O_NOFOLLOW, dir_fd=directory_fd)
        except (FileNotFoundError, OSError):
            return None
        try:
            details = os.fstat(summary_fd)
            if not stat.S_ISREG(details.st_mode) or details.st_size > 262144:
                return None
            payload = os.read(summary_fd, 262145)
            if len(payload) > 262144:
                return None
            document = json.loads(payload)
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            return None
        finally:
            os.close(summary_fd)
    finally:
        os.close(directory_fd)
    status = document.get("status") if isinstance(document, dict) else None
    return status if isinstance(status, str) else None


def _wait_legacy_summaries(directories: list[Path], test_admission) -> None:
    """Wait until the old daemon publishes each terminal atomic summary."""
    while True:
        running = [directory for directory in directories
                   if _summary_status(directory) == "running"]
        if not running:
            return
        watchers = []
        try:
            for directory in running:
                try:
                    watchers.append(test_admission.DirectoryEvents(directory))
                except OSError:
                    pass
            # Subscribe before the second read so a fast summary replacement
            # cannot be lost between observation and waiting.
            if not any(_summary_status(directory) == "running" for directory in running):
                return
            if not watchers:
                raise RuntimeError("cannot observe legacy test summary completion")
            poller = select.poll()
            for watcher in watchers:
                poller.register(watcher.fd, select.POLLIN | select.POLLERR)
            poller.poll()
        finally:
            for watcher in watchers:
                watcher.close()


@contextlib.contextmanager
def drain_active_tests(*, socket_path: Path, runtime_dir: Path,
                       unit_prefix: str, daemon_running: bool):
    """Close admission and wait without using the installed Coordinator client."""
    test_admission, _config = _coordinator_runtime()
    lease = test_admission.begin_drain(runtime_dir, "coordinator upgrade")
    fence = runtime_dir / "daemon.pre-drain.sock"
    legacy = daemon_running and test_admission.read_activity(runtime_dir) is None
    fenced = False
    fence_guard = None
    try:
        if legacy:
            if fence.exists():
                raise RuntimeError(f"stale legacy drain socket exists: {fence}")
            fence_guard = subprocess.Popen(
                ["/usr/bin/python3", "-c", _LEGACY_FENCE_GUARD,
                 str(socket_path), str(fence)],
                stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, start_new_session=True,
            )
            os.replace(socket_path, fence)
            fenced = True
            _wait_legacy_tests(unit_prefix)
            if _config is not None:
                _wait_legacy_summaries(
                    _registered_test_directories(_config.database_path),
                    test_admission)
        elif daemon_running:
            test_admission.wait_for_zero_activity(runtime_dir)
        yield
    except BaseException:
        if fenced and fence.exists() and not socket_path.exists():
            os.replace(fence, socket_path)
            fenced = False
        raise
    finally:
        test_admission.end_drain(lease)
        if fence_guard is not None:
            assert fence_guard.stdin is not None
            fence_guard.stdin.close()
            try:
                fence_guard.wait(timeout=10)
            except subprocess.TimeoutExpired:
                fence_guard.kill()
                fence_guard.wait()
        elif fenced:
            fence.unlink(missing_ok=True)


def write_if_absent(path: Path, content: str, mode: int,
                    owner: tuple[int, int] = (0, 0)) -> bool:
    if path.exists():
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    os.chmod(path, mode)
    os.chown(path, *owner)
    return True


def ensure_env_value(path: Path, key: str, value: str) -> bool:
    text = path.read_text() if path.exists() else ""
    prefix = f"{key}="
    matches = [line for line in text.splitlines() if line.strip().startswith(prefix)]
    if matches:
        if matches != [f"{key}={value}"]:
            raise RuntimeError(f"{key} already has a different installed value")
        return False
    suffix = "" if not text or text.endswith("\n") else "\n"
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(text + suffix + f"{key}={value}\n")
    if path.exists():
        info = path.stat()
        os.chmod(tmp, info.st_mode & 0o777)
        os.chown(tmp, info.st_uid, info.st_gid)
    os.replace(tmp, path)
    return True


def set_env_value(path: Path, key: str, value: str) -> bool:
    text = path.read_text() if path.exists() else ""
    prefix = f"{key}="
    lines = text.splitlines()
    indexes = [index for index, line in enumerate(lines) if line.strip().startswith(prefix)]
    if len(indexes) > 1:
        raise RuntimeError(f"{key} appears more than once in {path}")
    desired = f"{key}={value}"
    if indexes and lines[indexes[0]] == desired:
        return False
    if indexes:
        lines[indexes[0]] = desired
    else:
        lines.append(desired)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text("\n".join(lines) + "\n")
    if path.exists():
        info = path.stat()
        os.chmod(tmp, info.st_mode & 0o777)
        os.chown(tmp, info.st_uid, info.st_gid)
    os.replace(tmp, path)
    return True


def compose_env_authorizations(specs: list[str]) -> list[dict[str, str]]:
    entries = []
    for spec in specs:
        repository_text, separator, relative = spec.partition("=")
        if not separator or not repository_text or not relative:
            raise ValueError(
                "--compose-env-authorization must be REPOSITORY=RELATIVE_PATH")
        repository = Path(repository_text).resolve()
        git = run([
            "git", "-c", "safe.directory=*", "-C", str(repository), "rev-parse",
            "--path-format=absolute", "--show-toplevel", "--git-common-dir",
        ])
        lines = git.stdout.splitlines()
        if len(lines) != 2 or Path(lines[1]).name != ".git":
            raise ValueError(f"unsupported repository layout: {repository}")
        worktree = Path(lines[0]).resolve()
        common_root = Path(lines[1]).resolve().parent
        parsed = Path(relative)
        if parsed.is_absolute() or "\\" in relative or "\0" in relative \
                or any(part in ("", ".", "..") for part in parsed.parts) \
                or parsed.as_posix() != relative:
            raise ValueError(
                "Compose environment authorization path must be normalized relative")
        candidate = worktree / relative
        candidate.lstat()
        resolved = candidate.resolve(strict=True)
        if candidate.is_symlink() or not candidate.is_file() \
                or (resolved != worktree and worktree not in resolved.parents):
            raise ValueError(f"unsafe Compose environment file: {candidate}")
        ignored = run([
            "git", "-c", "safe.directory=*", "-C", str(worktree),
            "check-ignore", "--quiet", "--", relative,
        ], check=False)
        if ignored.returncode != 0:
            raise ValueError(f"Compose environment file must be ignored: {candidate}")
        repository_id = "r" + hashlib.sha256(
            _REPOSITORY_NAMESPACE + str(common_root).encode("utf-8")
        ).hexdigest()[:16]
        entries.append({"repository_id": repository_id, "path": relative})
    unique = {(entry["repository_id"], entry["path"]): entry for entry in entries}
    if len(unique) != len(entries):
        raise ValueError("duplicate Compose environment authorization")
    return [unique[key] for key in sorted(unique)]


def merge_compose_env_allowlist(path: Path, entries: list[dict[str, str]],
                                owner: tuple[int, int]) -> bool:
    existing = []
    if path.exists():
        try:
            details = path.lstat()
            if not stat.S_ISREG(details.st_mode) or stat.S_ISLNK(details.st_mode):
                raise ValueError("policy must be a regular non-symlink file")
            document = json.loads(path.read_text())
            if document.get("schema") != 1 \
                    or not isinstance(document.get("authorizations"), list):
                raise ValueError("wrong schema")
            existing = document["authorizations"]
        except (OSError, json.JSONDecodeError, AttributeError, ValueError) as exc:
            raise RuntimeError(f"cannot preserve Compose environment allowlist: {exc}") from exc
    merged = {(item["repository_id"], item["path"]): item
              for item in [*existing, *entries]}
    payload = {"schema": 1,
               "authorizations": [merged[key] for key in sorted(merged)]}
    if path.exists() and json.loads(path.read_text()) == payload:
        details = path.lstat()
        changed = (details.st_uid, details.st_gid) != owner \
            or stat.S_IMODE(details.st_mode) != 0o640
        if changed:
            os.chown(path, *owner)
            os.chmod(path, 0o640)
        return changed
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    os.chown(tmp, *owner)
    os.chmod(tmp, 0o640)
    os.replace(tmp, path)
    return True


def codex_usage_sources(accounts: list[str]) -> list[dict[str, object]]:
    sources = []
    for name in accounts:
        entry = pwd.getpwnam(name)
        home = Path(entry.pw_dir)
        codex_home = home / ".codex"
        executable = home / ".local" / "bin" / "codex"
        if not codex_home.is_dir() or not executable.is_file():
            raise ValueError(f"Codex usage source is not installed for {name}")
        sources.append({
            "uid": entry.pw_uid,
            "codex_home": str(codex_home),
            "executable": str(executable),
        })
    return sources


def merge_codex_usage_sources(path: Path, entries: list[dict[str, object]],
                              owner: tuple[int, int]) -> bool:
    existing = []
    if path.exists():
        try:
            details = path.lstat()
            if not stat.S_ISREG(details.st_mode) or stat.S_ISLNK(details.st_mode):
                raise ValueError("policy must be a regular non-symlink file")
            document = json.loads(path.read_text())
            if document.get("schema") != 1 or not isinstance(document.get("sources"), list):
                raise ValueError("wrong schema")
            existing = document["sources"]
        except (OSError, json.JSONDecodeError, AttributeError, ValueError) as exc:
            raise RuntimeError(f"cannot preserve Codex usage source policy: {exc}") from exc
    merged = {int(item["uid"]): item for item in [*existing, *entries]}
    payload = {"schema": 1, "sources": [merged[uid] for uid in sorted(merged)]}
    if path.exists() and json.loads(path.read_text()) == payload:
        details = path.lstat()
        changed = (details.st_uid, details.st_gid) != owner \
            or stat.S_IMODE(details.st_mode) != 0o600
        if changed:
            os.chown(path, *owner)
            os.chmod(path, 0o600)
        return changed
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    os.chown(tmp, *owner)
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)
    return True


def validate_live_checkout(root: Path, *, fetch: bool) -> str:
    lexical = Path(os.path.abspath(root))
    root = lexical.resolve(strict=True)
    if lexical != root or lexical.is_symlink():
        raise RuntimeError(f"live checkout must be a real absolute directory: {lexical}")
    top = Path(run(["git", "-C", str(root), "rev-parse", "--show-toplevel"]).stdout.strip())
    if top.resolve() != root:
        raise RuntimeError(f"live checkout must be the Git worktree root: {root}")
    if fetch:
        run(["git", "-C", str(root), "fetch", "origin", "main"])
    branch = run(["git", "-C", str(root), "branch", "--show-current"]).stdout.strip()
    if branch != "main":
        raise RuntimeError(f"live checkout must be on main, found {branch or 'detached HEAD'}")
    status = run([
        "git", "-C", str(root), "status", "--porcelain", "--untracked-files=all",
    ]).stdout.strip()
    if status:
        raise RuntimeError("live checkout must be clean")
    head = run(["git", "-C", str(root), "rev-parse", "HEAD"]).stdout.strip()
    upstream = run([
        "git", "-C", str(root), "rev-parse", "refs/remotes/origin/main",
    ]).stdout.strip()
    if head != upstream:
        raise RuntimeError("live checkout must exactly match fetched origin/main")
    return head


def unit_daemon(source_root: Path) -> str:
    text = (source_root / "deploy" / "devcoordinator2.service").read_text()
    return text.replace("Environment=PYTHONPATH=/home/DevCoordinator2/src",
                        f"Environment=PYTHONPATH={source_root}/src")


def unit_edge(source_root: Path, canary: bool) -> str:
    text = (source_root / "deploy" / "devcoordinator2-edge.service").read_text()
    text = text.replace("/home/DevCoordinator2/edge/devcoordinator2-edge.mjs",
                        f"{source_root}/edge/devcoordinator2-edge.mjs")
    if canary:
        lines = [ln for ln in text.splitlines()
                 if not ln.startswith(("LoadCredential=tls", "LoadCredential=oidc",
                                       "Environment=EDGE_TLS_CERT",
                                       "Environment=EDGE_OIDC_CLIENT",
                                       "AmbientCapabilities", "CapabilityBoundingSet"))]
        text = "\n".join(lines) + "\n"
    return text


def enable_and_restart_units() -> None:
    for unit in ("devcoordinator2.service", "devcoordinator2-edge.service"):
        run(["systemctl", "enable", unit])
        run(["systemctl", "restart", unit])


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--base-domain", required=True)
    ap.add_argument("--admin-emails", required=True, help="comma-separated")
    ap.add_argument("--client-accounts", required=True, help="comma-separated Unix accounts")
    ap.add_argument("--client-group", default="devcoordinator2-clients")
    ap.add_argument("--canary", action="store_true",
                    help="edge http-only on --canary-port; no TLS/OIDC credentials needed")
    ap.add_argument("--canary-port", type=int, default=28080)
    ap.add_argument("--start", action="store_true", help="enable and start the units")
    ap.add_argument(
        "--compose-env-authorization", action="append", default=[],
        metavar="REPOSITORY=RELATIVE_PATH",
        help="authorize one ignored repository Compose interpolation file")
    ap.add_argument(
        "--codex-usage-account", action="append", default=[], metavar="UNIX_ACCOUNT",
        help="aggregate the account's default ~/.codex usage collector")
    ns = ap.parse_args()
    if os.geteuid() != 0:
        print("run as root", file=sys.stderr)
        return 2
    source_root = ROOT.resolve(strict=True)
    source_commit = validate_live_checkout(source_root, fetch=True)
    accounts = [a.strip() for a in ns.client_accounts.split(",") if a.strip()]
    ensure_group(ns.client_group, accounts)
    edge_uid, edge_gid = ensure_edge_user(ns.client_group)
    gid = grp.getgrnam(ns.client_group).gr_gid

    for path, mode, owner in ((Path("/run/devcoordinator2"), 0o755, (0, 0)),
                              (Path("/var/lib/devcoordinator2"), 0o751, (0, 0)),
                              (Path("/var/lib/devcoordinator2/public"), 0o755, (0, 0)),
                              # World-writable by owner decision
                              # DC2-2026-08-24-OPEN-LOCAL-ACCESS: bug intake
                              # must work from any account, sandboxed included.
                              (Path("/var/lib/devcoordinator2-bugs"), 0o777, (0, gid)),
                              (Path("/var/lib/devcoordinator2-edge"), 0o750,
                               (edge_uid, edge_gid))):
        path.mkdir(parents=True, exist_ok=True)
        os.chmod(path, mode)
        os.chown(path, *owner)
    # tmpfiles so /run survives reboots
    shutil.copy2(source_root / "deploy" / "devcoordinator2.tmpfiles.conf",
                 "/etc/tmpfiles.d/devcoordinator2.conf")

    created = []
    created.append(write_if_absent(ETC / "instance.env", (
        f"DEVCOORDINATOR2_BASE_DOMAIN={ns.base_domain}\n"
        f"DEVCOORDINATOR2_ADMIN_EMAILS={ns.admin_emails}\n"
        f"DEVCOORDINATOR2_CLIENT_GROUP={ns.client_group}\n"
        f"DEVCOORDINATOR2_EDGE_UID={edge_uid}\n"
        "DEVCOORDINATOR2_PORT_RANGE=20000-29999\n"
        "# DEVCOORDINATOR2_TELEGRAM_TOKEN_FILE=/etc/devcoordinator2/telegram.token\n"),
        0o640, (0, gid)))
    compose_authorizations = compose_env_authorizations(ns.compose_env_authorization)
    if compose_authorizations or COMPOSE_ENV_ALLOWLIST.exists():
        created.append(merge_compose_env_allowlist(
            COMPOSE_ENV_ALLOWLIST, compose_authorizations, (0, 0)))
        created.append(ensure_env_value(
            ETC / "instance.env", "DEVCOORDINATOR2_COMPOSE_ENV_ALLOWLIST_FILE",
            str(COMPOSE_ENV_ALLOWLIST)))
    usage_sources = codex_usage_sources(ns.codex_usage_account)
    if usage_sources or CODEX_USAGE_SOURCES.exists():
        created.append(merge_codex_usage_sources(
            CODEX_USAGE_SOURCES, usage_sources, (0, 0)))
        created.append(ensure_env_value(
            ETC / "instance.env", "DEVCOORDINATOR2_CODEX_USAGE_SOURCES_FILE",
            str(CODEX_USAGE_SOURCES)))
    edge_env = [f"EDGE_BASE_DOMAIN={ns.base_domain}",
                "EDGE_ROUTES_FILE=/var/lib/devcoordinator2/public/routes.json",
                "EDGE_DAEMON_SOCKET=/run/devcoordinator2/daemon.sock",
                f"EDGE_CONSOLE_DIR={source_root}/console"]
    if ns.canary:
        edge_env += ["EDGE_HTTP_ONLY=1", f"EDGE_HTTP_PORT={ns.canary_port}",
                     "EDGE_SESSION_SECRET_FILE=/etc/devcoordinator2/edge/session.secret",
                     "# OIDC for the canary: set EDGE_OIDC_CLIENT_ID_FILE /"
                     " EDGE_OIDC_CLIENT_SECRET_FILE once the redirect URI is registered"]
    created.append(write_if_absent(ETC / "edge.env", "\n".join(edge_env) + "\n", 0o640,
                                   (0, edge_gid)))
    created.append(set_env_value(ETC / "edge.env", "EDGE_CONSOLE_DIR",
                                 str(source_root / "console")))
    secret_dir = ETC / "edge"
    secret_dir.mkdir(parents=True, exist_ok=True)
    os.chmod(secret_dir, 0o750)
    os.chown(secret_dir, 0, edge_gid)
    created.append(write_if_absent(secret_dir / "session.secret", secrets.token_urlsafe(48),
                                   0o640, (0, edge_gid)))

    test_admission, instance_config = _coordinator_runtime()
    del test_admission
    daemon_running = instance_config.socket_path.exists()
    if daemon_running and not ns.start:
        raise RuntimeError("--start is required to activate an upgrade over a running daemon")
    skill_links: list[str] = []
    retired_skill_links: list[str] = []
    policy_links: list[str] = []
    with drain_active_tests(
            socket_path=instance_config.socket_path,
            runtime_dir=instance_config.socket_path.parent,
            unit_prefix=instance_config.unit_prefix,
            daemon_running=daemon_running):
        Path("/etc/systemd/system/devcoordinator2.service").write_text(
            unit_daemon(source_root))
        Path("/etc/systemd/system/devcoordinator2-edge.service").write_text(
            unit_edge(source_root, ns.canary))
        shim = Path("/usr/local/bin/devcoordinator2")
        shim.write_text("#!/usr/bin/python3\n"
                        "import sys\n"
                        f"sys.path.insert(0, '{source_root}/src')\n"
                        "from devcoordinator2.client.cli import main\n"
                        "sys.exit(main())\n")
        os.chmod(shim, 0o755)
        skill_links, retired_skill_links = install_skill_links(
            accounts, source_root / "skills")
        policy_links = install_policy_links(
            accounts, source_root / "reference" / "universal" / "AGENTS.md")
        run(["systemctl", "daemon-reload"])
        if ns.start:
            enable_and_restart_units()
    print({"source_root": str(source_root), "source_commit": source_commit,
           "edge_uid": edge_uid, "client_group": ns.client_group,
           "created_config": created, "canary": ns.canary,
           "canary_port": ns.canary_port if ns.canary else None, "started": ns.start,
           "skill_links": skill_links,
           "retired_skill_links": retired_skill_links,
           "policy_links": policy_links,
           "compose_env_authorizations": compose_authorizations,
           "codex_usage_source_count": len(usage_sources)})
    return 0


if __name__ == "__main__":
    sys.exit(main())
