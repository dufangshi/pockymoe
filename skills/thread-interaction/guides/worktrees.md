# pockymoe guide worktrees

Create and inspect Git worktrees for the thread you are already working in.
Run `pockymoe worktree --help` for the commands and each subcommand's `--help`
for its flags. Data commands return JSON. Check their exit status and receipt.

## Create and work in a checkout

Use the manager for worktree lifecycle operations rather than running
`git worktree add`, `remove`, `move` or `prune` directly. This keeps the
Supervisor's ownership, thread associations and Git panel accurate.

```sh
pockymoe worktree current
pockymoe worktree create --name fix-login --request-id fix-login-checkout
pockymoe worktree list
pockymoe worktree show WORKING_COPY_ID
pockymoe command run --worktree WORKING_COPY_ID -- git status --short
```

Replace `WORKING_COPY_ID` with the returned working-copy ID. Creation associates
the checkout with the current thread. It returns an absolute path to use as cwd
for subsequent native commands and as the base of file paths. A branch or path
alone is not the working-copy ID. `--base REF` chooses a starting revision and
`--branch BRANCH` chooses the branch; the default base is committed HEAD of the
source checkout, not its uncommitted files. Never commit or discard the user's
changes merely to create a worktree.

Repeat an interrupted creation with the same `--request-id` and arguments, then
inspect its receipt and directory before taking further action. A transport
failure is not proof that creation failed.

Creating a checkout cannot change the parent shell or running harness's cwd.
Set each tool's cwd to the returned path, or run a controlled command with
`pockymoe command run --worktree ID -- ARGV...`. That flag is mutually exclusive
with `--cwd`. It selects one command's starting directory; a command can still
change directories internally.

## Inspect or register actual locations

`worktree current` sends this CLI process's actual absolute cwd, which can differ
from the thread's configured checkout or a concurrent tool's directory. `list`
and `show` expose the thread's associated working copies. Tool cwd and file
events provide additional observations; a read in another checkout does not
prove the entire thread switched there.

```sh
pockymoe worktree adopt --path ../existing-checkout
pockymoe worktree list
```

`adopt` resolves relative paths in the CLI's current directory and registers an
existing checkout. Registration does not grant automatic deletion ownership.
The Supervisor verifies repository identity and access: a sibling worktree may
be valid even though it is outside the original workspace. Do not bypass a
refusal by manipulating Git metadata or another thread's identity.

## Change a thread's default checkout

```sh
pockymoe worktree --thread IDLE_THREAD_ID use WORKING_COPY_ID
```

`use` requires an idle thread. It releases its loaded harness and updates the
configured directory for its next session load. The receipt reports
`sessionCwdUpdated: false` and `appliesOnNextSession: true`: this does not move
the cwd of an existing shell. Native session restore can also have its own
directory behavior, so continue checking actual execution cwd. A running
agent should use explicit tool cwd instead of waiting for its own turn to end.
`--thread` defaults to `self`; names and local thread IDs use the ordinary CLI
identity and authorization rules.

## Clean up

```sh
pockymoe worktree remove WORKING_COPY_ID
```

Only remove a managed worktree when it is no longer in use and its changes have
been kept as intended. The manager checks active users and uncommitted,
untracked and ignored files;
there is no force/discard shortcut. Registering or observing somebody else's
checkout does not make it yours to delete. Keep commits and branch cleanup
separate from removing a checkout.

`thread create --worktree` remains the shortcut for starting a new delegate in
an isolated checkout. It creates the checkout before the new harness starts;
see `guide delegate`.
