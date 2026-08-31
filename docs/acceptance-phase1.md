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

## Schema 10 Codex usage addendum (2026-08-29)

| Item | Result | Evidence |
|---|---|---|
| Private source policy validates explicit same-owner UIDs, Codex homes, and executables while thin clients never load it | PASS | `tests/test_paths.py`, `tests/test_install.py` |
| Repository mapping uses a fixed JSON command under the source UID and persists only opaque source/repository links | PASS | `tests/test_codex_usage.py`; schema-10 database and installer tests |
| Schema-4/taxonomy-1 databases are queried read-only; merged identities, provider categories, UTC buckets, interval unions, tool outcomes, and unsupported sources remain truthful | PASS | `tests/test_codex_usage.py`; live read-only DevCoordinator2 repository smoke returned 24 points and measured activity |
| Administrators and repository operators receive combined results; viewers and ungranted repositories are denied before private data is read | PASS | `tests/test_access.py`, `tests/test_usage_api.py` |
| Selected trend-first Console design works across populated, dense 30-day, empty, loading, error, denied, desktop, mobile, and breakpoint states | PASS | `console/verify.mjs`: 652 checks, 0 failures, including large-scale SVG label geometry; formal verifier: 18/18 cells, 0 criticals, manual review passed; `design-qa.md` passed |
| Complete source acceptance | PASS | `ruff check src scripts tests`; full pytest suite; `node --check` for Console sources; instance-data scan clean |
| Live installation preserves a dated root-only database/config backup and prior releases; schema 10, private two-source policy, database integrity, authenticated real-data chart, linked navigation, 7-day focus restoration, privacy scan, and clean browser console all pass | PASS | repository installer and external browser acceptance on 2026-08-30; final daemon restart: 42 MiB peak, six tasks, no automatic Codex child process |

## Shared Console navigation addendum (2026-08-30)

| Item | Result | Evidence |
|---|---|---|
| Every destination title is a real collection link in populated, empty, loading, error, denied, and applying states | PASS | `console/verify.mjs`: all 15 routes at 1280×800 and 390×844 |
| Plan, Progress, Decisions, and Codex Usage details use keyboard-operable custom DOM project menus with real same-destination links and no native select | PASS | mouse, Arrow-key, Escape/focus-return, route-change, and visible-project interaction proofs in `console/verify.mjs` |
| Global header stays on one row and swaps the original navigation links into a hamburger before wrapping | PASS | `console/verify.mjs`: 799×964 interaction plus current 1240/1241 px boundary; complete run 1,397 checks, 0 failures |
| Rendered hierarchy, contrast, theme, responsive geometry, scroll regions, headline wrapping, and final visual review | PASS | formal verifier: 29/29 cells checked, 0 criticals; 58 final images reviewed; 29 pass decisions finalized in manual-review manifest |
| Live authenticated release matches the verified source and preserves real navigation behavior | PASS | `nav-20260830T094006Z-5793a92`: both services active on schema 10; source/release hashes match; 1440 header 54 px and 799 header 55 px with zero overflow; 11-project Usage menu, Plan/Decisions menus, all eight destination links, switching, keyboard focus, and Escape passed with no browser errors; verified backup at `/var/backups/devcoordinator2/20260830T093742Z-pre-console-navigation` |

## Health layout addendum (2026-08-30)

| Item | Result | Evidence |
|---|---|---|
| Host capacity and operational status lead as aligned groups; incident cards keep their natural height | PASS | Health-specific rendered checks at 390×844, the user-marked 856×915, 959/960/961×900, and 1440×1024; current focused interaction run: 138 checks, 0 failures |
| Shared storage labels and values remain complete without cropped text or horizontal discovery; repository rows become labelled cards at 960 px and below | PASS | exact long-content fixture with all five shared-storage categories; formal verifier checked 9/9 cells including 960 and 1200 breakpoint profiles, 0 critical findings |
| Health ranges, unhealthy start/stop/restart, container navigation, deployment details, permission failure, load failure, and Retry work through the rendered interface | PASS | `console/verify.mjs` Health interaction inventory; 24h/7d/30d reads and all destination/action assertions pass |
| Final visual review at mobile, marked, desktop, and both responsive boundaries | PASS | 18 current-source formal viewport/full-page images reviewed; 9 pass decisions finalized in `/tmp/formal-web-ui-verification-6BqOQb/manual-review.json` |

## Codex Usage completeness wording addendum (2026-08-31)

| Item | Result | Evidence |
|---|---|---|
| Missing input data leads with “Some usage may be missing,” states how many configured Codex environments supplied data, defines an environment as a separate local Codex setup with its own usage history, and explains that absent values are excluded rather than counted as zero | PASS | focused partial-state interaction assertions and rendered 390×844 / 856×915 / 1440×900 evidence |
| Complete, no-measurement, and unavailable states use the same plain-language model; repository and exact-bucket tables say “Data included” / “Data status” | PASS | targeted fixture state transitions in `console/verify.mjs`; no visible `collector`, `Partial coverage`, `Complete coverage`, or `measured values only` copy remains |
| Complete current Console regression cycle | PASS | `console/verify.mjs`: 1,397 checks, 0 failures; full pytest suite passed; Console/edge syntax and diff checks passed |
| Formal hierarchy, contrast, scroll, clipping, and manual visual review | PASS | 9/9 cells, 0 critical findings; all 18 final viewport/full-page images reviewed; 9 pass decisions in `/tmp/dc2-usage-environment-formal-manual-review.json` |

## Repository Progress dashboard addendum (2026-08-30)

| Item | Result | Evidence |
|---|---|---|
| Hourly, daily, and Monday-aligned weekly buckets combine permanent task events, current planned-line estimates, bounded terminal test summaries, and provider total tokens without converting missing evidence to zero | PASS | `tests/test_progress_api.py`, `tests/test_test_history.py`, `tests/test_codex_usage.py`; focused 25-test pass |
| Real terminal test completion enters symlink-safe repository-local history and remains outside the authority database | PASS | root systemd integration `test_pass_uid_and_bounded_output`; secure filesystem and malformed-history must-catches |
| Release forecasts expose range, confidence, evidence, assumptions, unknown estimates, missing target date, zero-pace/no-release states, and truthful non-mutating priority scenarios | PASS | deterministic populated, no-release, no-history, and insufficient-pace contract tests; rendered exact-counting disclosure |
| Both owner-selected Product Design directions work as Delivery pulse and Priorities modes with repository, period, task, scenario, exact-value, and exact Plan-continuation interactions | PASS | `console/verify.mjs`: complete 1,397-check Console matrix, 0 failures; focused interaction inventory 147/147 |
| Selected visual target matches at desktop and remains usable at the reported 799×964 and narrow 390×844 sizes | PASS | `design-qa.md`: passed after one P2 density/hierarchy iteration; formal verifier 9/9 cells, 0 critical findings, all 18 images reviewed and 9 pass decisions finalized in `/tmp/dc2-progress-formal-final3/manual-review.json` |
| Complete source and relevant host-level validation | PASS | full pytest suite; ruff; JavaScript syntax; diff check; root test-history integration |

## Combined current-source release gate (2026-08-31)

| Item | Result | Evidence |
|---|---|---|
| Every Console route, state, and enabled interaction remains intact after combining lifecycle, Plan, Progress, Usage, navigation, and Health work | PASS | `console/verify.mjs`: 1,397 checks, 0 failures in `/tmp/devcoordinator2-release-console-final/report.json` |
| Plan task actions stay visible across the shared responsive transition and Progress no longer widens the page at 801 px | PASS | formal samples at Plan 1049/1050/1051, 1179/1180/1181, 1239/1240/1241 and Progress 799/800/801; zero critical findings |
| Current-source hierarchy, contrast, clipping, scroll topology, responsive geometry, and final visual evidence pass across the four changed primary destinations | PASS | formal verifier: 50/50 cells, 0 critical findings; all 100 viewport/full-page images reviewed; 50 pass decisions finalized in `/tmp/formal-web-ui-verification-xjNar4/manual-review.json` |
| Complete source and real-system acceptance passes before installation | PASS | ruff; 230 unit tests; public-edge suite; 18 root systemd/Docker/PostgreSQL/Compose integrations; instance-data scan; syntax and diff checks |
