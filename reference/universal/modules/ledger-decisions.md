## 4. Keep decisions, unfinished outcomes, and executions separate

- Use the configured software-owned database as the authoritative
  completion ledger and decision history, including user feedback and
  durable corrections. Access it through the configured coordinator's
  supported tools; never substitute files or chat memory. Database
  unavailability blocks the affected completion claim, not independent
  authorized work.
- Record consequential decisions with `decision_record`: an aspect,
  management-facing title and body, materially distinct options, and
  meaningful cost and risk. Put implementation detail in `technical_note`;
  use `supersedes` when replacing a decision and a stable `ref` when cited.
- Load routine decision context with `decision_tail`. Search the full
  history with `decision_search` before retrying an option that may have
  been rejected.
- When a decision read reports `summary_due`, store the rolling summary
  with `decision_summarize` before continuing. Summarize durable direction
  and quality expectations, distinguish confirmed decisions from inferred
  patterns, and cite decision refs. This append-only maintenance needs no
  additional user approval.
- The completion ledger records durable unfinished outcomes, not execution
  attempts. Every passed, failed, cancelled, timed-out, invalidated,
  retried, or superseded run belongs in governed run history.
- Diagnose before changing work state. Create or reopen a task only when
  evidence establishes a durable missing or regressed outcome not already
  represented. Do not automatically turn failures or suggestions into
  tasks, or passing runs into completion.
- Execution-only actions are not tasks. Missing test or harness capability
  may be an outcome; running or rerunning it is not.
- Keep every agreed gap active until resolved or explicitly removed. Size
  and split large work, preserve append-only history, and keep externally
  blocked outcomes open.
- Write tasks for a non-specialist: the remaining outcome, user impact,
  unblock condition, and observable completion proof come first.
  Technical details may follow.
- Link runs to tasks through structured evidence references. Keep compact
  run receipts after verbose evidence expires; do not copy run status,
  logs, or failure narratives into task history.
- Readiness requires both no request-related unfinished outcome and fresh
  required verification evidence.
