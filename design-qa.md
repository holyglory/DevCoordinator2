# Repository deployments dashboard design QA

final result: passed

## Comparison target

- Source visual truth: `/home/DevCoordinator2/console/design-reference/deployments-repository-dashboard.png`
- Normalized source comparison: `/tmp/dc2-deployments-target-1440x1024.png`
- Browser-rendered implementation: `/tmp/dc2-deploy-dashboard-formal-final/screenshots/cell-0004-repository-deployments-dashboard-desktop-viewport.png`
- Focused legacy-repository comparison: `/tmp/dc2-deployments-target-legacy-focus.png` and `/tmp/dc2-deployments-implementation-legacy-focus.png`
- Focused managed-repository comparison: `/tmp/dc2-deployments-target-repo-focus.png` and `/tmp/dc2-deployments-implementation-repo-focus.png`
- Source pixels: 1487 × 1058, proportionally normalized to 1440 × 1024 for comparison.
- Implementation pixels and CSS viewport: 1440 × 1024 at device scale factor 1.
- State: populated dark Console fixture with one healthy observed repository and one managed repository containing a running deployment and a degraded long-content deployment.

## Browser evidence

- The complete Console matrix passed 1,493 checks with zero failures in `/tmp/dc2-deploy-dashboard-final-browser2/report.json`.
- The focused repository-dashboard inventory passed 229/229 checks across 320, 390, 430, 619/620/621, 834, 959/960/961, 1179/1180/1181, 1239/1240/1241, and 1440 px in `/tmp/dc2-deploy-dashboard-interactions-final/report.json`.
- Primary interactions tested: every repository-specific Plan, Progress, Codex Usage, and Decisions continuation; repository-labelled Tests and Health continuations; domain editing; managed and observed lifecycle actions; apply-state disabling; and exact post-action API reads.
- Attribution fixtures deliberately give the two repositories different plan, progress, usage, test, health, and decision states. Assertions prove that no deployment or summary value appears in the other repository section.
- Empty, loading, error, permission-limited, applying, observed, degraded, long-content, missing-data, and partial-usage states remain covered by the complete matrix.
- Browser console errors in the final comparison: none.
- Formal run `formal-web-ui-mtit1v0d` checked 16 exact cells covering desktop, tablet, two mobile sizes, and ±1/at samples around 620, 960, 1180, and 1240 px. It reported zero critical findings and passed coverage.
- All 16 initial-viewport/full-page pairs were manually reviewed and finalized with zero gaps in `/tmp/dc2-deploy-dashboard-formal-final/manual-review.json`.
- Three formal warnings occurred when a below-fold domain-edit control landed exactly on the 960 px viewport edge. The reviewed full-page evidence shows each control fully visible and reachable through normal document scrolling; no element covers it.

## Findings

- No actionable P0, P1, or P2 difference remains.
- The generated target used an ambiguous copy-like glyph beside domains. The implementation deliberately uses the existing small text button labelled `edit`, because the real action changes the routed domain and must not imply copying. This is a truthful interaction refinement, not design drift.
- The implementation adds a small `3 of 4 environments included` line beneath partial Codex usage. The generated target omitted completeness, but the established Console contract requires missing usage to remain visible and never look complete. The note stays visually subordinate.
- Optional P3: the implementation keeps the existing Console shell's slightly denser typography and filled small-button treatment instead of enlarging the global shell to match ImageGen. This preserves the approved cross-page navigation system and does not weaken the selected page hierarchy.

## Required fidelity surfaces

- Fonts and typography: both views use the Console system UI stack with the same strong repository headings, compact labels, readable values, blue continuations, and monospaced repository/deployment identities. Long names and domains wrap without truncation or collision.
- Spacing and layout rhythm: desktop preserves two full-width repository sections, six aligned summary columns, status rails, and five operational deployment zones. The summary switches to 3-by-2 before the shell becomes compact; deployment facts switch at 1180 and 960 px; mobile uses labelled stacked rows. No document-level horizontal scrolling, clipped value, or off-canvas control remains.
- Colors and visual tokens: the implementation preserves the source and product's navy/slate surfaces, cool dividers, blue links, green running/healthy treatment, amber degraded/attention treatment, red unhealthy treatment, and muted evidence metadata. Formal contrast and declared-dark-theme checks produced no critical finding.
- Image quality and asset fidelity: the target contains no raster content or decorative illustration. The implementation uses the existing Tabler arrow asset for continuation links and adds no placeholder, handcrafted SVG, CSS illustration, gradient, or fake icon.
- Copy and content: every visible number and state maps to the fixture/API contract. Missing plan, progress, usage, tests, and decisions remain explicit; partial usage names its incompleteness; no global summary mixes repositories; the removed declared-only side area does not return.
- States and interactions: all enabled controls call existing real commands and re-read state. Viewer/operator/administrator differences, observed-only behavior, applying-state disabling, failure recovery, and repository-scoped navigation remain honest.

## Comparison history

1. The first implementation comparison found a P2 density mismatch against the approved desktop mock: headings, summary values, and deployment rows were too compact. Typography, vertical rhythm, and record spacing were increased while retaining the established Console shell.
2. The first formal pass found a P1 tablet clipping defect: the four-column deployment grid extended 70 px beyond an 834 px repository section. The two-column tablet transition moved to 960 px, and exact 959/960/961 samples now prove all fields and controls remain contained.
3. Focused visual review found a P2 summary-density issue just above 1180 px: six repository summary columns forced long repository-labelled links into cramped wraps. The summary transition now aligns with the 1240 px navigation boundary, while the independent deployment grid keeps its 1180 px boundary. Exact 1179/1180/1181 and 1239/1240/1241 samples pass.
4. The final full-view and focused source-to-render comparisons found no remaining actionable mismatch. The complete Console matrix, formal geometry/contrast pass, and immutable manual-review manifest all pass on the final source.

## Implementation checklist

- [x] Match the selected repository-centred desktop hierarchy and Console visual system.
- [x] Keep each repository's plan, progress, usage, tests, health, decision, and deployments strictly attributed.
- [x] Continue to the exact repository-specific destinations where those routes exist.
- [x] Preserve every real lifecycle and domain action without inventing dashboard mutations.
- [x] Show missing, partial, restricted, and failed evidence truthfully.
- [x] Pass desktop, tablet, several mobile sizes, every responsive boundary, full Console regression, formal verification, manual visual review, and source-to-render design QA.

## Follow-up polish

- No follow-up is required for the requested dashboard.
