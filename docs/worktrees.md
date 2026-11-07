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

## Git panel

The thread toolbar's Git button opens a read-only panel. It lists the configured
and associated working copies, marks their source and shows recent tool location
evidence. Selecting a copy changes this view only. The panel shows staged and
unstaged changes, selected untracked file contents, comparison from a revision's
merge base, and paginated commits. PR status and Git writes are not included.
It refreshes when opened, on thread updates and on request; it does not poll.

The endpoints are `GET /api/threads/{id}/working-context` and
`GET /api/threads/{id}/git/{status|diff|commits}`. Git reads accept a
`workingCopyId`; leaving it out selects the configured checkout. They require
the existing linked-file permission, so a conversation or workspace share does
not gain access to associated external worktrees. Older Supervisors show the
feature as unavailable until updated.

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
