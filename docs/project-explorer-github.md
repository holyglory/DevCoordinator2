# Project Explorer GitHub adapter

The daemon reads GitHub branches, selected branch commit pages and pull-request
evidence through `project::github::GithubClient`. The Console receives bounded
metadata from the project service's cache. A project open must request a live
refresh even when the cache is fresh; a refresh failure preserves previously
observed branches and reports the coverage gap. A failed or inaccessible remote
request is never evidence that a ref was removed.

## Private credential

Construct the client with `Config::secrets_dir().join("github.token")`. The owner
provisions the token outside the checkout through the private instance workflow;
there is no per-user OAuth flow. This follows the confirmed single-owner local
boundary and credential confidentiality requirements in `security-assumptions.md`
and the approved Project Explorer credential direction. The adapter does not
alter public Console authorization.

The token must be a regular mode-0600 file owned by root or the daemon's effective
UID, have one hard link and contain at most 4 KiB. Every path component is opened
without following symlinks. The adapter rejects relative paths, symlinks, special
files, unsafe permissions, concurrent file changes and malformed tokens. A
missing credential produces `MissingCredential`; it never triggers an anonymous
fallback. Each operation reads the file again for rotation, then keeps one
credential for that operation's whole HTTP session.

Credentials go only in a sensitive HTTP Authorization header. Production requests
use the fixed `https://api.github.com/` origin, refuse redirects and bypass proxy
environment variables. Repository input accepts an exact credential-free HTTPS
`github.com/owner/repository` URL or validated owner/name components. Enterprise
hosts are not yet supported; an arbitrary repository URL cannot receive the
Coordinator credential. Only the test build exposes a loopback fixture origin.

Read access requires the GitHub token's repository metadata, Contents read and
Pull requests read capabilities for the selected repositories. Remote retirement
also requires the write capability accepted by GitHub's `updateRefs` mutation;
server-side branch rules remain authoritative. Credentials and raw upstream
error text never enter returned metadata. Browser links are constructed from the
validated repository, ref, commit SHA or pull-request number, dropping upstream
userinfo, query strings and fragments. Commit author e-mail fields and full
commit bodies are not projected.

## Bounded reads and coverage

The async methods are `repository`, `list_branches`, `commits` and
`pull_requests`. Page sizes are 1–100. Cursors contain a bounded page number,
never an upstream URL. A commit request is anchored to an exact 40-character
head SHA and returns parent SHAs, a bounded subject, display author, authored
timestamp and constructed commit link. It does not download the whole graph.
Branch-filtered pull requests include exact head/base refs, original head SHA,
merge timestamp and merge commit SHA. Fork pull requests do not prove retirement
of a branch in the configured repository and are excluded from that branch's
evidence.

`GithubPage.complete` describes reaching the remote collection's end on that
page. The caller must retain all earlier pages and preserve concurrent-change
and scan-budget limits. An exactly full page without Link metadata remains
partial until the following page establishes the end. Pagination links must
retain the same origin, path and query, except for advancing one page; links are
validated and never followed directly.

Every HTTP request has a 4-second connection deadline and 12-second total
deadline; merge verification has a 60-second outer deadline. Response bodies
are capped at 2 MiB while streaming, including responses without Content-Length.
Invalid JSON, missing graph parents, malformed SHAs, duplicate rows, excessive
rows and contradictory merge metadata fail the page instead of silently
truncating it. The calling service must retain an honest partial or stale state.

Rate limits return `RateLimited { retry_after_seconds }`, using Retry-After and
the primary reset deadline. The client remembers that deadline across clones
and refuses further requests until it passes. There is no automatic retry or
unbounded wait. Other failures return fixed typed errors without upstream
messages or credential paths. A caller must distinguish `NotFound` from an
authoritative complete branch-list observation.

## Squash merge proof and retirement

`verify_integration` creates an opaque `GithubIntegrationProof` only after fresh
GitHub reads prove all of the following:

1. The repository identity matches and the feature ref is neither the current
   default nor the approved integration branch, and is not protected.
2. The current feature head equals the requested original head. The exact
   same-repository pull request is closed and merged; its feature ref and head
   match, and its base is the selected integration branch.
3. The pull request has a real merge timestamp and merge commit SHA. GitHub's
   comparison shows that merge commit is present in the current integration
   head. This works for squash merges without claiming that the original
   feature head is an ancestor of the integration branch.
4. Both exact refs still match after the comparison.

The proof is serializable evidence but is not deserializable authority. Its
freshness check expires after five minutes. Local checkout cleanup must also
perform its independent native Git and active-consumer checks.

`delete_branch` requires the lifecycle service's opaque `VerifiedCheckoutRemoval`,
binding the exact repository, checkout, branch, head and durable local cleanup
receipt. The calling service must additionally prove that all other local
copies and active consumers permit remote retirement. One checkout receipt
does not prove that every copy is gone. Browser flags, receipt-shaped JSON,
and a claimed success string cannot authorize this helper.

Immediately before deletion the adapter repeats the complete remote proof. It
uses GraphQL `updateRefs`, atomically comparing the feature `beforeOid` with the
expected SHA and setting `afterOid` to the all-zero deletion value. The same
atomic request contains an unchanged integration ref whose before and after
values equal the verified integration head. A concurrent feature push or target
force-push rejects the request; no unconditional REST DELETE fallback exists.

After mutation, the adapter rechecks repository identity and integration-ref
readability, then observes the exact feature ref absent. Only then does it
return a `GithubDeleteReceipt`, which the service must retain in its permanent
audit history. An ambiguous transport, mutation or postcondition result is
`DeletionUnverified`, which is deliberately not automatically retryable. A
refresh must resolve the remote state before another decision.

## Protocol references and test boundary

REST requests pin `X-GitHub-Api-Version: 2026-03-10`, a currently supported
[GitHub REST version](https://docs.github.com/en/rest/about-the-rest-api/api-versions).
The adapter uses the documented [branch listing](https://docs.github.com/en/rest/branches/branches#list-branches),
[commit listing and comparison](https://docs.github.com/en/rest/commits/commits),
[pull-request merge SHA semantics](https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request),
[rate-limit headers](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api),
and [atomic updateRefs before/after semantics](https://docs.github.com/en/graphql/reference/git#updaterefs).

The focused `project::github::tests` suite uses real HTTP requests against
disposable loopback fixtures. It checks pagination, graph parents, response
bounds, private credentials, missing configuration, HTTP authentication errors,
rate limits, malformed/truncated metadata, exact squash proof, atomic mutation
parameters, race rejection and post-delete uncertainty. These fixtures prove
the adapter boundary only. Live private-repository access, persisted cache
refresh, authorized Console use, complete local cleanup and retained retirement
receipts require their own integration evidence before product completion.
