# Progress redesign design QA

final result: passed

## Comparison target

- Source visual truth: `/home/DevCoordinator2/console/design-reference/progress-factual-release-work.png`
- Browser-rendered implementation: `/tmp/dc2-progress-label-desktop-viewport.png`
- Full-view comparison: `/tmp/dc2-progress-design-qa/comparison-full.png`
- Focused header/forecast comparison: `/tmp/dc2-progress-design-qa/comparison-header.png`
- Focused chart/release-work comparison: `/tmp/dc2-progress-design-qa/comparison-main.png`
- Source pixels: 1487 × 1058. The source was proportionally normalized to 1440 × 1024 for comparison.
- Implementation pixels and CSS viewport: 1440 × 1024 at device scale factor 1.
- State: dark Console theme, Day selected, Tue Aug 25–Mon Aug 31 2026, low-confidence forecast, 94 remaining tasks, 71 without estimates, missing test/token evidence, first release-work row selected.
- Browser geometry: scroll position 0; document and viewport height both 1024 px in the desktop comparison.

## Browser evidence

- The exact isolated hotfix release passed 1,406 complete Console checks with zero failures in `/tmp/dc2-progress-label-hotfix-console-final/report.json`; its coherent interaction inventory passed 152/152.
- Deterministic chart checks at 1440×1024, the reported 858×915 viewport, and 390×844 prove both running lines precede every value label in SVG paint order, every label has a 3 px chart-background halo, all labels are visible, document overflow is zero, and browser console errors are zero.
- Primary Progress interactions tested: repository switching, Hour/Day/Week reads, local task selection, selected-task continuation into Plan, full-Plan navigation, and exact-value disclosure.
- The reference-matched evidence state proves that missing tests and token use produce no zero-valued chart and raw unblock/reopening prose does not render in the compact work list.
- Browser console errors in the final desktop capture: none.
- Formal run `formal-web-ui-mthgf4b5` checked 14 cells at 390, 559/560/561, 799/800/801, 1179/1180/1181, 1319/1320/1321, and 1440 px. It reported zero critical findings; all 14 viewport/full-page pairs received a pass in `/tmp/dc2-progress-formal-redesign-final2/manual-review.json`.
- Focused formal run `formal-web-ui-mtihjr1d` checked the label-layering hotfix at 390, 858, and 1440 px with zero critical findings; all six viewport/full-page images were reviewed and the three pass decisions were finalized in `/tmp/dc2-progress-label-formal/manual-review.json`.

## Findings

- No actionable P0, P1, or P2 difference remains.
- The generated reference's decorative warning-triangle glyph is intentionally omitted rather than replaced with a one-off drawing. Amber confidence text and the `Forecast quality` heading carry the same meaning within the Console's existing icon-light language. This is non-blocking P3 polish.

## Required fidelity surfaces

- Fonts and typography: both views use the Console's system UI stack, compact 10–15 px supporting text, 18–22 px destination/forecast emphasis, and matching medium/bold hierarchy. Long task names wrap without clipping.
- Spacing and layout rhythm: the 1440 px frame preserves the reference's compact shell, one forecast strip, dominant chart, narrower release-work list, two-value comparison, and exact-value footer. The desktop page fits one 1024 px viewport. Responsive evidence shows deliberate stacking without overlap or document-level horizontal overflow.
- Colors and visual tokens: the implementation keeps the source and Console navy surfaces, slate dividers, blue selection/action treatment, green task series, blue planned-line series, amber low-confidence state, and muted explanatory text. Formal contrast checks found no critical issue.
- Image quality and asset fidelity: the target contains no raster product imagery. Charts remain sharp data-driven SVGs; existing Tabler navigation and chevron assets are preserved. No placeholder image, handcrafted decorative SVG, CSS illustration, gradient, or fake asset was introduced.
- Copy and content: `Priority queue`, inferred impact days, dependency claims, imported outcome prose, raw unblock text, and raw event-note prose are absent. Task titles, statuses, estimates, a simple reopened state, forecast gaps, missing-evidence text, and counting language map to recorded response fields.
- States and interactions: selection changes only row state and the Plan-continuation target. Empty, partial, unavailable, loading, denied, and error journeys remain covered by the complete Console suite.

## Comparison history

1. Initial comparison found a P2 density mismatch: the implementation extended roughly 100 px below the 1440 × 1024 reference, omitted daily bar labels, and lacked the reference's compact `Task` column cue. The chart/evidence lanes and release-work rows were tightened, truthful bar labels were added, and the task cue was restored. The revised desktop document fits 1024 px.
2. The first responsive evidence pass found a P2 transition issue just above 1180 px: the two-column workspace made the chart's right edge and legend horizontally dependent on scrolling. The side-by-side breakpoint moved to 1320 px, and explicit 1319/1320/1321 samples were added. The post-fix 14-cell formal run and manual review both passed.
3. Authenticated live acceptance found a P2 content-density recurrence: raw unblock and reopening notes contained internal test names and implementation detail. The compact list now keeps only the owner-facing title, status, estimate, elaboration, and a simple `reopened` state. A 1,404-check rerun with technical marker fixtures and the final source-to-render comparison pass.
4. A live browser comment found a P2 label-layering recurrence: the running-total line painted after the bar values and crossed through several numbers. The line and dots now paint first, bars next, and haloed value labels last. Crossing fixtures, deterministic paint-order checks, the 1,406-check exact-release matrix, and focused visual/formal review all pass.

## Implementation checklist

- [x] Match the selected desktop hierarchy and Console visual system.
- [x] Render daily completions as bars and running totals as unfilled lines.
- [x] Keep missing tests and token values blank and explicitly explained.
- [x] Show release work in factual Plan order with local selection.
- [x] Preserve exact task continuation and every existing supporting control.
- [x] Pass source tests, complete Console regression, formal responsive verification, changed visual review, and source-to-render comparison.

## Follow-up polish

- Optional P3: add the official matching alert icon from the Console's chosen icon library if that library is expanded later; do not substitute a text glyph or handcrafted drawing.
