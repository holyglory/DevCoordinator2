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
| Every control calls the real API and re-reads state (stop→stopped, start→running, deployment logs, remove with explicit data choice, progressive test-log retrieval, bug report, invite, container removal) | PASS | interaction proofs in the same run; current test-log proof is superseded by the Rust addendum below |
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
| Missing input data leads with “Some usage may be missing” and states how many configured Codex environments supplied data; the definition and excluded-not-zero explanation stay hidden until the adjacent labelled information hint is opened | PASS | focused partial-state interaction assertions, including pointer/keyboard opening and Escape/outside-click dismissal, plus rendered 390×844 / 856×915 / 1440×900 evidence |
| Complete, no-measurement, and unavailable states use the same plain-language model; repository and exact-bucket tables say “Data included” / “Data status” | PASS | targeted fixture state transitions in `console/verify.mjs`; no visible `collector`, `Partial coverage`, `Complete coverage`, or `measured values only` copy remains |
| Complete current Console regression cycle | PASS | `console/verify.mjs`: 1,409 checks, 0 failures; focused interaction inventory: 155 checks, 0 failures; Console JavaScript syntax and diff checks passed |
| Formal hierarchy, contrast, scroll, clipping, and manual visual review | PASS | closed missing/complete/unavailable states plus the opened hint at 390×844, 858×915, and 1440×900; 12/12 cells, 0 critical findings; all 24 final viewport/full-page images reviewed; 12 pass decisions in `/tmp/dc2-usage-hint-formal-manual-review.json` |
| The compact completeness hint is live with a recoverable rollback path | PASS | immutable release `usage-hint-20260831T233035Z-f974440`; root-only integrity-checked backup `/var/backups/devcoordinator2/20260831T233035Z-pre-usage-hint`; installed app, stylesheet, and icon hashes match the verified source; schema 11 and public edge generation 128 remain healthy; the atomic static release switch required no service restart |

## Codex Usage one-second collection addendum (2026-09-01)

| Item | Result | Evidence |
|---|---|---|
| The 11-row default collection no longer multiplies per-repository query deadlines and returns measured rows inside one second | PASS | live baseline 12,653 ms with six ~2,000 ms rows; final installed cold daemon read 590.0 ms; ten prior repeated installed reads max 580.1 ms; production source has 5 token-bearing rows instead of 11 timeout-derived unavailable rows |
| Every range responds inside one second without converting unfinished reads to unavailable or zero | PASS | installed 24h 590.0 ms with data; 7d 820.7 ms and 30d 810.9 ms with neutral `indexing` coverage and dashes, then 6 measured rows resolved at 9.6 s / 11.4 s through one ephemeral background reader; no usage aggregates persisted |
| Setup, partial, complete, no-measurement, updating, and real failure states remain semantically distinct and responsive | PASS | 155/155 focused interactions; `indexing` auto-refresh and focus preservation; 1,409/1,409 complete Console checks; formal verifier 3/3 at 390×844, marked 858×915, and 1440×900 with zero critical findings; all 6 images passed `/tmp/dc2-usage-fast-formal-final-manual-review.json` |
| The final behavior is live with an exact rollback point | PASS | immutable release `usage-fast-final-20260901T101358Z-f974440`; root-only integrity-checked backup `/var/backups/devcoordinator2/20260901T101358Z-pre-usage-indexing-final`; schema 11 and edge generation 151 healthy; installed hashes match source; authenticated cache-expired 858×915 navigation settled 11 rows in 635.4 ms with 5 amber partial, 5 neutral setup, 0 red failure, no fake zeroes/overflow/browser/network errors |

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

## Factual Progress redesign addendum (2026-08-31)

| Item | Result | Evidence |
|---|---|---|
| Open release work follows the same depth-first sibling order as Plan; the compact list shows recorded title, status, estimate, elaboration, and a simple reopened state without raw planning/event notes | PASS | `tests/test_progress_api.py`: 5 focused tests; nested order, unknown size, reopening evidence, no outcome/ranking contract; browser fixtures carry technical raw-note markers and assert neither renders |
| Task and planned-line charts use discrete bucket bars plus unfilled running-total lines; missing test/token evidence renders as an explained blank rather than zero | PASS | reference-matched `progressReference` browser fixture; bar/line DOM assertions; exact-value disclosure; no evidence chart when every value is missing |
| Release-work selection is local and exact Plan continuation remains truthful | PASS | workspace DOM identity and API-call assertions; all rendered rows selected; `Open selected in plan` highlights the exact task |
| The owner-selected first Product Design direction matches at desktop and remains usable across narrow and every changed breakpoint | PASS | `design-qa.md`: passed after two P2 iterations; formal run `formal-web-ui-mthgf4b5`: 14/14 cells, 0 critical findings; 28 viewport/full-page images reviewed and 14 passes finalized in `/tmp/dc2-progress-formal-redesign-final2/manual-review.json` |
| Complete Console and source regression remains green | PASS | `console/verify.mjs`: 1,397 checks, 0 failures in `/tmp/dc2-progress-redesign-final2/report.json`; full pytest suite passed; Ruff passed; edge tests 3/3 passed; JavaScript syntax passed |
| Complete source and real-system acceptance passes before installation | PASS | ruff; 230 unit tests; public-edge suite; 20 root systemd/Docker/PostgreSQL/Compose integrations; instance-data scan; syntax and diff checks |
| The corrected Progress redesign is live with a recoverable rollback path | PASS | release `progress-redesign-notes-20260831T232128Z-f974440`; root-only backup `/var/backups/devcoordinator2/20260831T230259Z-pre-progress-redesign`; both services active; source/installed hashes match; schema 11 and database quick check pass; public edge generation 128 is live; authenticated wide/narrow browser acceptance proves `release_work`, 10 bars, 2 running-total lines, 5 Plan-ordered rows, zero raw-note leakage, local selection, exact values, exact Plan continuation, zero document overflow, and no browser/network/server errors |
| Live-acceptance raw-note correction preserves the complete Console | PASS | marker-backed `console/verify.mjs`: 1,404 checks, 0 failures in `/tmp/dc2-progress-redesign-note-fix-final/report.json`; focused interaction inventory 152/152; updated `design-qa.md` passes |
| Daily values remain readable where the running-total line crosses them | PASS | live release `usage-fast-20260901T095846Z-f974440` already contains the isolated label-order/halo fix; exact-release Console matrix 1,406/1,406 and interaction inventory 152/152; formal run `formal-web-ui-mtihjr1d` checks 390×844, reported 858×915, and 1440×1024 with zero critical findings and three reviewed passes; fresh authenticated live 858×915 browser proves both lines paint before all labels, 10 labels are visible with 3 px halos, zero document overflow, and zero console errors. The existing open-tab cache issue remains separately tracked by `p8d93ca028f4d10bb`. |

## Governed parallel checks and upgrade drain addendum (2026-09-01)

| Item | Result | Evidence |
|---|---|---|
| Historical unbounded admission; completion-only and success-required dependencies remain distinct | SUPERSEDED | The dependency proof remains historical evidence. Host admission is now owned by the adaptive Rust scheduler under REQ-TEST-16; repositories still cannot encode a competing budget. |
| Historical Python process/event execution contract | SUPERSEDED | Current acceptance comes from the Rust executor's strict schema-2 process-group, event-identity, deadline, cleanup, and long-lived-service tests; Python runner reports are not accepted. |
| Historical Python per-check evidence and diagnostic contract | SUPERSEDED | Current acceptance comes from Rust executor artifact/source binding, bounded output, selected/retry proof, case expansion, and final complete release tests. |
| Normal source-owned upgrades close admission, let active tests and cleanup finish, switch an immutable release, reconnect, and recover from abort or parent failure without changing unexpected-restart semantics | PASS | admission/lease/activation-guard unit cases; real `test_repository_installer_drain_waits_then_switches_and_reconnects`; cancellation-reason and daemon-interruption journeys; REQ-TEST-08 remains green |
| Elapsed time never means success | CURRENT CONTRACT | Rust leaf deadlines produce `timed_out`; process exit or the exact completion event remains the only success signal. Current proof is the strict schema-2 executor and root integration suite. |
| Historical complete-source acceptance | SUPERSEDED | The listed Python-runner release predates the Rust cutover and is not current readiness evidence. |

## Repository deployments dashboard addendum (2026-09-01)

| Item | Result | Evidence |
|---|---|---|
| Every repository owns one complete summary of its Plan, Progress, 24-hour Codex Usage, Tests, Health, latest Decision, deployments, and actions; no value or deployment crosses repository boundaries | PASS | deliberately divergent `legacy-repo` and `repo-one` fixtures; strict section/link/deployment assertions in `console/verify.mjs`; complete 1,493-check Console matrix with zero failures |
| Plan, Progress, Codex Usage, and Decisions continue to the exact repository route; Tests and Health retain visible repository attribution; restricted evidence has no enabled dead link | PASS | all six repository-summary journeys invoked through the rendered interface; populated and permission-limited fixture assertions |
| The approved repository-centred design remains readable from desktop through tablet and several mobile widths without clipping, overlap, off-canvas controls, nested horizontal scrolling, or document overflow | PASS | focused browser pass 229/229 at 320, 390, 430, 619/620/621, 834, 959/960/961, 1179/1180/1181, 1239/1240/1241, and 1440 px; formal run `formal-web-ui-mtit1v0d`: 16/16 cells, zero critical findings, 32 reviewed images, finalized zero-gap manifest |
| Existing deployment domain and lifecycle behavior remains truthful across managed, observed, applying, empty, loading, error, and permission-limited states | PASS | domain edit plus start/stop/restart/apply interaction evidence; applying controls disabled; full Console matrix and focused interaction inventory pass |
| Source and visual quality gates pass on the final implementation | PASS | 273 Python tests passed with 25 environment skips; Ruff passed; edge 3/3 passed; JavaScript syntax and diff checks passed; `design-qa.md` final result passed |

## Rust execution and adaptive validation cutover addendum (2026-09-02)

| Item | Result | Evidence |
|---|---|---|
| One reusable Rust execution plane owns graph scheduling, process groups, exact completion and diagnostic events, per-leaf deadlines, byte-complete per-check/case streams, source/artifact receipts, and one-level case expansion without an aggregate copy | PASS | locked release build; 102 Rust workspace tests covering strict protocol, fan-out, inherited descriptors, events, deadlines, cancellation, complete output, structured evidence, catalogue/query/retention, and cleanup; Python-to-Rust golden plan and real executor-produced-store E2E |
| Every governed leaf enters one fair host-wide adaptive broker that starts at twice the logical CPU count, learns only after a long registered run, pauses under sustained CPU or memory pressure, survives dependency-wave gaps, and honors one immediate administrator cap | PASS | deterministic increase/decrease/neutral/pause/recovery/cap/restart tests; real Rust-executor-to-production-broker round-robin, queued/active crash-disconnect, and no-leak integration; live Auto state 64 with zero waiting/active permits; live set/clear round trip restored Auto |
| Validation tiers and invalidating preflights are strict schema-2 behavior, with no schema-1 or single-command compatibility | PASS | parser/protocol rejection cases; all five active repository configurations migrated to graph-only schema 2; installer preflight accepted 49 tests and seven deployments; development, pre-merge, release, invalidated, timed-out, and readiness boundaries covered |
| Complete logs stay cold while diagnostics are progressively disclosed | PASS | a real >5 MiB systemd run retains exact bytes, SHA-256, line count, and final sentinel; catalog, tail, literal search, exact range, and failure-context work per logical stream; 24-hour/three-history age-or-depth retention, active locks, garbage recovery, cursor snapshots, structured JUnit/Playwright/Rust/event evidence, injection/path/tamper guards, and no-aggregate/no-schema-1 assertions pass |
| The six-skill validation matrix uses the Rust scheduler instead of a Python subprocess harness | PASS | one 29-leaf release plan; 16 independent cheap preflights invalidate 13 expensive checks; 900-second leaf ceilings, bounded receipt, complete cold per-leaf logs, all-settled proof, and complete 29/29 current-source pass in `.devcoordinator/agent-validation/skills-20260902T131601Z-2160675-bada71/check-report.json` |
| Shared skill helpers have one canonical source and authorized administration no longer adds redundant confirmation | PASS | three vendored harness trees, synchronization and standalone-package tooling removed; 36 skill links and six policy links resolve directly to the live checkout; operation registry, role-denial, MCP annotation, append-only decision summary, and 1,521-check Console interaction matrix all pass |
| Tests, logs, retention, and Capacity remain usable and truthful at wide and narrow sizes | PASS | complete Console matrix: 1,521 checks, zero failures; formal log/retention run: 8/8 journey cells, zero critical findings, three reviewed intentional narrow scroll warnings; all eight current image pairs finalized with pass decisions; the lower narrow log action is reachable through the measured native dialog scroll |
| The verified release is active with a recoverable rollback point | PASS | clean `main` exactly matching `origin/main`; integrity-checked backup `/var/backups/devcoordinator2/20260902T010213Z-pre-rust-executor`; both services active; authority database quick-check `ok` and live schema 13; existing repository development typecheck passed through Rust with broker capacity 64 and unchanged source |
