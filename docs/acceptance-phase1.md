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
