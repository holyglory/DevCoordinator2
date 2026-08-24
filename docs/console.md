# Console (Phase 7)

Static browser application in `console/` (no build step, no dependencies),
served by the edge on the console host to signed-in users, driven only by
the edge's `/api/<command>` bridge. Every visible enabled control calls the
real API and re-reads state afterwards; nothing is a placeholder, nothing
fakes success, and no view carries fixture numbers.

## Destinations

1. **Deployments** — collection first, grouped under repository headers
   (display name + repository id) so `web@worktree` is always attributed
   (state, domain, port, generation, updated); a ✎ button on every row's
   domain opens the pop-up domain editor in place;
   start/stop/restart for operators on every deployment —
   observed ones drive the exact recorded containers
   (DC2-2026-08-24-OBSERVED-LIFECYCLE) — apply for administrators on
   managed ones; detail page with components (state, health, generation,
   port, restarts, exact binding, last error), per-component controls,
   on-demand logs (managed files or observed `docker logs`),
   rollback/remove (managed, administrators; remove asks explicitly whether
   persistent data should be deleted), the same pop-up domain editor
   (administrators; `deployment.set_domain` — for an observed deployment
   without a route it asks for the host port, and a re-import replaces
   observed edits), and per-component CPU/memory charts over a selectable
   1h/24h/7d/30d window. The detail page names the repository under the
   heading.
2. **Tests** — one current/most-recent run per worktree: result and
   duration first; stdout/stderr tails load only on demand (bounded); stop
   a running test or start the declared default (administrators).
3. **Health** — host condition first as tiles with capacity meters (CPU,
   memory, filesystem, load/swap, unhealthy count, active tests, container
   counts, critical alerts); then **Unhealthy deployments** as cards naming
   exactly which component is unhealthy and why (`reasons` from
   `health.summary`) with start/stop/restart and a link to details and
   logs; current alerts; **History** — host CPU, memory, and storage charts
   (min–max band plus average) over a selectable 24h/7d/30d window using
   server-side downsampling; the reconciliation line; then one row per
   repository with CPU/memory/storage/health/trends plus the DevCoordinator
   and shared/unattributed rows. **Containers** view: every
   container with full identity, state, classification, repository,
   deployment/test, caller and client, CPU/memory/layer size, creation
   time, TTL; removal is offered only for orphaned-managed and managed-test
   containers (unmanaged ones say "decide manually").
4. **Bugs** — open records with occurrence counts and correlations; report
   form; close.
5. **Administration** (administrators only) — users and grants, invitations
   (invite form with optional initial grant), Telegram chats and
   subscriptions (link code, subscribe), server versions and the served
   route-document generation.

Non-administrators see only the destinations and data their grants allow;
server-wide health and the Containers/Tests/Administration views render an
explicit permission-denied notice instead of partial data.

## Interaction inventory (all verified by `console/verify.mjs`)

| Control | API call | Proof of state change |
|---|---|---|
| Deployment start/stop/restart (list, detail, component; managed and observed) | `deployment.start/stop/restart` | view re-fetches `deployment.status`; header/component badges change |
| Domain edit / clear (pop-up from list rows and the detail page, administrators) | `deployment.set_domain {deployment_id, domain|null, port?, public?}` | status re-read; route document republished |
| Health range switch (24h/7d/30d) and usage range (1h/24h/7d/30d) | `health.history {minutes, points}` | charts re-render from the store |
| Unhealthy-deployment actions (health page cards) | `deployment.start/stop/restart` | summary re-read |
| Apply / rollback | `deployment.apply` / `deployment.rollback` | status re-read |
| Remove (confirm + explicit delete-data choice) | `deployment.remove {delete_data}` | list re-read |
| Component logs | `deployment.logs` | tail rendered on demand |
| Test stdout/stderr | `test.output {tail_bytes: 16384}` | bounded tail rendered |
| Test start/stop | `test.start` / `test.stop` | list re-read |
| Container remove (orphaned/test only) | `health.container_remove {container_id}` | inventory re-read |
| Bug report / close | `bug.report` / `bug.close` | list re-read |
| Invite, remove user, set/remove grant | `user.invite`, `user.remove`, `grant.set`, `grant.remove` | administration re-read |
| Telegram link / subscribe / unsubscribe | `telegram.link`, `telegram.subscribe`, `telegram.unsubscribe` | administration re-read |
| Sign out | `/auth/logout` | session cleared |

## Browser verification

`CONSOLE_VERIFY_PLAYWRIGHT=<dir with node_modules/playwright> node
console/verify.mjs` runs the real edge and Console against a fake daemon
with fixture scenarios — populated (long names, large numbers, degraded,
alerts), empty, error, loading, permission-denied — at 1280×800 and
390×844 for every destination, checking: no horizontal document overflow,
no clipped headline text, no off-canvas controls outside scroll containers,
explicit empty/error/loading/denied states, humanized large numbers; and
clicks through stop/start/logs/remove/test output/bug report/invite/
container removal proving each calls the API with the expected arguments
and re-renders. Screenshots and `report.json` are written to
`CONSOLE_VERIFY_OUT` (not committed). Last run: 279 checks, 0 failures.
