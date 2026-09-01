def heartbeat():
    return {"healthy": True}


def purge_expired_sessions(session_store):
    removed = session_store.delete_expired()
    return {"removed": removed}


def render_purge_report(result):
    return f"Removed sessions: {result['removed']}"


def build_scheduler(scheduler, session_store):
    scheduler.hourly("heartbeat", heartbeat)
    return scheduler
