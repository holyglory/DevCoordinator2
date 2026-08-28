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

## Phase 5 addendum (2026-08-23)

| Item | Result | Evidence |
|---|---|---|
| Invited identity signs in (real OIDC flow vs fixture issuer), is admitted via the daemon, reaches only granted routes (REQ-ACCESS-01) | PASS | `edge/test/edge.test.mjs` |
| Roles access/viewer/operator/administrator enforced per request; filtered list/health for non-admins (REQ-ACCESS-02) | PASS | `tests/test_access.py`, `tests/integration/test_access_edge.py` |
| Revocation effective on the next edge request and API call (REQ-ACCESS-03) | PASS | both |
| Only the configured edge uid may assert an identity; spoofing from another uid is refused (REQ-ACCESS-04) | PASS | `test_access_edge.py` |
| Edge keeps serving the last valid route document; malformed/tampered/stale documents rejected (REQ-REL-02) | PASS | `edge.test.mjs` |
| Session cookie never reaches upstreams; verified identity forwarded only on authenticated routes | PASS | `edge.test.mjs` |

## Phase 6 addendum (2026-08-23)

| Item | Result | Evidence |
|---|---|---|
| /start → link code → link to identity → scoped subscriptions; routing of deployment/test/alert/container/bug events; successful tests silent | PASS | `tests/test_telegram_bugs.py` (fake Telegram API) |
| Bounded outbox: retry with backoff, attempt cap, row cap, age cap | PASS | same |
| Bot token never appears in stored or returned data | PASS | same |
| Bug intake works with the daemon unavailable; recurrence counting; secrets/raw-log/private-path rejection; close removes the record (REQ-REL-03) | PASS | `test_clients.py::test_bug_report_works_without_daemon`, `test_bug_registry_independent_store` |

## Phase 7 addendum (2026-08-23)

| Item | Result | Evidence |
|---|---|---|
| Deployments, Tests, Health (+Containers), Bugs, Administration destinations rendered at wide and narrow viewports in populated / empty / error / loading / permission-denied states; observed-only deployments expose no lifecycle/log controls; no overflow, clipping, or off-canvas controls | PASS | `console/verify.mjs`: 279 checks, 0 failures |
| Every control calls the real API and re-reads state (stop→stopped, start→running, logs, remove with explicit data choice, test output, bug report, invite, container removal) | PASS | interaction proofs in the same run |
| Non-administrators get explicit permission-denied notices; host health hidden, repositories filtered | PASS | `denied` scenario |

## Phase 8 addendum (2026-08-23)

| Item | Result | Evidence |
|---|---|---|
| Read-only legacy export (repositories, ports, server definitions, routes, owners, pending requests, Telegram config without token, open bugs) | DONE | `scripts/legacy_export.py` → `instance/legacy-export.json` (untracked) |
| Import tool: administrators, Telegram chats/subscriptions, bugs, and exact running container/Compose identities as a replaceable observed-only projection; temporary/historical exclusion; reviewed current routes; fixture cleanup; dry-run | PASS | `tests/test_legacy_import.py`, `tests/test_observed.py`; private dry-run and installed import evidence under `instance/` / the recovery root |
| Canary installed beside legacy on private socket/port; daemon and edge active; route document served (generation ≥ 1, bootstrap owner) | PASS | `scripts/install.py --canary` on the host |
| Installed CLI journey as a client account: test start → passed as caller uid, output, list; MCP `test_status`; bug report/close; health summary/containers | PASS | run on the host through `/usr/local/bin/devcoordinator2` |
| Bugs found by the installed run and fixed: daemon unit `PrivateTmp` hid caller filesystems; test parser rejected files that also declare deployments; edge main-guard failed through the release symlink; installer ownership (uid used as gid) | FIXED | this commit |

## Schema 9 lifecycle addendum (2026-08-28)

| Item | Result | Evidence |
|---|---|---|
| Digest-pinned PostgreSQL-compatible fixture pulls/verifies the exact PostGIS image, injects private generated PG credentials, executes a real extension query, and removes the container | PASS | root integration `test_digest_pinned_postgis_fixture_is_pulled_injected_and_removed`; validator/Docker unit must-catches |
| Native Compose uses multiple reviewed files plus an instance-authorized ignored interpolation file; missing/malformed/writable/wrong-repository/path/symlink/committed-file authority fails closed | PASS | path, installer, engine unit tests; root native-Compose integration through the private allowlist |
| Finite bootstrap exits 0 with a generation receipt; unchanged apply and ordinary stop/start do not rerun it; changed apply reruns exactly once | PASS | root integration `test_native_compose_finite_service_receipt_and_start_semantics` |
| Reviewed independent worker stop/start affects exact service containers only, preserves other services, route, volume state, and bootstrap receipt, reports degraded while stopped, and restores running | PASS | same root integration; CLI/MCP shared control contract; Console interaction proof |
| Endpoint readiness preserves recoverable systemd restarts but aborts a terminal binding before a long health deadline | PASS | root integrations `test_failed_component_is_degraded_and_busy_is_immediate` and `test_readiness_allows_process_to_recover_within_restart_policy` |
| Applying-state controls and independent Compose-service controls work truthfully across required Console states and representative wide/narrow constraints | PASS | `console/verify.mjs`: 530 checks, 0 failures; formal verifier: populated + applying at 390×844 and 1440×900, 4 checked pages, 0 criticals/warnings, coverage passed (`/tmp/devcoordinator2-formal-*-report.{json,md}`) |
| Complete source and real-service acceptance after batch fixes | PASS | `ruff check src tests scripts`; full unit suite; complete root integration suite; instance-data scan clean |
| Edge switch / legacy fence / Docker authoritative mode / live import | OWNER-EXECUTED | `docs/cutover.md`, `scripts/edge_switch.py` |
