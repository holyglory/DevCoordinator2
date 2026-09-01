"""Periodic measurement: 15 s CPU/memory/IO per managed subject and host,
5 min storage, one-minute aggregates persisted, host reconciliation.

    managed repositories + DevCoordinator + shared/unattributed = host total

Runs in isolated daemon threads; a failure in one tick never blocks test or
deployment mutations."""

from __future__ import annotations

import json
import logging
import threading
import time
from datetime import UTC, datetime
from pathlib import Path

from devcoordinator2.daemon import events, metrics_postgres, metrics_store, systemd_unit
from devcoordinator2.daemon import metrics_sources as src
from devcoordinator2.daemon.alerts import AlertEngine, Condition
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.docker_cli import LABEL_PREFIX
from devcoordinator2.paths import InstanceConfig, test_dir

log = logging.getLogger("devcoordinator2.metrics")
SAMPLE_SECONDS = 15
STORAGE_SECONDS = 300
THRESHOLDS = {"host_cpu_percent": 90.0, "host_memory_available_fraction": 0.10,
              "host_disk_free_fraction": 0.10, "log_bytes": 1 << 30,
              "test_scratch_bytes": 10 << 30}


class Sampler:
    def __init__(self, config: InstanceConfig, db: Database):
        self._config = config
        self._db = db
        self._stop = threading.Event()
        self._lock = threading.Lock()
        self.current: dict[tuple[str, str], dict] = {}   # latest sample per subject
        self.storage: dict[tuple[str, str], dict] = {}   # latest storage per subject
        self.host: dict = {}
        self.meta: dict[tuple[str, str], dict] = {}      # attribution per subject
        self.alerts = AlertEngine(db)
        self._agg: dict[tuple[str, str, str], list] = {}
        self._minute: str | None = None
        self._prev: dict[tuple[str, str], tuple[int, float]] = {}
        self._prev_host: tuple[int, int] | None = None
        self._cgroup_cache: dict[str, Path | None] = {}
        self._restarts: dict[str, list[tuple[float, int]]] = {}
        self._last_expire = 0.0
        self._sample_wakeup = threading.Event()
        self._storage_wakeup = threading.Event()
        self._seen_containers: set[str] | None = None  # None until the first tick
        events.subscribe(self._on_event)

    def _on_event(self, event: dict) -> None:
        if event["kind"] in ("repository.registered", "deployment.applied",
                             "deployment.removed", "test.started"):
            self._sample_wakeup.set()
            self._storage_wakeup.set()

    # -- lifecycle -------------------------------------------------------------

    def start(self) -> None:
        threading.Thread(target=self._loop, args=(self.tick, SAMPLE_SECONDS),
                         daemon=True, name="metrics-sample").start()
        threading.Thread(target=self._storage_loop, daemon=True,
                         name="metrics-storage").start()

    def stop(self) -> None:
        self._stop.set()
        self._sample_wakeup.set()
        self._storage_wakeup.set()

    def _storage_loop(self) -> None:
        """Every 5 minutes, or sooner when a lifecycle event changes what
        exists (new repository, deployment applied/removed, test started)."""
        while not self._stop.is_set():
            self._storage_wakeup.clear()
            try:
                self.storage_tick()
            except Exception:
                log.exception("storage_tick failed")
            self._storage_wakeup.wait(STORAGE_SECONDS)
            if self._stop.is_set():
                break
            # Lifecycle events are published after their authoritative state
            # change, so the event itself is the signal to sample again.

    def _loop(self, fn, interval: int) -> None:
        while not self._stop.is_set():
            self._sample_wakeup.clear()
            started = time.monotonic()
            try:
                fn()
            except Exception:
                log.exception("%s failed", fn.__name__)
            elapsed = time.monotonic() - started
            self._sample_wakeup.wait(max(1.0, interval - elapsed))
            if self._stop.is_set():
                break

    # -- subject discovery -----------------------------------------------------

    def _cgroup_for_unit(self, unit: str) -> Path | None:
        if unit not in self._cgroup_cache:
            self._cgroup_cache[unit] = systemd_unit.control_group_path(unit)
        path = self._cgroup_cache[unit]
        if path is not None and not path.is_dir():
            self._cgroup_cache.pop(unit, None)
            path = systemd_unit.control_group_path(unit)
            self._cgroup_cache[unit] = path
        return path

    def _subjects(self) -> list[dict]:
        """Every measurable subject with attribution and cgroup path."""
        subjects = []
        worktrees = {r["worktree_id"]: r["repository_id"] for r in self._db.query(
            "SELECT worktree_id, repository_id FROM worktrees")}
        deployments = {r["deployment_id"]: dict(r) for r in self._db.query(
            "SELECT deployment_id, repository_id, name, source FROM deployments")}
        components = [dict(r) for r in self._db.query(
            "SELECT deployment_id, name, type, binding_kind, binding_identity FROM components")]
        # Test units: <prefix>-<worktree_id>-<run>.service
        prefix = self._config.unit_prefix + "-"
        for unit in systemd_unit.list_matching_units(f"{prefix}*.service"):
            rest = unit[len(prefix):]
            wt_id = rest.split("-", 1)[0]
            subjects.append({"kind": "test", "id": unit, "repository_id": worktrees.get(wt_id),
                             "cgroup": self._cgroup_for_unit(unit)})
        # Deployment components: units and containers are first-class subjects;
        # container-backed ones are excluded from reconciliation sums (the
        # container subject below already counts them once).
        for c in components:
            if not c["binding_identity"]:
                continue
            if c["binding_kind"] == "unit":
                cgroup = self._cgroup_for_unit(c["binding_identity"])
            elif c["binding_kind"] == "container":
                cgroup = src.container_cgroup(c["binding_identity"])
            else:
                cgroup = None
            dep = deployments.get(c["deployment_id"], {})
            subjects.append({"kind": "component",
                             "id": f"{c['deployment_id']}/{c['name']}",
                             "repository_id": dep.get("repository_id"),
                             "deployment_id": c["deployment_id"], "component": c["name"],
                             "type": c["type"], "binding": c["binding_identity"],
                             "cgroup": cgroup})
        # Containers (all on host)
        by_container = {c["binding_identity"]: c for c in components
                        if c["binding_kind"] == "container" and c["binding_identity"]}
        compose_projects = {c["binding_identity"]: c for c in components
                            if c["binding_kind"] == "compose" and c["binding_identity"]}
        observed_containers = {r["container_id"]: dict(r) for r in self._db.query(
            "SELECT container_id, repository_id, observed_deployment_id, compose_service"
            " FROM observed_containers")}
        seen_now: set[str] = set()
        for container in src.running_containers():
            labels = container["labels"]
            cid = container["id"]
            seen_now.add(cid)
            self._notice_container(container, labels, by_container, compose_projects,
                                   observed_containers)
            repo = None
            dep_id = labels.get(f"{LABEL_PREFIX}.deployment")
            comp = by_container.get(cid)
            project = labels.get("com.docker.compose.project")
            if labels.get(f"{LABEL_PREFIX}.instance") == self._config.unit_prefix:
                repo = labels.get(f"{LABEL_PREFIX}.repository") or None
            elif project in compose_projects:
                comp = compose_projects[project]
                dep_id = comp["deployment_id"]
                repo = deployments.get(dep_id, {}).get("repository_id")
            elif cid in observed_containers:
                imported = observed_containers[cid]
                dep_id = imported["observed_deployment_id"]
                repo = imported["repository_id"]
                comp = {"name": imported["compose_service"], "type": "observed-container"}
            cg = src.container_cgroup(cid) if container["state"] == "running" else None
            subjects.append({"kind": "container", "id": cid, "name": container["name"],
                             "repository_id": repo, "deployment_id": dep_id,
                             "component": comp["name"] if comp else None,
                             "type": comp["type"] if comp else None,
                             "image": container["image"], "state": container["state"],
                             "cgroup": cg})
        self._seen_containers = seen_now
        return subjects

    def _notice_container(self, container: dict, labels: dict, by_container: dict,
                          compose_projects: dict, observed_containers: dict) -> None:
        """Emit one event per newly observed unmanaged or orphaned container.
        The first tick seeds silently so a restart never floods."""
        cid = container["id"]
        if self._seen_containers is None or cid in self._seen_containers:
            return
        ours = labels.get(f"{LABEL_PREFIX}.instance") == self._config.unit_prefix
        project = labels.get("com.docker.compose.project")
        if ours:
            if labels.get(f"{LABEL_PREFIX}.purpose") != "test" and cid not in by_container:
                events.publish("container.orphaned_seen", container_id=cid,
                               name=container["name"], image=container["image"])
        elif project not in compose_projects and cid not in observed_containers:
            events.publish("container.unmanaged_seen", container_id=cid,
                           name=container["name"], image=container["image"])

    # -- 15 s tick -------------------------------------------------------------

    def tick(self) -> None:
        now = datetime.now(UTC)
        now_mono = time.monotonic()
        minute = metrics_store.minute_key(now)
        if self._minute is not None and minute != self._minute:
            self._flush()
        self._minute = minute

        busy, total = src.host_cpu_ticks()
        host_cpu = 0.0
        if self._prev_host is not None and total > self._prev_host[1]:
            host_cpu = 100.0 * (busy - self._prev_host[0]) / (total - self._prev_host[1])
        self._prev_host = (busy, total)
        mem = src.host_memory()
        load = src.host_load()
        fs = src.filesystem(Path("/"))
        host = {"cpu_percent": round(host_cpu, 2), "memory_total": mem["total"],
                "memory_used": mem["used"], "memory_available": mem["available"],
                "swap_total": mem["swap_total"],
                "swap_used": mem["swap_total"] - mem["swap_free"],
                "load_1": load[0], "load_5": load[1], "load_15": load[2],
                "fs_size": fs["size"], "fs_free": fs["free"], "fs_used": fs["used"],
                "ncpu": src.NCPU}
        self._record("host", "host", "cpu_percent", host_cpu)
        self._record("host", "host", "memory_used", mem["used"])
        self._record("host", "host", "load_1", load[0])

        current: dict[tuple[str, str], dict] = {}
        meta: dict[tuple[str, str], dict] = {}
        repo_totals: dict[str, dict[str, float]] = {}
        managed_cpu = managed_mem = 0.0
        subjects = self._subjects()
        for s in subjects:
            key = (s["kind"], s["id"])
            meta[key] = {k: v for k, v in s.items() if k != "cgroup"}
            stats = src.cgroup_stats(s.get("cgroup"))
            if stats is None:
                continue
            sample = self._delta_sample(key, stats, now_mono)
            current[key] = sample
            for metric, value in sample.items():
                self._record(s["kind"], s["id"], metric, value)
            # Reconciliation counts each cgroup once: components that are
            # containers are already counted as containers.
            if s["kind"] == "component" and s.get("type") in ("docker", "postgres", "compose"):
                continue
            if s["repository_id"]:
                totals = repo_totals.setdefault(s["repository_id"],
                                                {"cpu_percent": 0.0, "memory_bytes": 0.0,
                                                 "pids": 0.0})
                totals["cpu_percent"] += sample["cpu_percent"]
                totals["memory_bytes"] += sample["memory_bytes"]
                totals["pids"] += sample["pids"]
                managed_cpu += sample["cpu_percent"]
                managed_mem += sample["memory_bytes"]
        own = src.cgroup_stats(src.own_cgroup())
        daemon_cpu = daemon_mem = 0.0
        if own is not None:
            sample = self._delta_sample(("daemon", "daemon"), own, now_mono)
            current[("daemon", "daemon")] = sample
            daemon_cpu, daemon_mem = sample["cpu_percent"], sample["memory_bytes"]
            for metric, value in sample.items():
                self._record("daemon", "daemon", metric, value)
        for repo_id, totals in repo_totals.items():
            for metric, value in totals.items():
                self._record("repository", repo_id, metric, value)
            current[("repository", repo_id)] = totals
        other = {"cpu_percent": max(host_cpu - managed_cpu - daemon_cpu, 0.0),
                 "memory_bytes": max(mem["used"] - managed_mem - daemon_mem, 0.0)}
        current[("other", "other")] = other
        for metric, value in other.items():
            self._record("other", "other", metric, value)
        host["reconciliation"] = {"managed_cpu_percent": round(managed_cpu, 2),
                                  "daemon_cpu_percent": round(daemon_cpu, 2),
                                  "other_cpu_percent": round(other["cpu_percent"], 2),
                                  "managed_memory": int(managed_mem),
                                  "daemon_memory": int(daemon_mem),
                                  "other_memory": int(other["memory_bytes"])}
        with self._lock:
            self.current = current
            self.meta = meta
            self.host = host
        self._evaluate_alerts(host, current, meta)
        if now_mono - self._last_expire > 3600:
            self._last_expire = now_mono
            metrics_store.expire(self._db)

    def _delta_sample(self, key, stats: dict[str, int], now_mono: float) -> dict:
        prev = self._prev.get(key)
        cpu_percent = 0.0
        io_read = io_write = 0.0
        if prev is not None and now_mono > prev[1]:
            interval = now_mono - prev[1]
            cpu_percent = (stats["cpu_usec"] - prev[0]) / (interval * 1e6 * src.NCPU) * 100.0
            cpu_percent = max(cpu_percent, 0.0)
        self._prev[key] = (stats["cpu_usec"], now_mono)
        return {"cpu_percent": round(cpu_percent, 3), "cpu_usec_total": stats["cpu_usec"],
                "memory_bytes": stats["memory_current"], "memory_peak": stats["memory_peak"],
                "pids": stats["pids"], "io_read_bytes_total": stats["io_rbytes"],
                "io_write_bytes_total": stats["io_wbytes"], "io_read": io_read,
                "io_write": io_write}

    def _record(self, kind: str, sid: str, metric: str, value: float) -> None:
        if metric.endswith("_total") or metric in ("memory_peak",):
            return  # counters are not aggregated; derived rates are
        agg = self._agg.setdefault((kind, sid, metric), [value, 0.0, value, 0])
        agg[0] = min(agg[0], value)
        agg[1] += value
        agg[2] = max(agg[2], value)
        agg[3] += 1

    def _flush(self) -> None:
        aggregates = {k: (v[0], v[1], v[2], v[3]) for k, v in self._agg.items()}
        self._agg = {}
        try:
            metrics_store.flush(self._db, self._minute or "", aggregates)
        except Exception:
            log.exception("metric flush failed")

    # -- 5 min storage tick ----------------------------------------------------

    def storage_tick(self) -> None:
        storage: dict[tuple[str, str], dict] = {}
        repos = {r["repository_id"]: dict(r) for r in self._db.query(
            "SELECT repository_id, root_path FROM repositories"
            " WHERE archived_at IS NULL")}
        worktrees = [dict(r) for r in self._db.query(
            "SELECT worktree_id, repository_id, worktree_path FROM worktrees")]
        deployments = {r["deployment_id"]: dict(r) for r in self._db.query(
            "SELECT deployment_id, repository_id, name FROM deployments")}
        components = [dict(r) for r in self._db.query(
            "SELECT deployment_id, name, type, binding_kind, binding_identity"
            " FROM components")]
        per_repo: dict[str, dict[str, int]] = {
            rid: {"checkout": 0, "test_scratch": 0, "deployment_artifacts": 0,
                  "container_layers": 0, "volumes": 0, "postgres_data": 0}
            for rid in repos}
        for wt in worktrees:
            rid = wt["repository_id"]
            if rid not in per_repo:
                continue
            size = src.directory_size(Path(wt["worktree_path"]))
            if size is not None:
                per_repo[rid]["checkout"] += size
            scratch = src.directory_size(test_dir(Path(wt["worktree_path"])), timeout=60)
            if scratch is not None:
                per_repo[rid]["test_scratch"] += scratch
                storage[("worktree", wt["worktree_id"])] = {"test_scratch": scratch}
        for dep_id, dep in deployments.items():
            size = src.directory_size(self._config.deployments_dir / dep_id, timeout=60)
            if size is not None and dep["repository_id"] in per_repo:
                per_repo[dep["repository_id"]]["deployment_artifacts"] += size
                storage[("deployment", dep_id)] = {"artifacts": size}
        sizes = src.container_sizes()
        shared = src.docker_shared_sizes()
        volume_owner: dict[str, tuple[str, str]] = {}
        for c in components:
            dep = deployments.get(c["deployment_id"])
            if dep is None:
                continue
            prefix = f"devcoordinator2-{c['deployment_id']}-{c['name']}-"
            for vol in shared["volumes"]:
                if vol.startswith(prefix):
                    volume_owner[vol] = (dep["repository_id"], c["type"])
            if c["binding_kind"] == "container" and c["binding_identity"] in sizes:
                buckets = per_repo.setdefault(dep["repository_id"], {})
                buckets.setdefault("container_layers", 0)
                buckets["container_layers"] += sizes[c["binding_identity"]]
                storage[("component", f"{c['deployment_id']}/{c['name']}")] = {
                    "container_layer": sizes[c["binding_identity"]]}
            if c["type"] == "postgres" and c["binding_kind"] == "container" \
                    and c["binding_identity"]:
                creds = self._pg_identity(c)
                if creds:
                    facts = metrics_postgres.facts(c["binding_identity"], *creds)
                    if facts:
                        key = ("component", f"{c['deployment_id']}/{c['name']}")
                        storage.setdefault(key, {}).update(facts)
                        for metric, value in facts.items():
                            self._record("component", key[1], metric, value)
        for c in self._db.query(
                "SELECT container_id, repository_id, observed_deployment_id, compose_service"
                " FROM observed_containers"):
            if c["container_id"] not in sizes or c["repository_id"] not in per_repo:
                continue
            per_repo[c["repository_id"]]["container_layers"] += sizes[c["container_id"]]
            storage[("component", f"{c['observed_deployment_id']}/"
                                   f"{c['compose_service']}")] = {
                                       "container_layer": sizes[c["container_id"]]}
        shared_volumes = 0
        for vol, size in shared["volumes"].items():
            owner = volume_owner.get(vol)
            if owner and owner[0] in per_repo:
                bucket = "postgres_data" if owner[1] == "postgres" else "volumes"
                per_repo[owner[0]][bucket] += size
            else:
                shared_volumes += size
        for rid, buckets in per_repo.items():
            total = sum(buckets.values())
            storage[("repository", rid)] = {**buckets, "total": total}
            self._record("repository", rid, "storage_bytes", total)
        state_size = src.directory_size(self._config.state_dir, timeout=60) or 0
        fs = src.filesystem(Path("/"))
        # Persisted so the Console can chart storage over 24h/7d/30d windows.
        self._record("host", "host", "storage_bytes", float(fs["used"]))
        managed_total = sum(s["total"] for k, s in storage.items() if k[0] == "repository")
        docker_shared = shared["images"] + shared["build_cache"] + shared_volumes
        storage[("host", "storage")] = {
            "fs_used": fs["used"], "managed_repositories": managed_total,
            "devcoordinator_state": state_size, "docker_shared": docker_shared,
            "docker_images": shared["images"], "docker_build_cache": shared["build_cache"],
            "docker_shared_volumes": shared_volumes,
            "other": max(fs["used"] - managed_total - state_size - docker_shared, 0),
        }
        with self._lock:
            self.storage = storage

    def _pg_identity(self, component: dict) -> tuple[str, str] | None:
        path = (self._config.secrets_dir / component["deployment_id"]
                / f"{component['name']}.json")
        try:
            data = json.loads(path.read_text())
            return data["user"], data["database"]
        except (OSError, json.JSONDecodeError, KeyError):
            return None

    # -- alerts ----------------------------------------------------------------

    def _evaluate_alerts(self, host: dict, current: dict, meta: dict) -> None:
        conditions = [
            Condition("host/cpu", "host_cpu", "host", "host", "warning",
                      f"host CPU {host['cpu_percent']:.0f}% sustained",
                      host["cpu_percent"] > THRESHOLDS["host_cpu_percent"], 300),
            Condition("host/memory", "host_memory", "host", "host", "critical",
                      "host memory available below 10%",
                      host["memory_total"] > 0 and host["memory_available"]
                      < host["memory_total"] * THRESHOLDS["host_memory_available_fraction"],
                      300),
            Condition("host/disk", "host_disk", "host", "host", "critical",
                      "root filesystem below 10% free",
                      host["fs_size"] > 0 and host["fs_free"]
                      < host["fs_size"] * THRESHOLDS["host_disk_free_fraction"], 0),
        ]
        rows = self._db.query(
            "SELECT deployment_id, name, state, desired_state, binding_kind, binding_identity"
            " FROM components")
        now_mono = time.monotonic()
        for c in rows:
            sid = f"{c['deployment_id']}/{c['name']}"
            unhealthy = c["desired_state"] == "running" and c["state"] != "running"
            conditions.append(Condition(f"component/{sid}/unhealthy", "component_unhealthy",
                                        "component", sid, "critical",
                                        f"component {sid} is {c['state']}", unhealthy, 120))
            restarts = None
            if c["binding_kind"] == "unit" and c["binding_identity"]:
                props = systemd_unit.show_unit(c["binding_identity"], ["NRestarts"])
                restarts = int(props.get("NRestarts") or 0)
            if restarts is not None:
                history = self._restarts.setdefault(sid, [])
                history.append((now_mono, restarts))
                self._restarts[sid] = [h for h in history if now_mono - h[0] <= 600]
                delta = restarts - self._restarts[sid][0][1]
                conditions.append(Condition(f"component/{sid}/crashloop", "crash_loop",
                                            "component", sid, "critical",
                                            f"component {sid} restarted {delta}x in 10 min",
                                            delta >= 3, 0))
        for key, storage in list(self.storage.items()):
            if key[0] != "worktree":
                continue
            if storage.get("test_scratch", 0) > THRESHOLDS["test_scratch_bytes"]:
                conditions.append(Condition(f"worktree/{key[1]}/scratch", "test_scratch",
                                            "worktree", key[1], "warning",
                                            "test scratch exceeds 10 GiB", True, 0))
        self.alerts.evaluate(conditions)

    # -- snapshots for the API -------------------------------------------------

    def snapshot(self) -> dict:
        with self._lock:
            return {"host": dict(self.host), "current": dict(self.current),
                    "storage": dict(self.storage), "meta": dict(self.meta)}
