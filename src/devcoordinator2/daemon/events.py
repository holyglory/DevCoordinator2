"""Tiny in-process event bus: lifecycle and health events for notification
surfaces (Telegram in Phase 6) and the Console. Subscribers must be fast and
never raise into the publisher; failures are isolated."""

from __future__ import annotations

import logging
import threading
from collections.abc import Callable
from datetime import UTC, datetime

log = logging.getLogger("devcoordinator2.events")
Subscriber = Callable[[dict], None]

_subscribers: list[Subscriber] = []
_lock = threading.Lock()


def subscribe(fn: Subscriber) -> None:
    with _lock:
        _subscribers.append(fn)


def publish(kind: str, **fields) -> dict:
    event = {"kind": kind, "at": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"), **fields}
    with _lock:
        targets = list(_subscribers)
    for fn in targets:
        try:
            fn(event)
        except Exception:
            log.exception("event subscriber failed for %s", kind)
    return event
