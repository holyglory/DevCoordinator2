# Marker-free implementation-gap evaluations

This suite evaluates whether a manual repository auditor finds semantic
implementation gaps that are not announced by marker comments or exception
names. It also checks precision: each repository contains an intentional
lookalike that must remain unreported as a defect.

## Cases

| Case | Detection class | Agent input |
| --- | --- | --- |
| `ignored-input-config` | An accepted input or configuration value is not used by the promised behavior. | `cases/ignored-input-config/repo/` |
| `partial-plumbing-no-dependency-call` | Data is prepared and success is returned without invoking the real dependency. | `cases/partial-plumbing-no-dependency-call/repo/` |
| `false-persistence-success` | A write path reports persistence or success without making the durable write. | `cases/false-persistence-success/repo/` |
| `missing-registration-lifecycle` | Implemented behavior is absent from the registration or lifecycle path that makes it run. | `cases/missing-registration-lifecycle/repo/` |
| `production-fixture-mock` | A production path is backed by sample data instead of its required source. | `cases/production-fixture-mock/repo/` |
| `shallow-outcome-tests` | Tests establish shape or type but not the contract's observable outcome. | `cases/shallow-outcome-tests/repo/` |

Every `repo/README.md` is product truth for that isolated repository. Give the
auditing agent only the selected `repo/` directory and the response format
below. Never give it the case directory, `oracle.json`, this suite README, or
another case. The oracle is deliberately a sibling of `repo/`, so it is not
inside the agent input tree.

## Manual run

For each case:

1. Start a fresh agent context.
2. Provide only `cases/<case-id>/repo/` as repository input.
3. Ask the agent to audit the complete repository for unfinished or incomplete
   implementation, including behavior that has no explicit marker.
4. Require a JSON response conforming to `response.schema.json`. A finding's
   `symbol` should be the narrowest named definition responsible for the
   defect. A `precision_controls` entry records an ordinary-looking construct
   the agent inspected and correctly judged intentional.
5. Save the response as `<responses-dir>/<case-id>.json`.

Validate the suite and score a complete response set with:

```bash
python3 skills/full-repo-audit/evals/marker-free/score.py validate-suite
python3 skills/full-repo-audit/evals/marker-free/score.py score \
  --responses <responses-dir>
```

Validate one response while iterating with:

```bash
python3 skills/full-repo-audit/evals/marker-free/score.py validate-response \
  --case ignored-input-config \
  --response <responses-dir>/ignored-input-config.json
```

## Adjudication

Scoring is deterministic and anchor-based. Per case, finding recall is worth
60 points, explicit recognition of the intentional precision control is worth
20 points, and avoiding unmatched findings is worth 20 points. A perfect case
therefore scores 100; the complete suite passes only when every case is
perfect. Findings aimed at an oracle precision control count as false
positives. Free-text summaries, evidence, and reasons are required for human
review but do not change anchor matching.

The scorer reports matched and missed gaps, matched and missed controls,
unmatched findings, and controls misclassified as findings. A human adjudicator
should read the cited code and requirement before changing an oracle. Accept a
symbol alias only by recording it explicitly in the oracle; do not grant credit
from prose similarity. If a repository requirement changes, update its README,
source, oracle, and the scorer self-test in the same change.

Run the evaluation infrastructure self-test with:

```bash
python3 skills/full-repo-audit/evals/marker-free/self_test.py
```

This self-test synthesizes responses from the hidden oracle to exercise schema
validation, score accounting, and failure handling. It does not run an audit
agent and therefore does not demonstrate empirical recall or precision. Only a
complete response set produced by fresh agents under the manual-run isolation
rules can provide that evidence.
