# Formal Web UI Verification

This skill runs a deterministic Playwright/Chromium heuristic over rendered
web pages. It measures DOM geometry, computed visibility, clipping, occlusion,
off-canvas controls, broken media, contrast risks, document overflow, and
visible scrollbars with their same-axis nesting chains. Before scrolling, it
also measures browser navigation TTFB and document LCP. Defaults require local
TTFB below 10 ms and LCP below 800 ms; global or per-target contracts may
prescribe other strict thresholds. Every horizontal
scrollbar is a warning; nested horizontal scrolling is blocking. Two vertical
scroll layers warn and three or more block, with the document scrollbar
counting as a layer and mixed axes kept separate. It also measures rendered
placeholders and selected option labels without retaining their text, supports
opt-in readable-content inset
contracts, and samples immediately around declared responsive breakpoints.
It traverses discoverable open shadow roots, evaluates
Playwright-reachable frames, supports mobile device descriptors, and can open
declared interaction states with bounded actions. It complements screenshots
and human review; it cannot discover closed shadow roots or prove undeclared UI
states correct, and it reports reachable contexts it cannot evaluate as
coverage limits.

Every effective target/state also declares its primary journey, frequency/risk
context, semantic rendered regions, light/dark/mixed theme, and
repository-relative UI review inputs. The verifier rejects secondary workflows
above primary content, offscreen or unfocused continuation, nested horizontal
scrolling, triple vertical scroll nesting, WCAG text contrast failures, and
large declared-theme contradictions, plus applicable TTFB/LCP threshold
breaches. Palette-cohesion risks stay
explicit agent-review evidence rather than automatic aesthetic verdicts.

Each checked cell automatically captures a redacted initial viewport and full
page. `review-queue.json` contains only cells whose mapped UI inputs or
journey/theme intent changed, or whose route/state/viewport is new. Screenshot
hashes prove artifact integrity and never trigger review.

The verifier also supports fast, safe-complete execution: exact event/readback
waits, zero-wait handoff for conditionally owned controls, bounded concurrency
with resource locks, journey-priority ordering, one in-memory authentication
bootstrap per role, changed-input development selection, and an explicit
content-addressed development cache. Ordinary failures do not stop later safe
cells. Subsets and cache hits are always marked ineligible for readiness; final
delivery still requires a fresh complete run.

Run the self-test before relying on it:

```bash
python3 skills/formal-web-ui-verification/scripts/self_test.py
```

The self-test resolves Playwright explicitly from the repository's locked
`ci/playwright/node_modules` installation (or
`FORMAL_WEB_UI_PLAYWRIGHT_NODE_MODULES`) and passes that path to the verifier.
It does not depend on the temporary audit directory or a manually injected
`NODE_PATH`. If the locked dependency has not been installed in a checkout,
run `npm ci --ignore-scripts --prefix ci/playwright` once.

Verify explicit targets through a complete config. Bare `--url` targets fail
coverage because they have no journey/theme/input contract:

```bash
node skills/formal-web-ui-verification/scripts/formal_web_ui_verify.mjs \
  --config formal-web-ui.json \
  --fail-on critical
```

See `references/journey_review_contract.md` for the complete target/state
schema and changed-review workflow.

Target-specific breakpoint samples, readable insets, and deployment/source
binding are configured together:

```json
{
  "repoRoot": "/absolute/path/to/repository",
  "targetDefaults": {
    "journeys": [{"id": "view-items", "frequencyPercent": 100, "risk": "normal"}],
    "primaryJourney": "view-items",
    "regions": [{"selector": ".items-card", "role": "primary-content", "journey": "view-items"}],
    "theme": "light",
    "reviewInputs": [{"path": "src/items", "kind": "ui-code"}]
  },
  "targets": [{
    "url": "http://127.0.0.1:3000/items",
    "breakpointProfile": {
      "name": "items-layout",
      "breakpoints": [768, 1024],
      "height": 900
    },
    "contentInsets": [{"selector": ".items-card", "min": 12}],
    "sourceBinding": {"expected": "git:abc123"}
  }],
  "viewports": [{"name": "desktop", "width": 1440, "height": 900}],
  "execution": {"maxConcurrency": 4},
  "performance": {"ttfbMs": 10, "lcpMs": 800, "ttfbLocalOnly": true},
  "maxPageCount": 12
}
```

Each breakpoint adds `breakpoint−1`, `breakpoint`, and `breakpoint+1` for only
that target. Equivalent cells are de-duplicated. Expansion above
`maxPageCount` fails setup instead of silently sampling fewer pages. A source
binding reads `X-UI-Source-Revision` (or a configured meta name/header) from
the deployment and fails coverage when it is missing or differs from the
expected source value.

The default invocation creates a unique external artifact directory (normally
under the system temporary root), writes complete `report.json` and `report.md`
files, `journey-evidence.json`, `review-queue.json`, bounded `progress.jsonl`, and screenshot pairs, then prints one bounded JSON
receipt with the exit code, coverage, counts, directory, and filenames. Use
`--json-out` and `--markdown-out` to select known artifact paths; supplying one
derives the other. Setup/configuration failures use the same bounded receipt and
machine-readable artifact contract when a safe destination is available.
Reports record run and per-cell start/end times, verifier and privacy-safe
effective-config SHA-256 hashes, requested and final paths, every exact
route/state/viewport cell, sampled-only width coverage, and per-cell
deployment/source binding status. A sign-in redirect or stale bound deployment
does not count as checked coverage.

When the verifier runs as a governed DevCoordinator2 check, its automatic
bundle is written to the private run-leaf evidence directory. The
`journey-evidence.json` file is the Console-facing, path-free index of ordered
route/state/viewport cells and their immutable masked screenshots; it expires
with the same age/depth policy as that run's logs.

Advanced execution, ownership, authentication, changed-selection, cache, and
readiness schemas are documented in
`references/journey_review_contract.md`. Development cache storage is disabled
by default, must use an explicitly supplied existing external directory and
data revision, and never contains cookies or authentication storage state.

Full Markdown stdout is available only through the explicit human-terminal
compatibility flag `--human-readable-stdout`. Do not use that flag for agent
runs.

The formerly published `--receipt-only` flag and boolean config `receiptOnly`
remain accepted as deprecated no-op compatibility inputs. Neither changes the
safe default, and `receiptOnly: false` cannot enable full stdout.

Exit codes:

- `0`: required pages were checked and no configured finding threshold failed.
- `1`: blocking UI findings were detected.
- `2`: configuration, browser, or dependency setup failed.
- `3`: a required target could not be checked, redirected to another route,
  failed its source binding, or the minimum checked-page count was not met.

After all automatic checks complete, open only the queue's screenshot pairs and
finalize decisions with `scripts/formal_web_ui_review.py`. A verifier exit `0`
with pending changed review is not visual completion; unchanged prior gaps stay
blocking without reopening the same images.

Explicit target failures are fail-closed. Coordinator-discovered failures can
be tolerated only with the explicit `--allow-discovered-target-failures` flag,
and remain visible in the report.

`--from-coordinator` is optional and calls the installed `devcoordinator2`
command by default. `--coordinator-command` may name another executable. The
skill consumes only typed deployment-list output and does not import, clone,
pin, build, or test Coordinator source.
