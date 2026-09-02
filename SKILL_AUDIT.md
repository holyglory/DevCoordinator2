# DevCoordinator2 Agent Skills Audit

## Ownership

DevCoordinator2 owns exactly six agent skills and the shared audit harness.
Installed entries are direct links to this repository; there is no second
writable skills repository or installed copy to edit.

## Skill boundaries

### `dev-coordinator`

Owns governed local runtime observation and mutation, planning work, and
decision history through the product CLI or MCP contract. It is coupled to this
repository and validated against the current interfaces.

### `formal-web-ui-verification`

Owns deterministic browser evidence for declared routes, states, viewports,
journey hierarchy, continuation, performance, contrast, theme, geometry,
scroll topology, screenshot pairs, and changed-input review selection.

### `full-repo-audit`

Owns explicit exhaustive implementation and contract tracing, deterministic
batches, source evidence, lead reconciliation, and prioritized remaining work.

### `full-repo-test-coverage-audit`

Owns exact structural test targets and optional empirical coverage ingestion.
It does not present structural evidence as proof that tests executed.

### `ui-implementation-audit`

Owns explicit exhaustive review of an existing substantive UI against product
journeys, source wiring, design evidence, rendered evidence, and tests. It
consumes formal Web evidence rather than recreating it.

### `user-journey-docs-audit`

Owns audits of an existing product/journey documentation set for product
context, journey decisions, feature and UI inventories, edge cases,
implementation expectations, tests, and usability acceptance criteria.

## Shared contract

Shared skill instructions and generated prompts are runtime-neutral. They
require fresh isolated workers, complete prompt delivery, bounded artifacts,
and honest fallback without naming a runtime's private controls. Provider and
installation adapters may retain factual runtime-specific identifiers.

`full_repo_harness/` is the only shared harness source. The three audit skills
resolve their installed direct links to this checkout and import it from the
root; copied standalone skill directories are unsupported.

## Verification expectation

| Gate | Required result |
| --- | --- |
| Canonical ownership | Exactly six skill directories and `reference/universal/AGENTS.md` |
| Runtime neutrality | No runtime-branded shared contracts or generated prompts |
| Universal policy | Semantic policy checker and realistic self-tests pass |
| Issue ledgers | All scoped ledgers pass path, title, namespace, and table checks |
| Repository boundary | No live dependency on retired checkout, remote, paths, names, or release copies |
| Shared harness | One root harness; no audit-skill vendored copies or fallback imports |
| Canonical skills | Six complete in-repository self-tests through the live linked layout |
| Execution | One strict schema-2 plan executed by Rust `run-local`; invalidating preflights and all-settled independent checks proven |
| Coordinator skill | Current CLI, MCP, metadata, and skill contract agree |
| Browser runtime | Locked Playwright/Chromium fixtures pass |
| Link managers | Plan/apply/verify/rollback and source/target drift tests pass |

Run `python3 scripts/skills/validate.py` for the complete gate.
