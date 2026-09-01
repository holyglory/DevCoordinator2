# UX Principles For Journey Documentation Audits

Use this reference when judging whether docs are strong enough to support an easy, rich, user-centered app.

## Source Anchors

- Nielsen Norman Group's 10 usability heuristics: visibility of status, real-world language, user control, consistency, error prevention, recognition over recall, flexibility, minimalist design, error recovery, and help/documentation.
  Source: https://www.nngroup.com/articles/ten-usability-heuristics/
- MIT OpenCourseWare user-centered design material emphasizes user analysis, task analysis, contextual inquiry, and success criteria before design decisions.
  Source: https://ocw.mit.edu/courses/6-811-principles-and-practice-of-assistive-technology-fall-2014/resources/mit6_811f14_usercentered/
- WCAG organizes accessibility around perceivable, operable, understandable, and robust interfaces.
  Source: https://www.w3.org/WAI/standards-guidelines/wcag/glance/
- Apple Human Interface Guidelines for disclosure controls: expose likely controls first and hide more advanced functionality by default.
  Source: https://developer.apple.com/design/human-interface-guidelines/disclosure-controls
- Material Design responsive layout guidance treats layouts and navigation as adaptive to available screen width.
  Source: https://m1.material.io/layout/responsive-ui.html

## Documentation Must Enable These Decisions

- What the user is trying to accomplish.
- Which journeys are most frequent, most valuable, and most risky.
- Which information is required at each step.
- Which actions are primary, secondary, rare, destructive, or expert-only.
- Which details are required, conditional, rare, destructive, debug, or expert-only and why the user needs them.
- How lower-importance information should be accessed: inline, hover/focus hint, row selection, expansion, drawer, modal, or deep detail view.
- Which interactive signals should respond to hover, focus, click, keyboard, or row selection, whether they reveal popover/detail information, whether they navigate elsewhere, and how navigational affordance is shown.
- Which temporary panels, popovers, and expanded details close explicitly, on outside click, on focus loss, after idle/leave timeout, or persist by product intent.
- Which message metadata is decision-relevant versus passive: sender labels, routing labels, timestamps, copy controls, concise tool/runtime status, raw execution details, and whether metadata should be selectable.
- Which states must exist: loading, empty, error, success, partial success, denied, offline, retry, undo, and recovery.
- Which constraints apply: mobile, accessibility, latency, permissions, security, data loss, stress, expertise, and localization.
- For each web route/state, which journey is primary, its usage estimate and
  risk, which semantic regions may precede it, how activation continues without
  a viewport hunt, the intended light/dark/mixed theme, which implementation
  inputs own visual change, and which initial/full-page evidence needs review.

## Signs Of Excellent Journey Docs

- A new designer can choose an interface structure without asking what matters most.
- A new engineer can implement states and routes without inventing user intent.
- QA can write scenario tests, edge tests, and mobile checks from the docs alone.
- Support can understand what users were trying to do when they failed.
- Product can defend why something is critical, frequent, secondary, conditional, rare, destructive, debug, or expert-only.
- UI handoff explains access expectations for lower-importance information without freezing an unnecessary layout recipe.
- UI handoff explains interaction targets, feedback, and disclosure stability for badges, flags, expandable rows, icons, popovers, scrollable regions, and tool/result blocks.

## Common Gaps

- Docs list features but not user goals.
- Docs describe screens but not decisions users make on those screens.
- The happy path is documented but empty/error/permission/destructive paths are absent.
- Device constraints are mentioned but the required decisions and information needs are not defined.
- Rare details are treated as equal to critical information.
- Navigation names are internal system terms rather than user language.
- Docs say "dense", "dashboard", "command center", "overview", "compact", "settings", or "admin" without defining what decisions users make there.
- Accessibility is reduced to color contrast and ignores keyboard, focus, labels, and understandable language.
- Interactive badges, rows, and icons are named but not given interaction targets, hover/focus/click feedback, keyboard behavior, or detail access.
- Navigational rows or explanations look actionable but docs do not define the destination, cursor/focus affordance, and return path.
- Copy buttons, tool/result summaries, or status chips are documented as UI elements without deciding what is default-visible, hover-only, detail-only, or passive metadata.
- Message streams document content but not whether authorship, routing labels, timestamps, or tool status are useful visible metadata.

## Documentation Failures That Create Wrong UI

- Feature lists without a journey decision model let implementers treat settings, filters, and primary content as equal.
- Layout language copied from source, screenshots, or mockups can become false product truth when docs do not define the underlying user decisions.
- Mockups or screenshots are not enough unless the docs explain which decisions, facts, warnings, actions, and states they are meant to support.
- Requirements that only say "compact", "dense", or "no horizontal overflow" miss the more important question: what decision must the user make in that context?
- Overexposed configuration is a documentation risk when docs do not say which controls are frequent, occasional, rare, admin-only, or conditional.
- A UI implementation audit needs handoff constraints: primary goal, primary decision, required facts, action frequency, importance class, lower-importance access expectations, states to verify, and evidence expectations.
- A formal verifier cannot infer product priority from source or DOM order.
  Without primary-journey frequency/risk, initial-viewport region roles,
  continuation, theme, review-input ownership, and changed-review evidence, a
  geometry pass can certify the wrong page hierarchy.
- Handoff docs can accidentally create wrong UI when "required visible evidence" or "must show" lists include secondary/detail/debug facts without separating default decision state from hover/focus hints and click/drill-down detail access.
- Icon-only expansion, badge-only detail, and popover-only information can create wrong UI when docs do not define the intended target size, row activation, keyboard path, hover/focus feedback, and relationship to nearby scrollbars.
- Message metadata becomes visual noise when docs do not explain whether sender/authorship labels, routing labels, timestamps, and execution metadata are decision-driving content or passive context.
