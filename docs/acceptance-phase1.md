# Phase 1 Acceptance Record

Executed 2026-08-22 against a real root daemon on a private socket
(`devcoordinator2-accept-*` unit namespace), driven through the installed
CLI and MCP surfaces, plus the root integration suite
(`sudo DEVCOORDINATOR2_ROOT_TESTS=1 .venv/bin/pytest tests/integration`).

| # | Checklist item (Requirements.md) | Result | Evidence |
|---|---|---|---|
| 1 | Immediate `running` or terminal failure; no `queued` anywhere | PASS | acceptance 1a–1d |
| 2 | Latest-start-wins: one unit remains, prior directory replaced | PASS | acceptance 2; integration `test_supersession_latest_start_wins` |
| 3 | Child runs with all four caller UIDs | PASS | acceptance 3/3b; integration `test_pass_uid_and_bounded_output` |
| 4 | Timeout → `timed-out`, stop → `cancelled`, cgroup proven empty | PASS | acceptance 4a/4b; integration timeout/cancel tests |
| 5 | Successful status carries no log text; bounded explicit tails | PASS | acceptance 5a/5b |
| 6 | summary.json schema complete, atomic under crash injection | PASS | acceptance 6; unit `test_summary_atomic.py` |
| 7 | Daemon SIGKILL + restart → `interrupted`, never resurrected | PASS | integration `test_daemon_restart_marks_interrupted` |
| 8 | Peer UID recorded; body-asserted identity rejected; root caller refused | PASS | unit `test_server.py`; integration `test_root_caller_rejected` |
| 9 | CLI and MCP return identical result JSON | PASS | acceptance 9 |
| 10 | Sanitization gate clean over committable content | PASS | acceptance 10 |

Suite status at record time: 61 unit tests passed, 8 root integration tests
passed, `ruff check` clean.

Deliberately not yet done (later phases / cutover): wiring the MCP server
into a real agent client's configuration, installing the systemd unit as a
permanent service, and any deployment/health/edge/Telegram/bug surface.

## Phase 2 addendum (2026-08-22)

| Item | Result | Evidence |
|---|---|---|
| Ephemeral PostgreSQL reachable by the test via injected env; real SQL round-trip | PASS | integration `test_postgres_real_query_labels_secrecy_and_cleanup` |
| Container carries exact run/repository/caller/client/data labels | PASS | same |
| Credentials absent from unit `Environment`, summary, and status; env file 0600 caller-owned | PASS | same |
| Container removed on completion, on supersession, and by daemon crash recovery (REQ-TEST-09) | PASS | `test_postgres_removed_on_supersession_and_recovery` |

## Phase 3 addendum (2026-08-23)

Root integration suite `tests/integration/test_deployments.py` against real
systemd units and Docker containers:

| Item | Result | Evidence |
|---|---|---|
| Heterogeneous deployment (HTTP process + worker + dedicated PostgreSQL + Docker cache) applies, runs, and serves with injected ports/DB URL (REQ-DEPLOY-01) | PASS | `test_worktree_apply_stop_start_reapply_remove` |
| Route document published atomically with checksum; withdrawn on stop; moved on reapply (REQ-DEPLOY-05) | PASS | same + `test_ports_routes.py` |
| Stop/start preserves the PostgreSQL container and its data; component-level restart (REQ-DEPLOY-04) | PASS | same |
| Live-worktree reapply → new generation on a new port, old unit retired, stable DB kept | PASS | same |
| Checkout source: immutable generations, worktree edits cannot affect it, rollback to previous, only two generation dirs retained | PASS | `test_checkout_generations_and_rollback` |
| Failing component → `deployment_apply_failed` with exact component states, nothing routed, honest degraded/failed state (REQ-DEPLOY-03) | PASS | `test_failed_component_is_degraded_and_busy_is_immediate` |
| Concurrent mutation → immediate `busy`, never queued (REQ-DEPLOY-02) | PASS | same |
| Inventory classifies managed containers by labels + recorded bindings; others unmanaged (REQ-HEALTH-02) | PASS | same |
| Remove keeps data unless `delete_data`; volumes deleted only when asked | PASS | both |

Bug found and fixed by the integration run: `docker --env-file` keeps quotes
literally, so the systemd-style env file put `"app"` (with quotes) into the
PostgreSQL container; Docker/Compose env files now use the literal format.

## Phase 4 addendum (2026-08-23)

| Item | Result | Evidence |
|---|---|---|
| 15 s cgroup sampling of real deployment components and containers; per-repository attribution; reconciliation managed + DevCoordinator + other = host (REQ-HEALTH-03) | PASS | integration `test_health_views_measure_real_workloads` |
| Storage buckets per repository (checkout, scratch, artifacts, layers, volumes, PostgreSQL data) and host reconciliation incl. Docker shared images/cache | PASS | same |
| Dedicated PostgreSQL operational facts (connections, WAL, temp, size) — numbers only | PASS | same |
| One-minute aggregates persisted and queryable; 30-day expiry (REQ-HEALTH-04) | PASS | same + `test_metrics.py` |
| Sustained-threshold alerts: window, dedupe, single recovery, persistence across restart, vanished-subject recovery | PASS | `test_metrics.py::test_alert_sustain_dedupe_and_recovery` |
