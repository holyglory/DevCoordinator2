"""Deployment operations: apply, rollback, start/stop/restart, status, logs,
remove. Immediate execution; a concurrent mutation of one deployment gets
`busy`, never a queue."""

from __future__ import annotations

import json
import logging
import pwd
import threading
from datetime import UTC, datetime, timedelta
from pathlib import Path
from types import SimpleNamespace

from devcoordinator2.daemon import deploy_engine as eng
from devcoordinator2.daemon import deploy_runtime as rt
from devcoordinator2.daemon import deploy_state as st
from devcoordinator2.daemon import deploy_status as dstatus
from devcoordinator2.daemon import events, ports
from devcoordinator2.daemon.db import Database
from devcoordinator2.daemon.deploy_config import (
    ComponentSpec,
    DeploymentSpec,
    load_deployment_spec,
)
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.repoconfig import ConfigError
from devcoordinator2.daemon.server import Caller
from devcoordinator2.paths import InstanceConfig
from devcoordinator2.protocol import ProtocolError

log = logging.getLogger("devcoordinator2.deploy")


class Deployments:
    def __init__(self, config: InstanceConfig, db: Database, registry: Registry):
        self._config = config
        self._db = db
        self._registry = registry
        self._busy = eng.Busy()
        self._stop_expiry = threading.Event()

    # -- resolution ----------------------------------------------------------

    def _resolve(self, path: Path | None, name: str | None, dep_id: str | None,
                 caller: Caller) -> tuple[eng.Ctx, object, Path]:
        """Return (ctx, registration, worktree_root) for a deployment reference.

        By deployment_id the recorded worktree is used and no git runs as the
        caller; actions on behalf of a public identity execute as the account
        that created the deployment. By name a path is required and the
        caller's own account resolves the repository."""
        exec_uid, exec_gid = caller.uid, caller.gid
        if dep_id is not None:
            row = st.get_deployment(self._db, dep_id)
            if row is None:
                raise ProtocolError("deployment_not_found", f"no deployment {dep_id}")
            wt = self._db.query("SELECT worktree_path FROM worktrees WHERE worktree_id=?",
                                (row["worktree_id"],))
            if not wt:
                raise ProtocolError("deployment_not_found", "deployment worktree unregistered")
            worktree = Path(wt[0]["worktree_path"])
            reg = SimpleNamespace(repository_id=row["repository_id"],
                                  worktree_id=row["worktree_id"], worktree_path=str(worktree))
            name, source = row["name"], row["source"]
            if caller.identity is not None:
                exec_uid = row["created_by_uid"]
                exec_gid = pwd.getpwuid(exec_uid).pw_gid
        else:
            if path is None:
                raise ProtocolError("args_invalid", "'path' is required with 'name'")
            if caller.identity is not None:
                raise ProtocolError("permission_denied",
                                    "public callers address deployments by deployment_id")
            reg = eng.resolve_registration(self._registry, path, caller)
            worktree = Path(reg.worktree_path)
            if not name:
                raise ProtocolError("args_invalid", "'name' (or 'deployment_id') is required")
            name, _, source = name.partition("@")
        try:
            spec = load_deployment_spec(worktree, name)
        except ConfigError as exc:
            raise ProtocolError("repository_config_invalid", str(exc)) from exc
        if not source:
            if len(spec.sources) != 1:
                raise ProtocolError("args_invalid",
                                    f"deployment {name!r} enables {list(spec.sources)};"
                                    " address it as name@source")
            source = spec.sources[0]
        if source not in spec.sources:
            raise ProtocolError("repository_config_invalid",
                                f"deployment {name!r} does not enable source {source!r}")
        ctx = eng.Ctx(config=self._config, db=self._db,
                      dep_id=st.deployment_id(reg.worktree_id, name, source), spec=spec,
                      source=source, caller_uid=exec_uid, caller_gid=exec_gid,
                      client=caller.client_kind, session=caller.client_session)
        return ctx, reg, worktree

    def _existing(self, ctx: eng.Ctx) -> dict:
        row = st.get_deployment(self._db, ctx.dep_id)
        if row is None:
            raise ProtocolError("deployment_not_found",
                                f"{ctx.spec.name}@{ctx.source} was never applied")
        return row

    # -- apply / rollback ----------------------------------------------------

    def apply(self, path: Path | None, name: str | None, dep_id: str | None,
              caller: Caller) -> dict:
        ctx, reg, worktree = self._resolve(path, name, dep_id, caller)
        if ctx.caller_uid == 0:
            raise ProtocolError("deployment_apply_failed",
                                "repository code never runs as root; call as non-root")
        lock = self._busy.acquire(ctx.dep_id)
        try:
            commit, dirty = eng.head_commit(ctx, worktree)
            spec_fp = st.fingerprint({"spec": ctx.spec.canonical(ctx.source),
                                      "commit": commit, "dirty": dirty})
            row = st.get_deployment(self._db, ctx.dep_id)
            domain = ctx.spec.domain_for(ctx.source)
            if domain:
                owner = st.domain_owner(self._db, domain)
                if owner and owner != ctx.dep_id:
                    raise ProtocolError("deployment_apply_failed",
                                        f"domain {domain!r} is assigned to {owner}")
            if row and row["spec_fingerprint"] == spec_fp and row["state"] == "running" \
                    and not dirty:
                status = self._status(ctx, row)
                if status["state"] == "running":
                    status["unchanged"] = True
                    return status
            number = (row["current_generation"] if row else 0) or 0
            number += 1
            ttl = None
            if ctx.spec.ttl_seconds:
                ttl = (datetime.now(UTC) + timedelta(seconds=ctx.spec.ttl_seconds)
                       ).strftime("%Y-%m-%dT%H:%M:%SZ")
            old_rows = {c["name"]: c for c in st.components(self._db, ctx.dep_id)}
            st.upsert_deployment(self._db, dep_id=ctx.dep_id, reg=reg, name=ctx.spec.name,
                                 source=ctx.source, domain=domain, spec=ctx.spec,
                                 spec_fp=spec_fp, state="applying", caller_uid=ctx.caller_uid,
                                 client=caller.client_kind, ttl_expires_at=ttl)
            gen_path = eng.prepare_generation_path(ctx, worktree, number, commit)
            st.add_generation(self._db, ctx.dep_id, number, commit, dirty, gen_path, spec_fp)
            try:
                eng.run_build(ctx, gen_path)
            except ProtocolError:
                eng.remove_generation_path(ctx, worktree, gen_path)
                st.set_generation_state(self._db, ctx.dep_id, number, "failed")
                st.set_deployment(self._db, ctx.dep_id,
                                  state=row["state"] if row else "failed")
                raise
            return self._converge(ctx, worktree, row, old_rows, number, gen_path, domain)
        finally:
            lock.release()

    def rollback(self, path: Path, name: str | None, dep_id: str | None,
                 caller: Caller) -> dict:
        ctx, _, worktree = self._resolve(path, name, dep_id, caller)
        lock = self._busy.acquire(ctx.dep_id)
        try:
            row = self._existing(ctx)
            if ctx.source != "checkout":
                raise ProtocolError("rollback_unavailable",
                                    "worktree deployments keep no previous generation")
            prev = row["previous_generation"]
            gen = st.generation(self._db, ctx.dep_id, prev) if prev else None
            if gen is None or not Path(gen["path"]).is_dir():
                raise ProtocolError("rollback_unavailable", "no previous generation retained")
            number = row["current_generation"] + 1
            old_rows = {c["name"]: c for c in st.components(self._db, ctx.dep_id)}
            st.set_deployment(self._db, ctx.dep_id, state="applying")
            st.add_generation(self._db, ctx.dep_id, number, gen["commit_hash"],
                              bool(gen["dirty"]), Path(gen["path"]), gen["fingerprint"])
            st.set_deployment(self._db, ctx.dep_id, spec_fingerprint=gen["fingerprint"])
            return self._converge(ctx, worktree, row, old_rows, number, Path(gen["path"]),
                                  ctx.spec.domain_for(ctx.source),
                                  rollback=(row["current_generation"], prev))
        finally:
            lock.release()

    def _converge(self, ctx: eng.Ctx, worktree: Path, row: dict | None,
                  old_rows: dict[str, dict], number: int, gen_path: Path,
                  domain: str | None, rollback: tuple[int, int] | None = None) -> dict:
        db, spec = self._db, ctx.spec
        port_map: dict[str, int] = {}
        stable_ports = ports.assigned(db, ctx.dep_id, 0)
        for comp in spec.components:
            if not comp.wants_port or not st.is_owned(comp):
                continue
            if st.is_generation_scoped(comp):
                port_map[comp.name] = ports.lease(db, ctx.config.port_range, ctx.dep_id,
                                                  comp.name, number)
            else:
                port_map[comp.name] = stable_ports.get(comp.name) or ports.lease(
                    db, ctx.config.port_range, ctx.dep_id, comp.name, 0)
        started: list[tuple[ComponentSpec, tuple[str, str]]] = []
        try:
            for comp in spec.components:
                binding = self._bring_up(ctx, comp, number, gen_path, port_map, old_rows)
                if st.is_generation_scoped(comp) and binding[0] != "none":
                    started.append((comp, binding))
                ok, note = eng.prove_health(ctx, comp, binding, port_map)
                generation = number if st.is_generation_scoped(comp) else 0
                st.set_component(db, ctx.dep_id, comp.name, state="running" if ok else "failed",
                                 health="healthy" if ok else "unhealthy",
                                 generation=generation, binding_kind=binding[0],
                                 binding_identity=binding[1],
                                 spec_fingerprint=st.component_fingerprint(comp),
                                 last_error=None if ok else note)
                if not ok:
                    raise ProtocolError("deployment_apply_failed",
                                        f"component {comp.name} unhealthy: {note}")
        except ProtocolError as exc:
            self._abort_candidate(ctx, worktree, number, gen_path, started, old_rows)
            detail = json.dumps({"failed": exc.message,
                                 "components": self._component_states(ctx)})
            had_generation = bool(row and row["current_generation"])
            st.set_deployment(db, ctx.dep_id, state="degraded" if had_generation else "failed")
            events.publish("deployment.failed", deployment_id=ctx.dep_id, name=ctx.spec.name,
                           source=ctx.source, repository_id=row["repository_id"] if row else
                           None, message=exc.message, caller_uid=ctx.caller_uid)
            raise ProtocolError("deployment_apply_failed", exc.message, detail) from exc

        route_comp = spec.route_component
        if domain and route_comp:
            st.set_route(db, domain, ctx.dep_id, route_comp.name,
                         port_map[route_comp.name], number)
        else:
            st.set_route(db, None, ctx.dep_id, None, None, None)
        eng.publish_routes(ctx)
        prev = row["current_generation"] if row else None
        self._retire(ctx, worktree, spec, old_rows, prev, number)
        keep = {number} | ({prev} if prev and ctx.source == "checkout" else set())
        for stale in st.prune_generations(db, ctx.dep_id, keep):
            eng.remove_generation_path(ctx, worktree, Path(stale["path"]))
        if ctx.source != "checkout" and prev:
            ports.release(db, ctx.dep_id, generation=prev)
        st.set_generation_state(db, ctx.dep_id, number, "current")
        if prev and ctx.source == "checkout":
            st.set_generation_state(db, ctx.dep_id, prev, "previous")
        st.set_deployment(db, ctx.dep_id, state="running", current_generation=number,
                          previous_generation=prev if ctx.source == "checkout" else None)
        status = self._status(ctx, st.get_deployment(db, ctx.dep_id))
        if rollback is not None:
            status["rolled_back_from"], status["rolled_back_to"] = rollback
        events.publish("deployment.rolled_back" if rollback else "deployment.applied",
                       deployment_id=ctx.dep_id, name=ctx.spec.name, source=ctx.source,
                       repository_id=status["repository_id"], generation=number,
                       domain=status["domain"], caller_uid=ctx.caller_uid)
        return status

    def _bring_up(self, ctx: eng.Ctx, comp: ComponentSpec, number: int, gen_path: Path,
                  port_map: dict[str, int], old_rows: dict[str, dict]) -> tuple[str, str]:
        if not st.is_owned(comp):
            return "none", ""
        if st.is_generation_scoped(comp):
            return eng.start_component(ctx, comp, number, gen_path, port_map)
        old = old_rows.get(comp.name)
        unchanged = (old is not None and old["binding_identity"]
                     and old["spec_fingerprint"] == st.component_fingerprint(comp))
        if unchanged and comp.type in ("postgres", "docker"):
            state = rt.container_state(old["binding_identity"])
            if state["state"] == "running":
                return "container", old["binding_identity"]
            if state["state"] != "missing":
                try:
                    rt.start_container(old["binding_identity"])
                    return "container", old["binding_identity"]
                except rt.RuntimeError_:
                    pass
        return eng.start_component(ctx, comp, number, gen_path, port_map)

    def _abort_candidate(self, ctx, worktree, number, gen_path, started, old_rows) -> None:
        for comp, (kind, identity) in reversed(started):
            try:
                eng.stop_component_binding(kind, identity, comp, gen_path, None)
                if kind == "container":
                    rt.remove_container(identity, delete_volumes=False)
            except rt.RuntimeError_ as exc:
                log.error("abort cleanup failed for %s: %s", comp.name, exc)
            old = old_rows.get(comp.name)
            st.set_component(self._db, ctx.dep_id, comp.name,
                             state=old["state"] if old else "stopped",
                             generation=old["generation"] if old else None,
                             binding_kind=old["binding_kind"] if old else None,
                             binding_identity=old["binding_identity"] if old else None)
        ports.release(self._db, ctx.dep_id, generation=number)
        eng.remove_generation_path(ctx, worktree, gen_path)
        st.set_generation_state(self._db, ctx.dep_id, number, "failed")
        st.prune_generations(self._db, ctx.dep_id,
                             {n for n in (r["generation"] for r in old_rows.values()) if n}
                             | {0})

    def _retire(self, ctx: eng.Ctx, worktree: Path, spec: DeploymentSpec,
                old_rows: dict[str, dict], prev: int | None, number: int) -> None:
        """Stop the previous generation's generation-scoped bindings in reverse
        declared order, plus components removed from the declaration."""
        for cname in reversed(list(old_rows)):
            old = old_rows[cname]
            comp = spec.component(cname)
            if not old["binding_identity"]:
                continue
            scoped = comp is not None and st.is_generation_scoped(comp)
            removed = comp is None
            if not scoped and not removed:
                continue
            if scoped and old["generation"] == number:
                continue
            try:
                eng.stop_component_binding(old["binding_kind"], old["binding_identity"],
                                           comp, Path(worktree), None)
                if old["binding_kind"] == "container" and (scoped or removed):
                    rt.remove_container(old["binding_identity"], delete_volumes=False)
            except rt.RuntimeError_ as exc:
                log.error("retire %s failed: %s", cname, exc)
        if prev and ctx.source == "checkout":
            ports.release(self._db, ctx.dep_id, generation=prev)

    # -- start / stop / restart ---------------------------------------------

    def control(self, action: str, path: Path, name: str | None, dep_id: str | None,
                component: str | None, caller: Caller) -> dict:
        ctx, _, worktree = self._resolve(path, name, dep_id, caller)
        lock = self._busy.acquire(ctx.dep_id)
        try:
            row = self._existing(ctx)
            comps = list(ctx.spec.components)
            if component is not None:
                spec = ctx.spec.component(component)
                if spec is None:
                    raise ProtocolError("args_invalid", f"no component {component!r}")
                if not spec.independent_control:
                    raise ProtocolError("deployment_action_failed",
                                        f"component {component!r} declares independent_control"
                                        " = false")
                comps = [spec]
            if action in ("stop", "restart"):
                self._stop(ctx, row, worktree, list(reversed(comps)))
            if action in ("start", "restart"):
                self._start(ctx, row, worktree, comps)
            self._recompute_state(ctx, row)
            status = self._status(ctx, st.get_deployment(self._db, ctx.dep_id))
            events.publish(f"deployment.{action}", deployment_id=ctx.dep_id,
                           name=ctx.spec.name, source=ctx.source, component=component,
                           repository_id=status["repository_id"], state=status["state"],
                           caller_uid=ctx.caller_uid)
            return status
        finally:
            lock.release()

    def _stop(self, ctx: eng.Ctx, row: dict, worktree: Path,
              comps: list[ComponentSpec]) -> None:
        gen = st.generation(self._db, ctx.dep_id, row["current_generation"] or 0)
        gen_path = Path(gen["path"]) if gen else worktree
        rows = {c["name"]: c for c in st.components(self._db, ctx.dep_id)}
        for comp in comps:
            old = rows.get(comp.name)
            if not st.is_owned(comp) or not old or not old["binding_identity"]:
                continue
            try:
                eng.stop_component_binding(old["binding_kind"], old["binding_identity"], comp,
                                           gen_path, None)
                st.set_component(self._db, ctx.dep_id, comp.name, state="stopped",
                                 health="none", desired_state="stopped", last_error=None)
            except rt.RuntimeError_ as exc:
                st.set_component(self._db, ctx.dep_id, comp.name, state="failed",
                                 last_error=str(exc)[:512])
                raise ProtocolError("deployment_action_failed",
                                    f"stop {comp.name} failed: {exc}") from exc
        if ctx.spec.route_component and any(c.route for c in comps):
            st.set_route(self._db, ctx.spec.domain_for(ctx.source), ctx.dep_id,
                         ctx.spec.route_component.name, None, row["current_generation"])
            eng.publish_routes(ctx)

    def _start(self, ctx: eng.Ctx, row: dict, worktree: Path,
               comps: list[ComponentSpec]) -> None:
        number = row["current_generation"] or 0
        gen = st.generation(self._db, ctx.dep_id, number)
        gen_path = Path(gen["path"]) if gen else worktree
        port_map = {**ports.assigned(self._db, ctx.dep_id, 0),
                    **ports.assigned(self._db, ctx.dep_id, number)}
        rows = {c["name"]: c for c in st.components(self._db, ctx.dep_id)}
        for comp in comps:
            old = rows.get(comp.name)
            if not st.is_owned(comp):
                continue
            try:
                if old and old["binding_kind"] == "container" and old["binding_identity"] \
                        and rt.container_state(old["binding_identity"])["state"] != "missing":
                    rt.start_container(old["binding_identity"])
                    binding = ("container", old["binding_identity"])
                elif old and old["binding_kind"] == "compose":
                    rt.compose_up(old["binding_identity"], gen_path / comp.compose_file,
                                  gen_path, ctx.env_path(comp.name, number)
                                  if ctx.env_path(comp.name, number).exists() else None,
                                  comp.services)
                    binding = ("compose", old["binding_identity"])
                else:
                    binding = eng.start_component(ctx, comp, number, gen_path, port_map)
                ok, note = eng.prove_health(ctx, comp, binding, port_map)
            except (rt.RuntimeError_, ProtocolError) as exc:
                st.set_component(self._db, ctx.dep_id, comp.name, state="failed",
                                 health="unhealthy", desired_state="running",
                                 last_error=str(exc)[:512])
                events.publish("component.failed", deployment_id=ctx.dep_id,
                               component=comp.name, message=str(exc)[:256])
                raise ProtocolError("deployment_action_failed",
                                    f"start {comp.name} failed: {exc}") from exc
            st.set_component(self._db, ctx.dep_id, comp.name,
                             state="running" if ok else "failed",
                             health="healthy" if ok else "unhealthy",
                             desired_state="running", binding_kind=binding[0],
                             binding_identity=binding[1], last_error=None if ok else note)
            if not ok:
                raise ProtocolError("deployment_action_failed",
                                    f"component {comp.name} unhealthy after start: {note}")
        route = ctx.spec.route_component
        if route and any(c.route for c in comps):
            st.set_route(self._db, ctx.spec.domain_for(ctx.source), ctx.dep_id, route.name,
                         port_map.get(route.name), number)
            eng.publish_routes(ctx)

    def _recompute_state(self, ctx: eng.Ctx, row: dict) -> None:
        states = [c["state"] for c in st.components(self._db, ctx.dep_id)
                  if st.is_owned(ctx.spec.component(c["name"]) or ctx.spec.components[0])]
        if states and all(s == "running" for s in states):
            state = "running"
        elif all(s == "stopped" for s in states):
            state = "stopped"
        else:
            state = "degraded"
        st.set_deployment(self._db, ctx.dep_id, state=state)

    # -- status / list / logs (deploy_status.py) -----------------------------

    def _status(self, ctx: eng.Ctx, row: dict) -> dict:
        return dstatus.status(ctx, row)

    def _component_states(self, ctx: eng.Ctx) -> list[dict]:
        return dstatus.component_states(ctx)

    def status(self, path: Path, name: str | None, dep_id: str | None,
               caller: Caller) -> dict:
        ctx, _, _ = self._resolve(path, name, dep_id, caller)
        return dstatus.status(ctx, self._existing(ctx))

    def list(self, path: Path | None, caller: Caller) -> dict:
        return dstatus.list_all(self._db, self._registry, path, caller)

    def logs(self, path: Path, name: str | None, dep_id: str | None, component: str,
             tail_lines: int, caller: Caller) -> dict:
        ctx, _, worktree = self._resolve(path, name, dep_id, caller)
        return dstatus.logs(ctx, self._existing(ctx), worktree, component, tail_lines)

    # -- remove --------------------------------------------------------------

    def remove(self, path: Path, name: str | None, dep_id: str | None,
               delete_data: bool, caller: Caller) -> dict:
        ctx, _, worktree = self._resolve(path, name, dep_id, caller)
        lock = self._busy.acquire(ctx.dep_id)
        try:
            row = self._existing(ctx)
            self._stop(ctx, row, worktree, list(reversed(ctx.spec.components)))
            deleted_volumes = []
            for c in st.components(self._db, ctx.dep_id):
                spec = ctx.spec.component(c["name"])
                if c["binding_kind"] == "container" and c["binding_identity"]:
                    rt.remove_container(c["binding_identity"], delete_volumes=False)
                    if delete_data and spec is not None:
                        names = ["pgdata"] if spec.type == "postgres" else \
                            [v.split(":", 1)[0] for v in spec.volumes]
                        for v in names:
                            vol = rt.volume_name(ctx.dep_id, c["name"], v)
                            rt.remove_volume(vol)
                            deleted_volumes.append(vol)
                elif c["binding_kind"] == "compose" and spec is not None:
                    gen = st.generation(self._db, ctx.dep_id, row["current_generation"] or 0)
                    gp = Path(gen["path"]) if gen else worktree
                    rt.compose_down(c["binding_identity"], gp / spec.compose_file, gp, None,
                                    delete_volumes=delete_data)
            for gen in self._db.query("SELECT path FROM generations WHERE deployment_id=?",
                                      (ctx.dep_id,)):
                eng.remove_generation_path(ctx, worktree, Path(gen["path"]))
            st.delete_deployment_rows(self._db, ctx.dep_id)
            eng.publish_routes(ctx)
            if delete_data:
                st.delete_secrets(ctx.config, ctx.dep_id)
            for sub in ("env", "logs"):
                d = ctx.dir / sub
                if d.is_dir():
                    for f in d.iterdir():
                        f.unlink()
                    d.rmdir()
            if ctx.dir.is_dir() and not any(ctx.dir.iterdir()):
                ctx.dir.rmdir()
            events.publish("deployment.removed", deployment_id=ctx.dep_id,
                           name=ctx.spec.name, source=ctx.source,
                           repository_id=row["repository_id"], data_deleted=delete_data,
                           caller_uid=ctx.caller_uid)
            return {"deployment_id": ctx.dep_id, "removed": True,
                    "data_deleted": delete_data, "deleted_volumes": deleted_volumes}
        finally:
            lock.release()

    # -- preview expiry ------------------------------------------------------

    def start_expiry_thread(self) -> None:
        threading.Thread(target=self._expiry_loop, daemon=True).start()

    def _expiry_loop(self) -> None:
        while not self._stop_expiry.wait(60):
            try:
                self.expire_previews()
            except Exception:  # isolated periodic task; never blocks mutations
                log.exception("preview expiry failed")

    def expire_previews(self) -> list[str]:
        now = st.now_iso()
        expired = []
        for row in st.list_deployments(self._db):
            if row["ttl_expires_at"] and row["ttl_expires_at"] < now \
                    and row["state"] not in ("stopped",):
                expired.append(row["deployment_id"])
                caller = Caller(pid=0, uid=row["created_by_uid"], gid=0,
                                client_kind="other", client_session=None)
                wt = self._db.query("SELECT worktree_path FROM worktrees WHERE worktree_id=?",
                                    (row["worktree_id"],))
                if not wt:
                    continue
                try:
                    self.control("stop", Path(wt[0]["worktree_path"]), None,
                                 row["deployment_id"], None, caller)
                    events.publish("preview.expired", deployment_id=row["deployment_id"],
                                   name=row["name"], source=row["source"],
                                   repository_id=row["repository_id"])
                except ProtocolError as exc:
                    log.error("expiring %s failed: %s", row["deployment_id"], exc)
        return expired

    def shutdown(self) -> None:
        self._stop_expiry.set()
