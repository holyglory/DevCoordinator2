# Progress dashboard design QA

final result: passed

## Comparison target

- Source visual truth: `console/design-reference/progress-dashboard-combined.png`
  (`1487 × 1058` pixels), normalized to `1440 × 1024`.
- Rendered implementation: `/tmp/dc2-progress-implementation-final.png`
  (`1440 × 1024` pixels, CSS viewport `1440 × 1024`, device scale factor `1`).
- State: populated repository, Delivery pulse, daily period, complete evidence,
  dark theme.
- Combined full-view evidence:
  `/tmp/dc2-progress-design-qa-comparison-passed.png`.
- Focused priority/scenario evidence:
  `/tmp/dc2-progress-design-qa-priority-passed.png`.

## Findings and iteration history

### Initial comparison — blocked

- **[P2] The desktop composition extended below the selected 1440 × 1024
  frame.** The first implementation used full-detail priority rows beside a
  500px chart and a taller forecast strip. The period comparison and exact
  values fell below the target viewport, weakening the selected one-screen
  decision hierarchy. The implementation was tightened by replacing the pulse
  sidebar with a compact top-three summary, reducing chart height, shortening
  forecast copy, adding the visible UTC range, and reserving the full reasons
  for the Priorities mode. Post-fix geometry is exactly `1440 × 1024` with the
  comparison band and disclosure visible.
- **[P2] The activated priority view followed a tall stacked forecast on narrow
  screens.** This delayed the content the user explicitly selected. At 520px
  and below, the ranked queue and scenarios now move directly after the view
  controls; forecast evidence remains complete immediately afterward. The
  390px continuation is visible and focused without a document jump.

### Final comparison — passed

No actionable P0, P1, or P2 differences remain. Both selected directions are
present as complementary modes: the default shared-time-axis pulse includes a
compact priority/scenario decision column, while Priorities exposes the full
ranked queue, scenarios, and KPI signals.

Three visible differences are intentional product constraints rather than
design drift:

- The selected mock's dashed future task/line paths and token budget were not
  copied as stand-ins. The product shows measured buckets and puts the real
  deterministic date range, confidence, and assumptions in the forecast.
- Dynamic repository outcomes, task estimates, tests, tokens, and dates replace
  the mock values. Missing target dates, test history, token history, and task
  estimates are named honestly.
- Decorative mock icons are omitted where the existing Console has no matching
  asset or where plain labels are clearer; no handcrafted SVG, CSS-art icon, or
  placeholder asset was introduced.

## Required fidelity surfaces

- **Fonts and typography:** The implementation uses the Console's system UI
  stack and reproduces the reference's compact weights, 10.5–15px support
  scale, 18px KPI emphasis, uppercase metric labels, and readable line height.
  Long real outcomes wrap in the full view and clamp only in the compact pulse
  summary, where the complete text remains in Priorities.
- **Spacing and layout rhythm:** The global shell, 20px desktop inset, compact
  control row, six-part forecast strip, dominant two-column workspace, thin
  section dividers, comparison band, and square surfaces match the target.
  Final desktop document height equals the `1024px` viewport.
- **Colors and visual tokens:** Existing navy surfaces, slate rules, blue
  selection, green task progress, blue planned lines, mint test stability,
  purple tokens, amber uncertainty, and red adverse movement map directly to
  the selected direction. Formal contrast/theme checks found no critical issue.
- **Image quality and asset fidelity:** This operational dashboard has no
  raster imagery, logos, avatars, or illustration assets. Dynamic charts are
  accessible data visualizations, not asset substitutes; no generated image or
  custom decorative SVG was required.
- **Copy and content:** Every static label explains the real measurement.
  “Planned lines completed” explicitly means current task estimates, token use
  is provider `total_tokens`, scenarios state that they do not mutate Plan, and
  ordering alone does not falsely move the central date.
- **Icons:** The route preserves the existing Tabler-backed global shell. The
  new decision surface relies on text, selection borders, and native buttons
  where the source's decorative icons would add no action meaning.
- **Responsive and accessibility:** Delivery pulse, Priorities, and weekly
  states pass at `390 × 844`, the reported `799 × 964`, and `1440 × 1024`.
  Mode/period controls restore focus, priority rows are keyboard buttons,
  chart scroll is contained on narrow screens, exact values need no hover, and
  the selected task continues to the exact Plan row.

## Browser evidence

- Primary interactions tested: repository collection/detail, project menu,
  Delivery pulse/Priorities switching, task selection, hourly/daily/weekly
  reads, exact-value expansion, partial evidence, and exact Plan continuation.
- Loading, empty, error, denied, populated, partial, long-content, desktop, and
  narrow states were rendered. Browser console errors: none.
- Complete Console inventory: 1,397 checks, 0 failures.
- Formal verifier: 9/9 Progress cells checked, 0 critical findings, 6
  review-only warnings; all nine viewport/full-page pairs manually reviewed.
- Formal report: `/tmp/dc2-progress-formal-final3/report.json`.
- Reviewed manifest: `/tmp/dc2-progress-formal-final3/manual-review.json`.

## Follow-up polish

- The measured chart intentionally omits a fabricated future series. A later
  product decision could add a separately labelled, modelled projection lane
  if its source data and uncertainty contract are expanded.
