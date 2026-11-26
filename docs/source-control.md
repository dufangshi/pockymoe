# Source control hosts

Settings → Source control manages device-local Git host sign-in and writing
instructions. Rescan checks workspace remotes, GitHub/GitLab CLI accounts and configured hosts. GitHub
uses `gh api` or `GH_TOKEN` (`GH_ENTERPRISE_TOKEN` for Enterprise); GitLab uses
`glab api` or `GITLAB_TOKEN`. Forgejo/Gitea, Bitbucket and Azure DevOps accept saved
tokens (`GITEA_TOKEN`, `BITBUCKET_ACCESS_TOKEN`, `AZURE_DEVOPS_EXT_PAT` also work).
Self-hosted services are configured by hostname and host kind. HTTPS is required
except for loopback Forgejo fixtures. Saved tokens use the upstream key store's
private atomic file writer; API responses contain only `hasToken`, never tokens.
Azure can also use an existing `az` sign-in through `az rest`. Host API
authentication and Git SSH/HTTPS transport credentials are separate.

Thread tools → Linked PRs and issues shows the thread's links and watch status.
The Git panel's PR view lists linked PRs and issues, accepts manual links, browses
host items and creates PRs or issues. PR creation pushes the current branch if it
has no upstream. Detail actions read checks, comments, reviews, commits and files;
update title/body, comment, ready-for-review, auto-merge and guarded merge depend
on the host. A merge requires the head SHA the caller reviewed. GitHub, GitLab,
Forgejo and Azure receive the expected SHA in the host write itself. Bitbucket
merge is refused because its endpoint lacks an atomic head guard.

GitHub and GitLab support issue list/search/show/create/comment. Forgejo supports
basic issues. Unsupported operations return an error instead of guessing a host
capability. Linked items belong to a thread and are removed with it. Creation
links the result automatically; completed `gh pr create` and `glab mr create`
command output also supplies links. Local agent CLI credentials can use only their
own thread; shares and remote peers cannot access the owner's host sign-in.

`pockymoe pr --help`, `pockymoe issue --help` and `pockymoe guide pr` describe the
agent interface. Use `--item-url URL` to address an already linked item in another
repository. `pockymoe pr work-on NUMBER` fetches the host PR ref (including
fork heads on GitHub/GitLab/Forgejo and source branches on Bitbucket/Azure), creates a managed worktree and opens an idle
thread there. It does not change the caller's shell or harness directory.

## Watching a PR

`pockymoe pr watch NUMBER` saves a watch for a root thread. The Supervisor polls
active watches every two minutes and delivers changes through the existing
thread queue. Watches survive restart and stop on PR merge/close, thread close or
delete, explicit unwatch, eight consecutive read failures, or ten consecutive
comment-only wakes. Changes include failed checks, required checks passing, new
comments/reviews from others, conflicts and merge-queue removal. GitHub reads
required checks, inline review comments and the merge queue through GraphQL.
GitLab and Forgejo use their REST summaries; older Enterprise servers fall back to REST.
Bitbucket and Azure watch are currently unavailable. REST summaries do not provide
the same required-check and merge-queue detail as GitHub GraphQL.
One watch is active per thread. Starting another replaces the previous watch.

## Pull requests page and review

Workspaces → Pull requests (`/pull-requests`, `/devices/ID/pull-requests`) lists the
PRs of every repository this device knows: the remotes of its workspaces (fork
upstreams included) and repositories of linked PRs. The state (open, merged,
closed, all) is read from the hosts; involvement (authored, review requested),
review decision, checks, drafts and repository narrow the same list, and the
search box accepts `author:`, `label:`, `-label:`, `draft:`, `is:draft` and
`review:`. Sorts: merge readiness (approved and passing first, drafts and
failing later), updated, newest, oldest, largest, smallest. The last filters are
kept per browser; saved views are kept on the Supervisor
(`/api/source-control/pull-request-views`), so every browser of the device sees them.

An open PR shows Conversation (description, reviewers, comments, reviews, review
threads with Reply and Resolve), Code (changed files with the Git panel's diff
viewer, a file list with Viewed ticks, threads and drafts under their lines),
Commits, Checks (logs, Rerun failed jobs where the host allows it) and Guide.
"Open beside thread" opens a linked thread with `?pr=URL`, which shows the same
review in its Git dock; the dock's PR view also has a Review button per linked PR.

- **Drafts.** Selected diff lines or a whole file become draft comments, stored
  on the Supervisor (table `pull_request_drafts`) per PR and per author, so a phone
  and a desktop see the same drafts. Agents have their own drafts and never see or
  send the owner's. The review summary is a draft too. Submit sends Comment,
  Approve or Request changes with every draft in one review; only what was sent is
  cleared. A draft written on an older head says so.
- **Head guard.** The page pins the head it loaded. A poll (every minute and when
  the page shows again) notices new commits and offers "Load new commits". Submit
  and merge carry that head and are refused when the host has moved on (GitHub,
  Forgejo, Azure and GitLab approval also guard in the write itself).
- **Viewed files.** A ticked file folds away. GitHub keeps the state itself
  (`markFileAsViewed`; its dismissed state shows as Changed). Other hosts keep it
  in `pull_request_viewed` with the file's blob id (else a digest of its diff): a
  push that changes the file shows it as Changed.
- **Merging.** Methods follow the repository. "Remove agent credits" (default in
  Settings → Source control) reads the host's default message (GitHub
  `viewerMergeBodyText`, GitLab default merge/squash messages, otherwise built
  from the commits), drops `Co-authored-by` lines whose e-mail belongs to a coding
  agent (`noreply@anthropic.com`, `codex@openai.com`, Copilot, Cursor and others)
  and "Generated with …" lines, keeps human co-authors and code blocks, and shows
  the result before merging. Rebase merges keep the original commits, so credits
  stay there. Auto-merge, ready for review, convert to draft, close, reopen and
  request reviewers follow the host; GitHub also opens revert PRs.
- **Diffs after a push.** Forgejo, Bitbucket and Azure refresh a PR's diff some
  time after a push. With a local checkout the Code tab and the guide fetch the
  head branch (from the fork for fork PRs) into `refs/pockymoe/review/…`, check it
  is the head the host reports, and diff locally; the refs are removed afterwards.

### Review guides

Guide → Write guide produces an ordered tour of the change: stops in reading
order with a summary, an optional "look closely" call (tradeoff, contract,
product, risk, accessibility), code excerpts, line notes and questions; mechanical
files are grouped at the end. The writer runs in a temporary shared clone of the
local repository at the PR head (no checkout in the user's working tree, files
read-only): Codex with `--sandbox read-only`, or Claude Code limited to Read, Grep,
Glob and `git diff/log/show`, 15 minutes at most; "Saved model connection" sends
the diff in the prompt instead. The answer is checked against the real diff
(excerpts clamped into one hunk and 25 lines, notes inside excerpts, every changed
file placed once, an "Other files" stop for the rest) and stored per PR in
`pull_request_guides`; a guide for an older head is marked and can be rewritten.
Selected excerpt lines become drafts or a question for an agent ("Ask agent"
sends to a linked thread's composer or starts a new thread with the question).
Settings → Source control → Pull request review holds the writer, model and the
guide instructions, with an override per workspace. A guide needs a workspace with
the repository on this device.

### Hosts

| Host | Review | Line / file comments | Reply / resolve | Viewed | Rerun | Other |
| --- | --- | --- | --- | --- | --- | --- |
| GitHub | Comment, Approve, Request changes (one GraphQL review) | both, ranges | both | on GitHub | failed jobs of a run | revert PR, auto-merge |
| GitLab | Comment, Approve (`sha` guard), Request changes | both (range noted in the text) | both | device | pipeline retry | auto-merge |
| Forgejo / Gitea | Comment, Approve, Request changes (needs a summary) | lines; file comments go into the review text; ranges noted | reply as a one-comment review on the line; no resolve API | device | — | auto-merge |
| Bitbucket | Comment, Approve, Request changes | both | both | device | — | no merge (no atomic head guard), cannot reopen |
| Azure DevOps | Comment, Approve and Request changes as votes | both | both | device | — | files without patches unless the repository is local |

### Agents

`pockymoe pr review show|files|comment|drafts|discard|submit|reply|resolve|viewed|reviewers|state|merge|rerun|revert|guide`
gives agents the same review state and actions on their own thread's repository
or a linked PR (`pockymoe guide pr`). Agent drafts are separate from the owner's.

### HTTP

Owner-only, like every `/api/source-control/` route:

- `GET /api/source-control/pull-requests?state=open|closed|merged|all` → `{pulls, repositories, viewers, fetchedAt}`
- `POST /api/source-control/pull-requests/{action}` with `url` and the action's fields:
  `detail`, `head`, `files`, `drafts`, `draftSave`, `draftDelete`, `submit`
  (`event`, `body`, `expectedHead`), `reply`, `resolve`, `comment`, `update`, `viewed`
  (`paths`, `viewed`, `revisions`), `reviewers` (`add`, `remove`), `state`
  (`close|reopen|draft|ready`), `merge` / `autoMerge` (`method`, `expectedHead`,
  `removeAgentCredits`), `mergeMessage`, `rerun` (`token`), `revert`, `guide`,
  `guideShow`, `link` (`threadId`), `workOn` (`workspaceId`).
- `GET/PUT /api/source-control/pull-request-views`
- Settings: `review` in `/api/source-control/settings` (`removeAgentCredits`,
  `guideSource`, `guideModel`, `guideInstructions`, `guideWorkspaceInstructions`).

## Repository creation

Settings → Source control can clone a Git URL into the configured development
home. Clones run as background jobs; failures are visible. SSH failure falls back
to HTTPS. A private staging directory keeps partial clones out of the final path.
The PR panel can publish a repository with no remotes through GitHub, GitLab or
Forgejo: it creates a remote (private by default), adds origin and pushes. A push
failure leaves the created remote visible for recovery rather than deleting it.

## Worktree setup

A source checkout may contain `.pockymoe/worktree.json`:

```json
{
  "copyFiles": [".env"],
  "submodules": true,
  "sparsePatterns": ["/crates/", "/Cargo.toml"],
  "commands": ["cargo metadata --no-deps --format-version 1"]
}
```

Managed worktree creation copies files, applies sparse patterns, initializes
submodules and runs commands in order. Copy paths must stay inside the source and
destination checkouts; existing files are never overwritten. Commands have bounded
output and a two-minute deadline each. The creation response includes setup output
or the failure and retains the registered worktree for inspection. Setup output and errors are also
shown after the PR panel's Create worktree action. Repository setup commands are
executable project configuration; review them before creating a worktree. Setup in isolated environments returns an explicit unavailable result.

`pockymoe pr cleanup` suggests managed worktrees merged into the default remote
branch, or created from a PR that is now closed/merged, when no other thread is
associated with them and no thread has their path configured. Removal still goes through the existing safe
`pockymoe worktree remove ID` checks; suggestions never delete files.

## HTTP surface

All routes below require the owner; local agents use the restricted CLI bridge.

- `GET/PUT /api/source-control/settings`, `POST /api/source-control/scan`
- `POST /api/source-control/clone`, `GET /api/source-control/jobs/{id}`
- `GET/POST /api/threads/{id}/pull-requests` (links / create)
- `POST /api/threads/{id}/source-control/{action}` for PR/issue operations,
  link/unlink, scan, watch/unwatch/watchStatus, workOn, generate, publish, cleanup and removeWorktree.

PR/issue operations use `number`, optional `kind: "issue"`, and optional
`workingCopyId`. `url` selects an item already linked to this thread. Merge takes
`expectedHead` and `method: "merge" | "squash" | "rebase"`. Host response payloads
are under `data`; links and watch state use their own fields.
