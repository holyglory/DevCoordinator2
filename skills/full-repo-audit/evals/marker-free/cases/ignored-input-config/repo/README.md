# Candidate selection service

## Requirements

- `select_candidates(records, policy)` returns record IDs whose score is at
  least `policy["minimum_score"]`, preserving input order.
- A missing or non-numeric minimum score is rejected rather than guessed.
- Export envelopes always use the protocol identifier
  `candidate-export/v1`. That identifier is deliberately fixed and must not be
  configurable per request.
