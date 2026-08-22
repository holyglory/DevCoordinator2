"""Daemon entry point: init, restart recovery, serve."""

from __future__ import annotations

import logging
import signal
import sys

from devcoordinator2.daemon.db import Database, SchemaMismatch
from devcoordinator2.daemon.handlers import build_handlers
from devcoordinator2.daemon.registry import Registry
from devcoordinator2.daemon.server import Server
from devcoordinator2.daemon.tests_lifecycle import TestLifecycle
from devcoordinator2.paths import load_instance_config

log = logging.getLogger("devcoordinator2")


def main() -> int:
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    config = load_instance_config()
    try:
        db = Database(config.database_path)
    except SchemaMismatch as exc:
        log.error("refusing to start: %s", exc)
        return 1
    registry = Registry(db)
    lifecycle = TestLifecycle(config, registry)
    lifecycle.recover()
    handlers = build_handlers(config, registry, lifecycle)
    server = Server(config.socket_path, handlers,
                    client_group=config.client_group)
    server.bind()

    def _shutdown(signum, frame):
        log.info("signal %s: shutting down", signum)
        server.shutdown()

    signal.signal(signal.SIGTERM, _shutdown)
    signal.signal(signal.SIGINT, _shutdown)
    log.info("serving on %s", config.socket_path)
    server.serve_forever()
    db.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
