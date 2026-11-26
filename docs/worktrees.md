# Worktree management

Pockymoe records Git working copies separately from the workspace where a
thread was created. A thread can associate with several worktrees, including
linked worktrees outside the workspace directory. Git state belongs to each
working copy; branches and the shared Git object database do not identify its
uncommitted files or index.

## CLI

```sh
pockymoe worktree current
pockymoe worktree create --name fix-login --base HEAD --request-id fix-login-checkout
pockymoe worktree list
pockymoe worktree show WORKING_COPY_ID
pockymoe worktree adopt --path ../existing-checkout
pockymoe command run --worktree WORKING_COPY_ID -- git status --short
pockymoe worktree --thread IDLE_THREAD_ID use WORKING_COPY_ID
pockymoe worktree remove WORKING_COPY_ID
```

`--thread` defaults to `self` and accepts local thread IDs or names. The
authenticated thread credential determines the caller; changing `--from`
does not grant another thread's rights. These operations use the local
Supervisor's CLI transport. `pockymoe guide worktrees` contains the agent
workflow, and each command's `--help` lists its flags.

Creation records an association and returns the working-copy ID and absolute
path. It uses committed HEAD of the source checkout unless `--base` specifies
another revision. It does not copy uncommitted files or change a running
shell's cwd. Use the returned path for native tools, or pass its ID to
`command run --worktree`; `--worktree` and `--cwd` are mutually exclusive.
Creation request IDs allow a retry to refer to the original intent.

`current` submits the CLI process's absolute cwd. `adopt` resolves a relative
path in that process before sending it to the Supervisor. The server verifies
Git identity and access. Registering an existing checkout or observing it in a
tool event does not acquire deletion ownership.

`use` requires an idle thread, releases its loaded harness, and changes the
configured checkout for its next session load. Its response states
`sessionCwdUpdated: false` and `appliesOnNextSession: true`; an existing parent
shell is unaffected. Native session restore can retain directory state, so
actual execution observations remain necessary after loading. Running agents
can use explicit tool cwd immediately without calling `use`.

Removal is explicit and limited to managed working copies. It checks current
users and uncommitted, untracked and ignored files; there is no force/discard flag. Removing a checkout
and deleting its branch are separate operations.
The database upgrade preserves the original ownership of delegate worktrees
created by older Supervisors; selecting or adopting a checkout later does not
grant that ownership.

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
**Staged** with their file counts, **Turn** (checkpoint changes for a selected turn), **Compare** (from a revision's merge base;
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
copy of the full ID) and subject, with Back to the commit list. Owners can stage, unstage or discard a file or a complete hunk. Discard always
asks for confirmation; writes during a running turn ask once while that turn is
running. The Staged view has a commit message box, one-shot message generation,
amend, and Commit; with an empty index it explicitly stages all files before
committing. Branch selection, creation, fetch, fast-forward pull, push and stash
controls sit beside the location. Shares retain read-only inspection.

Turn view offers before/after restore with a preview list, confirmation and Undo.
A completed turn's file-count chip opens this view. Edit from here under a user
message rewinds the conversation, optionally restoring files, then returns the
prompt to the composer. External effects remain. A restore refusal disables the
file option and explains why.

Diff preferences (unified/split, ignore whitespace, and default file folding) are
remembered in this browser. Split applies at dock widths of at least 720px.
Find in diff (Ctrl/Cmd+F within the patch) opens matching files; Enter and
Shift+Enter move between matches. Hunk writes are hidden for whitespace-filtered
or truncated diffs. Select lines on one side of a diff and choose Comment to
collect file review chips in the normal composer; several comments send together
using the existing `<pockymoe-quotes>` format with path, line range and comment.
The panel provides an optional PR view slot for the source-control integration.
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

## Location observations

The thread's configured checkout and a tool's actual operation location are
separate facts. The runtime records structured command cwd and file locations
when the harness reports them. It resolves Git roots and common Git directories
to identify linked worktrees, rather than assuming that a workspace path
prefix contains every valid checkout. A read in another repository does not
silently move the whole conversation there. Concurrent tools and subagents can
operate at different locations.

Tool evidence is stored in history before a bounded background worker probes
Git. Context can therefore lag the live tool event; overload may skip a probe.
Discovery of an additional working copy emits a thread update.

These observations are evidence, not process isolation. A command's starting
cwd does not describe every directory it accesses through internal `cd`,
`git -C`, child scripts, SSH or container execution. Agent instructions require
worktree lifecycle operations through the manager; arbitrary filesystem and
shell access can still bypass that convention.

## Service boundaries

CLI, Web and harness integrations use the Rust worktree service. The manager
tracks working-copy identity, thread association and management ownership;
Git reads use the selected working copy. File and Git access retain existing
authorization checks. Observing an external path does not expand a thread or
workspace share to include it.

`thread create --worktree` remains available for a new delegate. It creates the
checkout before the new session starts; see [thread interaction](thread-interaction.md).
