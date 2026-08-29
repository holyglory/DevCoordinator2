"""Daemon entry point: init, restart recovery, serve."""

from __future__ import annotations

import logging
import signal
import sys

from devcoordinator2.daemon import events
from devcoordinator2.daemon.access import Access, guard, public_commands
from devcoordinator2.daemon.db import Database, SchemaMismatch
from devcoordinator2.daemon.deploy_control import Deployments
from devcoordinator2.daemon.handlers import build_handlers, build_notification_handlers
from devcoordinator2.daemon.health_api import build_health_handlers
from devcoordinator2.daemon.metrics_sampler import Sampler
from devcoordinator2.daemon.plan_api import build_plan_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Server
from devcoordinator2.daemon.telegram import Telegram
from devcoordinator2.daemon.tests_lifecycle import TestLifecycle
from devcoordinator2.paths import load_instance_config

log = logging.getLogger("devcoordinator2")


def main() -> int:
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    config = load_instance_config(load_compose_authorizations=True)
    try:
        db = Database(config.database_path)
    except SchemaMismatch as exc:
        log.error("refusing to start: %s", exc)
        return 1
    registry = Registry(db)
    lifecycle = TestLifecycle(config, registry)
    lifecycle.recover()
    deployments = Deployments(config, db, registry)
    deployments.start_expiry_thread()
    handlers = build_handlers(config, registry, lifecycle, deployments, db)
    sampler = Sampler(config, db)
    sampler.start()
    handlers.update(build_health_handlers(config, db, registry, sampler))
    handlers.update(build_plan_handlers(config, db, registry))
    access = Access(config, db)
    access.republish()  # the edge always has a current document after a (re)start
    handlers.update(public_commands(access))
    telegram = Telegram(config, db)
    handlers.update(build_notification_handlers(config, telegram, access))
    handlers = guard(handlers, access, db)
    telegram.start()
    server = Server(config.socket_path, handlers,
                    client_group=config.client_group, edge_uid=config.edge_uid)
    server.bind()

    def _shutdown(signum, frame):
        log.info("signal %s: shutting down", signum)
        server.shutdown()

    signal.signal(signal.SIGTERM, _shutdown)
    signal.signal(signal.SIGINT, _shutdown)
    log.info("serving on %s", config.socket_path)
    events.publish("coordinator.started", socket=str(config.socket_path))
    server.serve_forever()
    telegram.stop()
    deployments.shutdown()
    sampler.stop()
    db.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
