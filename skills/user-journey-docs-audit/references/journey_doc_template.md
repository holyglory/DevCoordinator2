# User Journey Documentation Template

Use this template when recommending docs to add or repair.

## App Idea

- Product promise:
- Primary users:
- Primary value:
- Non-goals:
- Main devices/contexts:

## User Contexts

| User type | Expertise | Frequency | Environment | Stress/urgency | Mistake cost |
| --- | --- | --- | --- | --- | --- |

## Journey Inventory

| Journey | User | Frequency | Importance | Risk if broken | Entry point | Success state |
| --- | --- | ---: | ---: | ---: | --- | --- |

## Journey Detail

### Journey Name

- User:
- Goal:
- Trigger:
- Preconditions:
- Entry point:
- Route/screen sequence:
- Primary decisions:
- Required information per step:
- Warning/flag conditions:
- Primary actions:
- Secondary/rare actions:
- Usage percentage or best available frequency estimate:
- Risk if broken:
- Is this the primary journey on each surface/state?:
- Priority override reason when a lower-frequency journey is primary:
- Conditional or rare details and when they matter:
- Interaction targets and feedback:
- Message metadata relevance:
- Unresolved UI assumptions:
- Empty/loading/error/permission states:
- Recovery and undo:
- Device/context constraints:
- Accessibility expectations:
- Initial-viewport region roles: primary content, secondary workflow surfaces, compact supporting regions, and justified blocking alerts:
- Activation continuation: visible anchor, focus destination, document-scroll expectation, or expected destination route:
- Theme intent per state: light, dark, or mixed:
- Implementation-input ownership: UI code, styles, tokens, fonts, and assets that an implementation must map into formal verification:
- Visual evidence: initial-viewport and full-page screenshot pair plus changed-review expectations:
- Acceptance criteria:
- Analytics/support signals:

## Journey Decision Model

| Surface | Primary user goal | Primary decision | Required facts | Warning/flag conditions | Frequent actions | Secondary/rare actions | Unresolved assumptions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Screen or route | User outcome this surface must support | Decision the user must be able to make | Facts the UI must provide for that decision | Conditions that change urgency, safety, or next action | Common actions | Occasional, expert, destructive, admin, or configuration actions | Product or journey questions that remain open |

## Information Relevance Inventory

| Journey | Surface | Item | Relevance | Why it matters | Condition/frequency | Evidence source |
| --- | --- | --- | --- | --- | --- | --- |
| Journey name | Screen or route | Fact, control, status, warning, or detail | critical-always / primary-frequent / secondary-occasional / rare-under-5-percent / conditional / debug / expert-only | Decision or task this item supports | Always, frequent, conditional, rare, destructive, admin, debug, or expert-only | Expected access: inline / hover-focus hint / row selection / expansion / drawer / modal / deep detail view | Doc, user answer, metric, support signal, or source hint |

## Interaction And Metadata Model

| Surface | Element or metadata | User intent | Interaction target | Feedback/detail access | Stability/accessibility expectation |
| --- | --- | --- | --- | --- | --- |
| Screen or route | Badge, flag, row, disclosure, timestamp, sender label, tool/result block, or icon | Decision or task this supports | Whole row / badge / icon / keyboard focus / not interactive | Hover/focus state, click popover, row selection, expansion, modal, deep detail, or passive metadata | Stable width/position, no scrollbar collision, readable icon meaning, non-selectable passive metadata, accessible name, focus behavior |

## UI Handoff Constraints

| Surface | Decisions the UI must support | Required evidence for UI audit | States to verify | Mockups/screenshots/assets | Unconfirmed assumptions |
| --- | --- | --- | --- | --- | --- |
| Screen or route | Decisions and required facts from the journey model | Screenshot, rendered state, visual tree, DOM/native measurement, accessibility evidence, or blocker | Loading, empty, error, permission, success, warning, destructive, or recovery states | Relevant design artifacts | Open product/journey questions |

Do not turn this table into a fixed layout recipe. If default-visible content is product-critical, state the decision it supports and separate default state, hover/focus hints, row selection, expansion, drawer, modal, and deep detail access. Otherwise leave exact grouping, widgets, and disclosure controls to UI design and `ui-implementation-audit`.

## Formal Web UI Verification Handoff

| Surface/state | Primary journey | Usage % | Risk if broken | Priority override reason | Initial-viewport region roles | Activation continuation | Theme intent | Implementation-input ownership | Evidence/review expectation |
| --- | --- | ---: | --- | --- | --- | --- | --- | --- | --- |
| Route and state | Stable journey id and user-readable name | Best available estimate | critical / high / normal / low | Required when a lower-frequency journey is primary | primary-content, workflow-surface, compact supporting region, justified blocking-alert | visible/focused continuation anchor without document jump, or expected destination route and visible primary anchor | light / dark / mixed | Components plus UI code, styles, design tokens, fonts, and assets the implementation must map to repository-relative review inputs | Initial-viewport and full-page screenshot pair; manual review only for changed mapped inputs/intent or new cells |

This table defines semantic intent, not CSS selectors or a widget prescription.
The implemented verifier configuration owns the exact selectors and
repository-relative input paths. Every audited target, including component
demos and utility routes, needs a complete mapping before formal journey
verification can pass.

When flags, badges, expandable rows, message streams, tool/result blocks, copy controls, or icons are part of a journey, document their intent rather than only naming the component. State whether the whole row or only the icon is the activation target, what hover/focus/click feedback reveals, whether a navigational item has a predictable destination and pointer/focus affordance, whether popover detail is expected and how it closes, which copy/status controls appear only on hover/focus, and which metadata such as sender labels or timestamps is passive rather than selectable content.

### Example: Primary Decision Surface

- Primary user goal: decide whether the current item needs action.
- Primary decision: act now, inspect detail, or continue monitoring.
- Required facts: current status, latest update time, severity, and any threshold warning.
- Frequent actions: perform the current workflow action or inspect the item.
- Secondary/rare actions: adjust display preferences or open advanced configuration when needed.
- UI handoff constraint: the implementation audit must verify that the rendered surface lets the user make this decision and that conditional detail is reachable without overwhelming the decision path.

## Screen Requirements

| Screen | Journey | Critical info | Primary actions | Secondary actions | Rare details | Device/context constraints |
| --- | --- | --- | --- | --- | --- | --- |

## QA And Acceptance

- Happy-path scenarios:
- Edge cases:
- Failure/recovery scenarios:
- Mobile scenarios:
- Accessibility checks:
- Test data or fixture mode:
