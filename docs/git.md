# Git

Git state is scoped to a thread's selected working copy; `workingCopyId` selects
an associated checkout and omission selects the configured one.

## Git panel

The Git entry on the workbench rail (on a phone, the top bar or the dock's More
sheet) docks a Git inspector at the right of the conversation, which
stays usable beside it; Files and the isolation inspector share that dock. It
follows the open thread and shows that thread's title.

One line says where the view is: the working copy (a switch listing each copy's
branch, directory and whether it is the primary one, when there are several;
otherwise the branch), the short HEAD ID with a copy of the full ID, a
**Details** toggle and refresh. Details shows the copy's absolute path, whether
it is the primary copy and how it was found (a configured checkout is not
labelled), the path and time of a recent operation elsewhere, and that
selecting a copy changes this view only. When the latest tool location is in
another working copy, one line names that copy and the time, with **View**
(`View that copy`) to switch to it; while that copy is shown the line says the
latest operation was here.

Below it one row picks the view: **Unstaged** (including untracked) and
**Staged** with their file counts, **Compare** (from a revision's merge base;
a base field and Compare button appear) and **Commits**. The file list starts
with the file count and the lines added and removed. Each row
is one line: change letter, name, directory (shortened from its start) and the
file's counts (`+3`, not `+3 −0`), or "Binary". A clean tree, an empty index or
a comparison without differences says so in one line. In the default dock
width the list (at most 40% of the area, scrolling on its own when longer) sits
above the scope's combined diff in one scroll, so a short list leaves the rest
to the diff. Selecting a file, in the list or by its heading in the combined
diff, shows that file alone, with Back, the path, previous and next file and
Copy patch; Back returns to its row. A dock dragged wider (720px or more) shows
the list beside the diff, which starts with all files; All diffs returns to
them there.

The diff is split per file with added and removed lines colored (tint plus an
edge bar). Git's header lines become words in the file's heading and a note: a
new or deleted file, a binary file (whose contents are not shown as text), an
empty file and a rename (`Renamed from …`). Untracked files appear in the
whole-scope diff as new files within the same output limit, each under its own
name. Commits are paginated (the pager shows only when there is more than one
page); each row shows the subject, short ID, author and date. Selecting a commit
shows what it changed against its first parent, headed by its short ID (with a
copy of the full ID) and subject, with Back to the commit list. Git writes use the owner-only service described below.
It refreshes when opened, on thread updates and on request; it does not poll.

The endpoints are `GET /api/threads/{id}/working-context` and
`GET /api/threads/{id}/git/{status|diff|commits}`. `diff` takes `area`
(`worktree`, `index`, `branch` with `base`, or `commit` with `base` set to the
commit). Git reads accept a
`workingCopyId`; leaving it out selects the configured checkout. They require
the existing linked-file permission, so a conversation or workspace share does
not gain access to associated external worktrees. Older Supervisors show the
feature as unavailable until updated.

Each Git command, on the host or in an environment worker, has fifteen seconds
in total, including the closing of its output pipes, and keeps at most 512 KiB of
output. A command whose descendant keeps a pipe open after Git exits fails at
that deadline; the command is stopped rather than awaited.


## Turn checkpoints and restore

The runtime captures a turn's start before sending its prompt and its end when
the harness settles, including failed and interrupted turns. Snapshots copy the
index to a disposable index, add tracked and untracked changes, write a tree and
create hidden refs under `refs/pockymoe/checkpoints/THREAD/TURN/COPY/PHASE`.
Ignored files are excluded. The user's index, HEAD, branch and stash stay intact.
An unavailable checkpoint records its reason without failing the turn. The newest
200 checkpointed turns per thread are retained; deleting a thread removes its refs.

`GET /api/threads/{id}/checkpoints` lists availability and file/line counts.
`GET .../git/diff?area=turn&turnId=...` compares start and end. When HEAD moves,
dirty files at either boundary and locally authored commits count; clean imported
merge changes do not. Merge conflict resolutions use Git's remerge diff.
Every diff area accepts `ignoreWhitespace=1`.

`POST .../checkpoints/preview` accepts `{turnId, phase: "start" | "end",
workingCopyId?}` and returns a file list plus a patch. `restore` accepts the same
body, captures a safety snapshot first, then restores tracked and untracked
nonignored files. It returns `undoId`; `undo` accepts `{undoId}`. Undo itself
creates another safety snapshot. Restoring never changes the user's index.
Restores refuse an active thread or another running/loaded thread using the copy.

`POST .../rewind` accepts `{turnId, restoreFiles, dropQueued?}`. It removes the
selected user turn and later journal turns, returning the prompt and attachment
paths to the composer. Queued prompts require explicit `dropQueued: true`.
Rewound turns retain spent usage in device and automation accounting. Automation
receipts, activity notes and fork lineage survive; conversation search drops the
removed turns. Restore writes only changed paths, preserving other file mtimes.
Checkpoint file/line totals are stored at capture for inexpensive listing.
History uses native rollback when supported, then a session fork; an unsupported
backend returns `unsupported_rewind`. With files, the start checkpoint is restored
and `undoId` is returned. External effects such as pushes, messages and delegates
are retained.

## Git writes and generated messages

`POST .../git/{action}` supports stage, unstage and discard (`paths` or `patch`),
commit (`message`, `amend`, `all`), branchCreate and checkout (`name`), fetch,
pull (`mode`: `ffOnly`, `rebase`, `merge`), push (`setUpstream`, `forceWithLease`),
stashPush (`message?`) and stashPop (`index?`). Writes return `{status}` and emit
`ThreadUpdated` with reason `gitChanged`. A running thread requires
`confirmRunning: true`. No force push is inferred. Network commands disable
interactive credential prompts and use a 120-second deadline; local commands
use 15 seconds and cap output at 512 KiB. Hunk writes require current patch context.

Status includes upstream, ahead/behind, conflicted paths, the current operation
and stash count. `GET .../git/branches` lists local and remote branches;
`GET .../git/stashes` lists stash entries for the owner.

`POST .../git/commitMessage` returns `{message,status}` from a one-shot generator.
It uses staged changes, or a disposable index when `all: true` and nothing is
staged, plus recent subjects as conventions. It never prompts the thread's session.
`GET`/`POST .../git/textSettings` reads/saves device settings: `source` (`auto`,
`upstream`, `codex`, `claude`), `connectionId`, `model`, and `instructions`.
Generation requests may override those fields. Automatic selection uses a saved
connection, then an installed one-shot harness. Generation has a 60-second deadline.

Writes, checkpoints, rewind and generation settings require owner access.
Conversation, workspace and device shares cannot invoke these operations.
Existing Git reads retain linked-file access rules.

## CLI

```sh
pockymoe checkpoint list
pockymoe checkpoint diff TURN_ID
pockymoe checkpoint restore TURN_ID --phase start --preview
pockymoe checkpoint restore TURN_ID --phase end
pockymoe checkpoint undo UNDO_ID
pockymoe thread rewind self --turn-id TURN_ID --restore-files
```

Checkpoint commands accept `--thread` and `--working-copy-id`. CLI operations bind
to the authenticated local thread, like worktree management; a peer credential
cannot acquire another thread's file-write rights.
