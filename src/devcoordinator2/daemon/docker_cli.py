"""Docker CLI wrapper: argv-only, label-scoped, exact-ID operations.

The daemon is the only component that invokes Docker for managed work. It
injects labels callers cannot override, records the full container ID Docker
returns, and removes only exact IDs it recorded or containers carrying its
own instance/purpose labels. Docker prune is never used.
"""

from __future__ import annotations

import json
import subprocess

LABEL_PREFIX = "devcoordinator2"
_TIMEOUT = 120
_IMAGE_PULL_TIMEOUT = 600


class DockerError(Exception):
    """Docker invocation failure with a bounded diagnostic."""


def _run(argv: list[str], env: dict[str, str] | None = None,
         timeout: int = _TIMEOUT) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(["docker", *argv], capture_output=True, text=True,
                              timeout=timeout, check=False, env=env)
    except FileNotFoundError as exc:
        raise DockerError("docker CLI not installed") from exc
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise DockerError(f"docker {argv[0]} failed: {exc}") from exc


def available() -> bool:
    try:
        return _run(["version", "--format", "{{.Server.Version}}"],
                    timeout=15).returncode == 0
    except DockerError:
        return False


def ensure_digest_image(image: str) -> None:
    """Ensure one immutable image reference exists and resolves to its digest.

    Mutable tags deliberately stay on the established preloaded-image path.
    Compatible database fixtures use this before ``docker run --pull never``
    so the daemon, rather than repository code, owns the only network pull.
    """
    if "@sha256:" not in image:
        return
    requested = image.rsplit("@", 1)[1]
    if _image_has_digest(image, requested):
        return
    pull = _run(["pull", "--quiet", image], timeout=_IMAGE_PULL_TIMEOUT)
    if pull.returncode != 0:
        raise DockerError(pull.stderr.strip()[:1024] or
                          "cannot pull the pinned database fixture image")
    if not _image_has_digest(image, requested):
        raise DockerError("pulled database fixture image did not resolve to the"
                          " requested sha256 digest")


def _image_has_digest(image: str, requested: str) -> bool:
    proc = _run(["image", "inspect", "--format", "{{json .RepoDigests}}", image],
                timeout=30)
    if proc.returncode != 0:
        return False
    try:
        digests = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return False
    return isinstance(digests, list) and any(
        isinstance(value, str) and value.rsplit("@", 1)[-1] == requested
        for value in digests
    )


def managed_labels(*, instance: str, repository_id: str, worktree_id: str,
                   run_id: str, purpose: str, caller_uid: int, client: str,
                   session: str | None, created_at: str,
                   data_class: str) -> dict[str, str]:
    labels = {
        f"{LABEL_PREFIX}.instance": instance,
        f"{LABEL_PREFIX}.repository": repository_id,
        f"{LABEL_PREFIX}.worktree": worktree_id,
        f"{LABEL_PREFIX}.run": run_id,
        f"{LABEL_PREFIX}.purpose": purpose,
        f"{LABEL_PREFIX}.caller_uid": str(caller_uid),
        f"{LABEL_PREFIX}.client": client,
        f"{LABEL_PREFIX}.created": created_at,
        f"{LABEL_PREFIX}.data": data_class,
    }
    if session:
        labels[f"{LABEL_PREFIX}.session"] = session[:128]
    return labels


def run_detached(*, name: str, image: str, labels: dict[str, str],
                 env_names: list[str], env_values: dict[str, str],
                 publish: list[str], tmpfs: list[str],
                 command: list[str] | None = None) -> str:
    """`docker run -d`; secrets travel via the process environment (-e NAME),
    never via argv. Returns the full 64-hex container ID."""
    argv = ["run", "--detach", "--name", name, "--pull", "never"]
    for key, value in labels.items():
        argv += ["--label", f"{key}={value}"]
    for key in env_names:
        argv += ["--env", key]
    for spec in publish:
        argv += ["--publish", spec]
    for spec in tmpfs:
        argv += ["--tmpfs", spec]
    argv.append(image)
    if command:
        argv += command
    proc = _run(argv, env={"PATH": "/usr/bin:/bin", **env_values})
    if proc.returncode != 0:
        raise DockerError(proc.stderr.strip()[:1024] or "docker run failed")
    container_id = proc.stdout.strip()
    if len(container_id) != 64:
        raise DockerError(f"unexpected docker run output: {container_id[:80]!r}")
    return container_id


def inspect(container_id: str) -> dict:
    proc = _run(["inspect", "--format", "{{json .}}", container_id], timeout=30)
    if proc.returncode != 0:
        raise DockerError(proc.stderr.strip()[:512] or "docker inspect failed")
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise DockerError("docker inspect returned invalid JSON") from exc


def published_host_port(container_id: str, container_port: str) -> int:
    info = inspect(container_id)
    ports = (info.get("NetworkSettings") or {}).get("Ports") or {}
    bindings = ports.get(container_port) or []
    for binding in bindings:
        try:
            return int(binding["HostPort"])
        except (KeyError, TypeError, ValueError):
            continue
    raise DockerError(f"container publishes no host port for {container_port}")


def exec_ok(container_id: str, argv: list[str], timeout: int = 15) -> bool:
    proc = _run(["exec", container_id, *argv], timeout=timeout)
    return proc.returncode == 0


def follow_logs(container_id: str) -> subprocess.Popen:
    """Stream exact container logs; existing records are included before follow."""
    if len(container_id) != 64:
        raise DockerError("refusing logs for a non-exact container reference")
    try:
        return subprocess.Popen(
            ["docker", "logs", "--follow", container_id],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True,
        )
    except OSError as exc:
        raise DockerError(f"docker logs failed: {exc}") from exc


def remove_exact(container_id: str) -> None:
    """Remove one exact full ID with its anonymous volumes; idempotent."""
    if len(container_id) != 64:
        raise DockerError("refusing to remove a non-exact container reference")
    proc = _run(["rm", "--force", "--volumes", container_id], timeout=60)
    if proc.returncode != 0 and "No such container" not in proc.stderr:
        raise DockerError(proc.stderr.strip()[:512] or "docker rm failed")


def list_ids_by_labels(labels: dict[str, str]) -> list[str]:
    """Full IDs of all containers (any state) carrying every given label."""
    argv = ["ps", "--all", "--no-trunc", "--quiet"]
    for key, value in labels.items():
        argv += ["--filter", f"label={key}={value}"]
    proc = _run(argv, timeout=30)
    if proc.returncode != 0:
        raise DockerError(proc.stderr.strip()[:512] or "docker ps failed")
    return [line.strip() for line in proc.stdout.splitlines()
            if len(line.strip()) == 64]
