#!/usr/bin/env python3
"""Install a DevCoordinator2 release beside the legacy system (run as root).

Creates: /opt/devcoordinator2/releases/<id> (+ `current` symlink), the
client group and edge system user, state directories, instance
configuration templates (only if absent), the daemon and edge units, the
/usr/local/bin/devcoordinator2 shim, and Codex/Claude skill links for client
accounts that already have those agent roots. In `--canary` mode the edge
runs http-only on a private port and needs no TLS/OIDC credentials, so the
legacy edge keeps 80/443 untouched. Every installation-specific value is an
argument; nothing here names an installation.
"""

from __future__ import annotations

import argparse
import grp
import hashlib
import json
import os
import pwd
import secrets
import shutil
import stat
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OPT = Path("/opt/devcoordinator2")
ETC = Path("/etc/devcoordinator2")
RELEASE_ITEMS = ("src", "edge", "console", "deploy", "scripts", "skills",
                 "pyproject.toml")
COMPOSE_ENV_ALLOWLIST = ETC / "compose-env-allowlist.json"
_REPOSITORY_NAMESPACE = b"devcoordinator2.repository\0"


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


def install_skill_links(accounts: list[str], release_skill: Path) -> list[str]:
    installed: list[str] = []
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
            link = skills_dir / "codex-dev-coordinator"
            if link.exists() and not link.is_symlink():
                raise RuntimeError(f"refusing to replace non-symlink skill: {link}")
            tmp = skills_dir / ".codex-dev-coordinator.tmp"
            if tmp.is_symlink() or tmp.exists():
                if not tmp.is_symlink():
                    raise RuntimeError(f"refusing to replace non-symlink staging path: {tmp}")
                tmp.unlink()
            tmp.symlink_to(release_skill)
            os.replace(tmp, link)
            installed.append(str(link))
    return installed


def install_release(release_id: str) -> Path:
    target = OPT / "releases" / release_id
    if target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True)
    for item in RELEASE_ITEMS:
        src = ROOT / item
        if src.is_dir():
            shutil.copytree(src, target / item, ignore=shutil.ignore_patterns(
                "__pycache__", ".pytest_cache", "test", "tests"))
        else:
            shutil.copy2(src, target / item)
    # World-readable release: the edge user and every client account read it.
    for dirpath, _dirnames, filenames in os.walk(target):
        os.chmod(dirpath, 0o755)
        for name in filenames:
            path = Path(dirpath) / name
            os.chmod(path, 0o755 if os.access(path, os.X_OK) else 0o644)
    os.chmod(OPT, 0o755)
    os.chmod(OPT / "releases", 0o755)
    current = OPT / "current"
    tmp = OPT / "current.tmp"
    if tmp.is_symlink() or tmp.exists():
        tmp.unlink()
    tmp.symlink_to(target)
    os.replace(tmp, current)
    return target


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


def unit_daemon(release: Path) -> str:
    text = (release / "deploy" / "devcoordinator2.service").read_text()
    return text.replace("Environment=PYTHONPATH=/opt/devcoordinator2/src",
                        f"Environment=PYTHONPATH={OPT}/current/src")


def unit_edge(release: Path, canary: bool) -> str:
    text = (release / "deploy" / "devcoordinator2-edge.service").read_text()
    text = text.replace("/opt/devcoordinator2/edge/devcoordinator2-edge.mjs",
                        f"{OPT}/current/edge/devcoordinator2-edge.mjs")
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
    ap.add_argument("--release-id", default=None, help="default: git HEAD short id")
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
    ns = ap.parse_args()
    if os.geteuid() != 0:
        print("run as root", file=sys.stderr)
        return 2
    release_id = ns.release_id or run(["git", "-C", str(ROOT), "rev-parse", "--short",
                                       "HEAD"]).stdout.strip()
    accounts = [a.strip() for a in ns.client_accounts.split(",") if a.strip()]
    ensure_group(ns.client_group, accounts)
    edge_uid, edge_gid = ensure_edge_user(ns.client_group)
    release = install_release(release_id)
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
    shutil.copy2(release / "deploy" / "devcoordinator2.tmpfiles.conf",
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
    edge_env = [f"EDGE_BASE_DOMAIN={ns.base_domain}",
                "EDGE_ROUTES_FILE=/var/lib/devcoordinator2/public/routes.json",
                "EDGE_DAEMON_SOCKET=/run/devcoordinator2/daemon.sock",
                f"EDGE_CONSOLE_DIR={OPT}/current/console"]
    if ns.canary:
        edge_env += ["EDGE_HTTP_ONLY=1", f"EDGE_HTTP_PORT={ns.canary_port}",
                     "EDGE_SESSION_SECRET_FILE=/etc/devcoordinator2/edge/session.secret",
                     "# OIDC for the canary: set EDGE_OIDC_CLIENT_ID_FILE /"
                     " EDGE_OIDC_CLIENT_SECRET_FILE once the redirect URI is registered"]
    created.append(write_if_absent(ETC / "edge.env", "\n".join(edge_env) + "\n", 0o640,
                                   (0, edge_gid)))
    secret_dir = ETC / "edge"
    secret_dir.mkdir(parents=True, exist_ok=True)
    os.chmod(secret_dir, 0o750)
    os.chown(secret_dir, 0, edge_gid)
    created.append(write_if_absent(secret_dir / "session.secret", secrets.token_urlsafe(48),
                                   0o640, (0, edge_gid)))

    Path("/etc/systemd/system/devcoordinator2.service").write_text(unit_daemon(release))
    Path("/etc/systemd/system/devcoordinator2-edge.service").write_text(
        unit_edge(release, ns.canary))
    shim = Path("/usr/local/bin/devcoordinator2")
    shim.write_text("#!/usr/bin/python3\n"
                    "import sys\n"
                    f"sys.path.insert(0, '{OPT}/current/src')\n"
                    "from devcoordinator2.client.cli import main\n"
                    "sys.exit(main())\n")
    os.chmod(shim, 0o755)
    skill_links = install_skill_links(
        accounts, OPT / "current" / "skills" / "codex-dev-coordinator")
    run(["systemctl", "daemon-reload"])
    if ns.start:
        enable_and_restart_units()
    print({"release": str(release), "edge_uid": edge_uid, "client_group": ns.client_group,
           "created_config": created, "canary": ns.canary,
           "canary_port": ns.canary_port if ns.canary else None, "started": ns.start,
           "skill_links": skill_links,
           "compose_env_authorizations": compose_authorizations})
    return 0


if __name__ == "__main__":
    sys.exit(main())
