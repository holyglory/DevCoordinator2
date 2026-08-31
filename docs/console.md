# Console (Phase 7)

Static browser application in `console/` (no build step, no dependencies),
served by the edge on the console host to signed-in users, driven only by
the edge's `/api/<command>` bridge. Every visible enabled control calls the
real API and re-reads state afterwards; nothing is a placeholder, nothing
fakes success, and no view carries fixture numbers.

## Navigation shell

The header is one non-wrapping row. Its destinations are ordinary hash links
at wide widths and the same links move into a custom DOM hamburger menu at
1240 px and below. The menu exposes expanded state, closes on outside click,
link activation, or Escape, and returns focus to its button after keyboard
dismissal. Every page's destination title links back to its collection. Plan,
Progress, Decisions, and Codex Usage repository details place the current project beside
a compact custom project menu whose entries are real same-destination links;
Arrow keys, Home/End, Escape, outside click, and focus restoration work without
a native select.

## Destinations

1. **Deployments** — collection first, grouped under repository headers
   (display name + repository id) so `web@worktree` is always attributed
   (state, domain, port, generation, updated); a ✎ button on every row's
   domain opens the pop-up domain editor in place;
   start/stop/restart for operators on every deployment —
   observed ones drive the exact recorded containers
   (DC2-2026-08-24-OBSERVED-LIFECYCLE) — apply for administrators on
   managed ones; detail page with components (state, health, generation,
   port, restarts, exact binding, last error), per-component controls and
   nested Compose-service state; only explicitly independent long-running
   services receive exact start/stop/restart controls,
   on-demand logs (managed files or observed `docker logs`),
   rollback/remove (managed, administrators; remove asks explicitly whether
   persistent data should be deleted), the same pop-up domain editor
   (administrators; `deployment.set_domain` — for an observed deployment
   without a route it asks for the host port, and a re-import replaces
   observed edits), and per-component CPU/memory charts over a selectable
   1h/24h/7d/30d window. The detail page names the repository under the
   heading. While a deployment is applying, conflicting mutations are disabled
   and a notice explains that closing the page does not cancel the accepted
   operation; the caller refreshes status after it finishes.
2. **Plan** — the completion ledger as a per-repository interactive Gantt workspace
   (picker first: every visible repository with its current release,
   done-lines progress, open-task count, and a preview-requested badge).
   A sticky, collapsible and resizable task navigator keeps the arbitrary-depth
   tree aligned with a dominant, independently scrollable cumulative-lines
   canvas. The canvas supports pointer and keyboard pan, zoom, fit, a draggable
   minimap, animated hover details, synchronized task selection, direct
   drag-and-drop, and truthful right-edge estimate resizing with an exact-number
   dialog as the keyboard/touch path. A distinct **Not estimated** band gives
   every unknown-size leaf and parent summary a selectable non-proportional
   mark without adding invented lines to the numeric axis; its selected tray
   offers **Add estimate**. Parent tasks are summary spans; releases
   group in sequence with boundaries and per-release progress; delivered releases link to the running app
   ("Open the app ↗") or name the server port. Everything is plain
   language: statuses read planned / being built / done / delivered, owner
   feedback carries a "your request" badge, long labels reveal their complete
   text through selection/hover, and the agent-facing `technical_note` is never
   rendered. A compact bottom tray keeps the selected task, progress, impact,
   release, move, resize and drop actions in context; narrow screens use a
   compact summary and focused bottom sheet. Selection, task disclosure,
   navigator disclosure, and tray disclosure update the existing workspace
   locally, preserving scroll/focus without another plan read or whole-page
   replacement. Owner controls (administrators):
   drag-and-drop a task between releases or to a new position within one
   (drop between rows or onto a release header; delivered releases refuse
   drops), the move pop-up as the touch/accessibility path, "Request
   preview now" (replaced by a pending notice while one is requested),
   drop-task with confirm, and an "Ask for a change" form that files a
   `user_feedback` task. Viewers get the same chart read-only.
3. **Progress** — an operator/administrator repository dashboard with two
   complementary modes selected by the owner. **Delivery pulse** aligns
   terminal task completions, current planned task lines completed, terminal
   test pass rate, and provider total-token use on hourly, daily, or
   Monday-aligned weekly UTC buckets. **Priorities** ranks owner-facing open
   outcomes by recorded status, estimate uncertainty, request kind, and
   contribution to remaining work, then compares the current forecast with
   moving the selected work first or moving it to a later release. Selection
   and scenarios are local and never mutate Plan order or scope; **Open selected
   in plan** continues to the exact task. The release forecast is a date range
   with confidence, evidence, and assumptions—not a promise—and becomes
   unavailable when there is no release or measurable completion pace. Missing
   test/token history stays blank and visibly partial. Exact bucket values and
   counting semantics remain available without hover.
4. **Decisions** — the per-repository decision history in plain language:
   "The story so far" (latest rolling summary), a full-text search box over
   every decision ever recorded, an aspect filter (server-side), entries
   newest-first with aspect badge, optional stable ref, and age; superseded
   decisions collapse and dim; "Show older decisions" pages the permanent
   history.
5. **Tests** — one current/most-recent run per worktree: result and
   duration first; stdout/stderr tails load only on demand (bounded); stop
   a running test or start the declared default (administrators).
6. **Health** — host condition first as one aligned capacity group (CPU,
   memory, root filesystem, load/swap) beside a separate operational-status
   group (unhealthy deployments, critical alerts, active tests, total
   containers, and class counts). At 1200 px and below the two groups stack;
   incident cards keep their natural height. **Unhealthy deployments** names
   exactly which component is unhealthy and why (`reasons` from
   `health.summary`) with start/stop/restart and a link to details and
   logs; current alerts; **History** — host CPU, memory, and storage charts
   (min–max band plus average) over a selectable 24h/7d/30d window using
   server-side downsampling; structured CPU and memory reconciliation; then
   one row per
   repository with CPU/memory/storage/health/trends plus the DevCoordinator
   and shared/unattributed rows. Shared storage is a set of complete labelled
   values instead of one clipped sentence. At 960 px and below repository rows
   become labelled cards without document-level horizontal scrolling.
   **Containers** view: every
   container with full identity, state, classification, repository,
   deployment/test, caller and client, CPU/memory/layer size, creation
   time, TTL; removal is offered only for orphaned-managed and managed-test
   containers (unmanaged ones say "decide manually").
7. **Codex Usage** — operator/administrator repository collection first, with
   combined 24h totals and a plain-language **Data included** status. Repository details lead with one
   continuous total/request/tool/execution strip and a stacked UTC token chart
   whose only additive basis is provider `total_tokens`, grouped by work phase.
   Ranked activities, separate request-wall/execution-union/summed-agent rails,
   tool outcomes, exact bucket values, and data-completeness details follow.
   The interface calls each configured input a **Codex environment** and
   explains that each environment is a separate local Codex setup with its own
   usage history. When an environment supplies no data or a measurement is missing, it says some
   usage may be missing and explains that absent values are excluded rather
   than counted as zero. Internal collector status remains an API detail. The page returns no contributor identities, private
   paths, raw Codex IDs, captured content, or per-user values.
8. **Bugs** — open records with occurrence counts and correlations; report
   form; close.
9. **Administration** (administrators only) — users and grants, invitations
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
| Independent Compose-service start/stop/restart (detail only; explicitly declared services) | `deployment.start/stop/restart {component: "stack/service"}` | service badge and aggregate header change; unrelated service and route remain |
| Domain edit / clear (pop-up from list rows and the detail page, administrators) | `deployment.set_domain {deployment_id, domain|null, port?, public?}` | status re-read; route document republished |
| Health range switch (24h/7d/30d) and usage range (1h/24h/7d/30d) | `health.history {minutes, points}` | charts re-render from the store |
| Health container inventory / unhealthy deployment details | — (real hash links) | opens the Containers or exact deployment destination; Back/Health returns to the same Health context |
| Codex Usage repository selection and range (24h/7d/30d) | `usage.repositories {range}` / `usage.repository {repository_id, range}` | repository heading, totals, phase chart, exact table, and data-completeness explanation re-render from canonical reads |
| Progress period (Hour/Day/Week) | `progress.repository {repository_id, period}` | all four aligned evidence lanes, comparison totals, forecast, priorities, coverage, and exact values re-render from one bounded report |
| Progress mode (Delivery pulse/Priorities) | — (client-side) | switches between the two selected views without another API read or loading state |
| Select priority work | — (client-side) | selected row and defer comparison update; task order and release scope remain unchanged |
| Open selected in plan | — (real hash navigation with local task continuation) | opens the same repository Plan with the exact selected task highlighted |
| Progress exact values disclosure | — (client-side) | exposes every visible bucket value, coverage status, and counting method without hover |
| Destination heading link (every route and state) | — (real hash link) | returns to that destination's collection |
| Plan / Progress / Decisions / Codex Usage project menu | destination collection read followed by the selected detail read | custom DOM menu lists every visible project; choosing one changes the same-destination route and visible project |
| Responsive hamburger | — (client-side) | the original navigation links open in a DOM menu; Escape closes it and restores focus; link activation navigates and closes it |
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
| Drag a task between rows or onto a release header (Gantt) | `task.update {task_id, position, release_id?, parent_task_id?}` | plan re-read; the bar moves |
| Move/reorder pop-up on a task row | `task.update {task_id, release_id|null, position?}` | plan re-read; the bar moves under the chosen release |
| Resize a leaf task from its selected bar or exact-number dialog | `task.update {task_id, estimated_loc}` | plan re-read; the estimate persists and following cumulative bars reflow |
| Add the first estimate to an unknown-size leaf | `task.update {task_id, estimated_loc}` | the dashed non-proportional mark becomes a proportional bar after the plan re-read |
| Elaborate on any task row or selected-task tray (administrator) | `task.update {task_id, elaboration_needed: true}` | the existing row and tray are marked `elaboration needed` without replacing the Plan; a persistent notice explains that agents receive the request. The action remains visible but disabled as `Requested` until an agent saves clearer title/outcome wording and clears it atomically. Viewers see the mark but no request control. |
| Select a row/bar and hover a bar | — (client-side) | row and bar highlight together; the bottom tray and anchored hover badge reveal the same task |
| Pan, zoom, fit, horizontal scroll, and minimap navigation | — (client-side) | the cumulative-lines viewport and minimap stay synchronized |
| Select, collapse/expand a task, collapse the selected tray, or collapse the navigator | — (client-side) | only affected DOM state changes; no `plan.overview`, loading state, whole-workspace replacement, or page flash |
| Request preview now (confirm) | `release.request {repository_id}` | plan re-read; pending notice replaces the button; delivery later shows the app link/port |
| Ask-for-a-change form | `task.create {repository_id, title, impact?, kind: user_feedback}` | plan re-read; the task appears with a "your request" badge |
| Drop task (confirm) | `task.update {task_id, status: dropped}` | plan re-read; the task leaves the chart (history kept) |
| Collapse/expand a parent task | — (client-side) | subtree rows hide/show |
| Decision aspect filter / Show older | `decision.tail {repository_id, aspect?, n, before_seq?}` | list re-fetched server-side |
| Decision search | `decision.search {repository_id, query, aspect?}` | matching decisions rendered |
| Sign out | `/auth/logout` | session cleared |

## Browser verification

`CONSOLE_VERIFY_PLAYWRIGHT=<dir with node_modules/playwright> node
console/verify.mjs` runs the real edge and Console against a fake daemon
with fixture scenarios — populated (long names, large numbers, degraded,
alerts), empty, partial analytics, error, loading, permission-denied — at 1280×800 and
390×844 for every destination, checking: no horizontal document overflow,
no clipped headline text, no off-canvas controls outside scroll containers,
explicit empty/error/loading/denied states, humanized large numbers; and
clicks through stop/start/logs/remove/test output/bug report/invite/
container removal proving each calls the API with the expected arguments
and re-renders — including Progress period/mode/priority/exact-value/Plan
continuation, the Plan drag-and-drop (reorder and cross-release),
move pop-up, preview request, feedback form, task drop, decision
filter/search/paging, and a plain-language proof that the agent-facing
technical note never renders. Screenshots and `report.json` are written to
`CONSOLE_VERIFY_OUT` (not committed). The Administration Server line is
asserted to render the daemon version, schema, and served route-document
generation (the edge accepts the dotless `ping` alongside `family.name`
commands). The Plan interaction pass also proves select, pan, zoom, fit,
minimap, navigator collapse/resize, hover, pointer resize success/cancel/failure,
exact resize, persistence after reload, modal cancellation, unknown-size marks,
first-estimate persistence, 111 simultaneous unknown verification jobs,
zero-refetch/zero-replacement selection and disclosure, and read-only or
delivered-release protections. The Codex Usage interaction pass additionally
proves linked navigation, repository selection, phase stacks, all time ranges,
focus restoration, separate timing rails, plain complete/missing/empty/unavailable
data explanations, and exact non-hover values and large-scale axis-label separation. The shared navigation
pass additionally verifies the correct heading link on all 13 routes in every
fixture state, custom project menus on all three repository details, the
reported 799×964 surface, and 1240/1241 px boundary behavior. Last complete
run: 1,397 checks, 0 failures. Current-source formal usage verification checked
missing, complete, and unavailable states at 390×844, 856×915, and 1440×900:
9/9 cells, zero critical findings, and all 18 final viewport/full-page images
passed the manual-review manifest.

The Health layout has an additional focused journey gate at 390×844,
856×915, 1440×1024, 959/960/961×915, and 1199/1200/1201×915. It verifies
capacity/status hierarchy, naturally sized incident cards, the repository
table-to-card transition, every shared-storage label/value, and zero horizontal
document or attribution scrolling. The current focused interaction pass has
138 checks and zero failures; formal verification checked all 9 planned cells
with zero critical findings, and all 18 viewport/full-page images passed the
manual review manifest.
