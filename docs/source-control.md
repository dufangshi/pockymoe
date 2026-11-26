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
