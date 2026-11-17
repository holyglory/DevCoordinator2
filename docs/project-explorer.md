# Projects, branches and local copies

A project represents a product. It can contain several source repositories,
independent clones of those sources, linked worktrees, immutable deployment
generations and build output. A repository name or Git remote URL alone does
not establish project ownership. An administrator assigns that ownership with
`project.configure`; `project.context` supplies the same explicit membership to
other Console pages.

The Explorer opens saved inventory immediately and requests a fresh local and
GitHub observation. A failed refresh leaves the last successful observation
visible with its age and failure. Unknown space and remote preservation remain
unknown rather than appearing as zero bytes or a successful backup.

## Using the Console

Choose **Projects**, then a project. **Copies** connects each branch to its local
copies. **Branches** focuses on Git history; **Show commits** loads actual commit
parents on demand. **Overview** shows source ownership and configured folders.
**Storage** separates source, build and shared Git space. **History** retains
metadata changes and lifecycle events after local paths have been removed.

Select a branch or copy to inspect its intent, outcome, owner, next action,
linked outcome and lifecycle. Operators can edit these details within their
existing repository access. Conflicting edits preserve the draft instead of
silently overwriting another operator. Absolute paths are administrator-only;
other users see paths relative to a configured project folder.

**Review removal** inspects the exact selected copies before offering an action.
It shows local changes, ignored and untracked files, stashes, recoverable Git
history, remote preservation,
active uses, protections and estimated reclaimed space. An incomplete inspection
does not authorize deletion. Proven-safe removal needs no additional chat
approval. Possible loss requires an administrator's explicit approval of the
listed effect and observed content; a changed target invalidates that approval.
Hard protections cannot be overridden through data-loss approval.

Inspection is a durable background job. Its progress reports completed copies,
entries and bytes inspected; cancelling a review does not remove files. Reloading
the Console resumes the pending review. Approval binds the completed plan, and
the removal worker rechecks the exact content before deleting anything.
An otherwise clean, pushed checkout can still contain unpublished commits in
its recovery history. Those commits require the same explicit loss approval.
For linked worktrees, the review distinguishes their private history from the
shared Git history that remains available in other copies.

Recorded build outputs outside the selected checkout remain in place. The
review lists their retained count and measured size; removing a source copy
does not silently remove its external outputs. Shared outputs inside a removal
target prevent removal until their ownership is resolved.

## Folder roles

Each project has configurable source and build roots. The agreed organization,
relative to those roots, is:

```text
<source-root>/<project>/
  repos/<repo>/primary/
  repos/<repo>/worktrees/<id>-<intent>/
  deployments/<deployment>/<generation>/<repo>/

<build-root>/<project>/
  builds/
  artifacts/
  scratch/
```

The primary checkout is retained and may contain valuable unfinished local
edits. Feature work belongs in separate worktrees. Current deployment sources
and the verified rollback generation are retained while they are needed.
Temporary copies have explicit ownership and a removal path; naming a directory
`scratch` does not by itself make its contents disposable.

## Branch lifecycle

| State | Meaning |
| --- | --- |
| Needs review | Discovered branch without a confirmed working intent. |
| Planned / Active / Paused / Blocked | Recorded working status, owner and next action. |
| Ready | Implementation is ready for its required review and integration. |
| Merged | Integration is supported by a recorded GitHub merge proof. |
| Cleanup pending | Local copies or references still need verified removal. |
| Retired | Required cleanup and remote reference retirement have receipts. |
| Abandoned | Work is intentionally discontinued; this alone does not authorize data loss. |

Create a feature branch with a clear intent, owner, outcome link and next action.
Keep that record current while work continues. Integrate new feature work through
a squash-merged pull request. Preserve the original feature head, pull request,
integration branch and resulting merge commit as evidence: the original feature
commit need not be an ancestor after a squash merge.

After integration, remove disposable local copies and their local feature refs,
then retire the GitHub feature ref. Remote deletion compares the observed head
atomically, so it cannot discard commits pushed after the review. Current
deployments, primary copies and active consumers remain protected. Deleting a
checkout does not complete its unfinished development outcome. Lifecycle history
remains in Coordinator; cleanup does not create whole-checkout archives or
remote archive tags.

## Local migration and recovery

The Console edits metadata and manages removal; it is not a bulk directory mover.
Trusted local migration currently supports inactive checkouts renamed within
one filesystem. Inventory ownership and active consumers first. Prepare the exact old/new location with
`project.checkout.relocate.prepare`, perform the supported filesystem move,
verify the resulting content and Git identity, then commit the relocation.
Use the returned fingerprint for verify, commit or cancellation. A path alias is
history, never permission to remove a replacement directory at an old path.

Cross-filesystem copies and deployment-bound checkouts, including stopped applied
or rollback generations, remain unavailable. Their runtime recreation and
content-pinned copy recovery require separate verified implementations; this
release never removes an original source after a copy. Preserve those locations
and their pending records for recovery. Failed migration keeps the pending record and exact
paths available for recovery. Registry, project and storage identities must
remain stable through relocation; directory reuse must never inherit an old
copy's cleanup authorization.

## Authority and credentials

Project ownership, branch descriptions, lifecycle, jobs and receipts live in
Coordinator's database. Agents should use these operations rather than maintain
a separate spreadsheet or Markdown completion ledger. Existing deployment
access grants remain the authorization boundary: viewers inspect, operators
edit and request proven-safe cleanup, and administrators approve potential loss
and retire remote refs.

GitHub uses a private Coordinator-managed credential. The Console never receives
it and does not require users to connect a personal OAuth account. See
[GitHub adapter](project-explorer-github.md) for credential placement, permissions,
freshness and squash-merge proof requirements.
