# Session maintenance schedule

## Requirements

- `build_scheduler(scheduler, session_store)` must register both the hourly
  heartbeat and `purge_expired_sessions` as a daily job. Installing the
  scheduler is the production entry point for both operations.
- `render_purge_report(result)` is a library formatting helper used by an
  on-demand report. It is not a scheduled task and must not be registered with
  the scheduler.
