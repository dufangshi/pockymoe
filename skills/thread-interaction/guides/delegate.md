# pockymoe guide delegate

Create, wait for, close and clean up delegates; lineage, roles, worktrees.

## The orchestration loop

For an existing peer, choose by what must change **in its current turn**, before
calling something a task:

- Stop, replace, reprioritize or correct its active work: use `steer` (or `direct`
  when its state is uncertain) with the concrete wasted work/harm in
  `--interrupt-reason`. Do not queue “use the existing implementation instead,”
  “switch the primary strategy,” or “stop launching work under the old grant.”
- Give it distinct work that can wait until its current turn ends: use `queue`.
  Queue waits for the **entire turn**, not the next tool result, checkpoint or
  compute batch. A long-running turn can defer that instruction for hours.
- Provide ready inputs, results, adoption notices or report-only updates: use
  `inbox`. “Already received,” “task complete” and “no action needed” are not tasks.

When another peer needs a running worker's output now, split the request: steer
the minimal unblock/correction, and queue any independent later work. A waiting
consumer should collect ready inputs through inbox in its existing turn.

Most delegation is this, and every step is one command:

```bash
pockymoe thread create --name tests --kind task --subject 'Fix the flaky auth test' \
  --text-file /tmp/tests.txt                      # 1. delegate, with a name
pockymoe thread create --name docs --worktree --kind task \
  --subject 'Document the new flag' --text-file /tmp/docs.txt
# 2. do your own independent work here, if you have any
pockymoe thread wait tests docs --timeout 600 # 3. block until both settle
pockymoe thread close tests docs              # 4. free their slots once collected
```

`thread wait` returns each delegate's state and its **closing message** inline, so you
usually need no transcript call. It returns early with `blocked: true` when a delegate
waits on an approval, or has mailed you a `--kind question` that nobody has answered
(`--in-reply-to`) or acknowledged - `waitingOn` names the message; answer it, then wait
again. `--any` returns as soon as one settles (`allSettled` tells you whether the rest
did), so you can handle results as they land. `--any` and `--wake` cannot be combined.

Choose by how long you expect to wait:

- **Short (about a minute or less), or you will act immediately:** block with
  `thread wait`. Your shell tool may return before the wait does (Codex yields about
  every 30 s); just keep waiting on the same command. Each of those returns costs you
  a model call, so a long blocking wait is not free.
- **Long, and nothing else to do:** `pockymoe thread wait tests docs --wake`, then
  end your turn saying what you are waiting for. When they all settle (or one blocks),
  one report carrying their closing messages reaches you: it joins your turn if that
  turn is still running and your harness takes steers (for example while you wait on
  a native subagent), otherwise it is queued as your next turn. It costs nothing
  while you wait. That explicitly requests an immediate continuation; delayed unread inbox reminders
  are a separate 10-minute fallback.

Run `pockymoe thread tree` any time for the whole picture: every delegate's state,
unread mail, current task and worktree.

A settled delegate finished *executing*; that is not proof the work is right. Ask
every delegate to end with evidence - files changed, commit hash on its branch, the
exact test command and its result - and check it before you merge or close.


## Identity and discovery

```bash
pockymoe thread self
pockymoe thread tree
pockymoe thread list --limit 20
pockymoe thread list --workspace WORKSPACE_ID --limit 10
pockymoe thread status THREAD_ID
pockymoe thread backends
pockymoe thread models --provider codex
pockymoe thread models --provider acp --agent grok
```

Use **Pockymoe thread IDs**, the last segment of `/devices/DEVICE_ID/threads/THREAD_ID`, not native Codex/ACP session IDs. A target may be a UUID, a full Web thread URL, or `DEVICE/THREAD_UUID`. Names of open threads in your lineage and `parent`, `root` or `self` resolve only locally. `self` identifies your caller. `list` defaults to 20 entries, capped at 100, without transcripts. `status`/`show` return lightweight metadata including `activeTurnId`, `queuedCount`, `unreadMessageCount`, `waitingForInput`, and `lastError`.

Reuse a peer when its workspace, model, and earlier work fit. Read status and only enough recent transcript to assess context. Create when separate context or another model is useful.


## Lineage: threads you create group under you

A thread you create records you as its parent and inherits your lineage root — the
thread a person actually started. `thread list` shows only those roots by default,
each with the number of agent threads beneath it, so a fan-out does not bury the
person's own conversations. With `--workspace`, it also lists the agent threads
working in that workspace for a conversation started in another one: there they
are top-level (an agent thread they started in the same workspace stays folded
under them). Pass `--all` to see every thread flat, or `--group THREAD_ID` to list
one root's descendants.

Two bounds apply, and both are refusals rather than queues:

- **Depth 3.** You may delegate, and your delegate may delegate once more. Past that,
  creation fails; delegate from the root instead of chaining deeper.
- **20 open threads per root.** A delegate that finished its turn is `idle`, not gone:
  it stays addressable and keeps its slot until you `thread close` it. Failed and
  interrupted threads release theirs. So close delegates once you have collected their
  work, and a long sequential fan-out stays unrestricted.

When either refusal arrives, **do not retry the same call** — it will fail identically.
Close what you have collected, wait for outstanding work, or reuse an idle peer. Prefer
a handful of concurrent delegates (3-5) over a large burst.

### Names, closing, roles and worktrees

- `--name reviewer` gives a delegate an address unique among the open threads of your
  lineage; any command that takes a thread id accepts it. Names match
  `[a-z][a-z0-9_-]{0,31}`.
- `pockymoe thread close NAME...` frees each slot and name. History stays readable,
  and prompting it again reopens it. Only an ancestor (or the thread itself) can close
  it, and not while it is running or has queued work. The command exits nonzero if any
  target fails; `failed` lists which and why. After a name is reused, refer to the old,
  closed thread by its UUID.
- `--role ROLE` starts from `.pockymoe/agents/ROLE.md` (workspace, then
  `~/.pockymoe/agents/`; `.remote-codex/agents/` still works). Front matter `model`, `effort`, `agent` become defaults
  you can still override; the body is prepended to the delegate's first prompt.
  `pockymoe thread roles` lists them.
- `--worktree` runs the delegate in its own git worktree, `../REPO.worktrees/NAME` on
  branch `agent/NAME` (or `--worktree-branch`), checked out from **committed** HEAD -
  uncommitted changes in your checkout are not in it, so commit what a delegate builds
  on first. If `agent/NAME` already exists from an earlier delegate, a fresh
  `agent/NAME-2` is used; pass `--worktree-branch` to continue an existing branch.
  Tell the delegate to commit on its branch; you merge it. `thread close NAME --remove-worktree` removes the checkout
  and refuses if it has uncommitted changes. Use worktrees for parallel edits that
  could touch the same files; skip them for read-only work.

Model IDs and effort options come from local discovery. Preserve explicitly requested models; do not invent an ID from a display name or silently substitute another model. Availability depends on the installed harness and working directory.


## Create and dispatch a task

```bash
pockymoe thread create --title 'Build helper' \
  --provider acp --agent grok --model MODEL_ID --reasoning-effort EFFORT \
  --kind task --subject 'Build the release artifact' \
  --text-file /tmp/build-request.txt --notify-on-complete
```

`thread create` accepts the same `--subject`/`--kind` flags as `thread send`, and the
initial prompt is a message like any other - label it. Create defaults to provider
`acp`, model `default`; it does not inherit the caller's model. Workspace and approval mode inherit from the caller unless specified. `--workspace WORKSPACE_ID` selects an **existing** workspace; provide it from an unscoped shell. `--approval-mode guarded|yolo` is supported; thread creation does not expand user authorization.

Unlike ordinary `send`, a create's initial prompt defaults to **queue**, so the new peer actually starts its assigned task. Creating without prompt leaves it idle. `--delivery inbox` explicitly makes the initial message passive. Creation and first send are sequential, not one idempotent transaction. If send fails after creation, the error includes the created thread ID: reuse it instead of creating a duplicate.

`--text` and `--text-file` are mutually exclusive. `--text-file -` reads stdin. Use a quoted heredoc or a file for multiline text so the shell cannot execute dollar expansions/backticks:

```bash
pockymoe thread send PEER_ID --delivery queue --kind task \
  --subject 'Build the assigned checkout' --text-file - <<'PROMPT'
Build the assigned checkout with the documented command.
Report command, exit code, artifact paths, and blockers.
Send the result to CALLER_THREAD_ID using default inbox delivery.
Do not publish packages as part of this build task.
PROMPT
```

Replace placeholders before sending. Provide goal, checkout, relevant files, constraints, expected artifacts, and reply destination. Do not copy your entire transcript by default. Execute external actions such as publication only within the user's authorization.


## Completion notifications

`--notify-on-complete` subscribes to the receiving **execution turn**, so it requires direct/queue/steer delivery plus a caller identity. Passive inbox messages have no execution turn and reject this flag. It is a per-send subscription, not a permanent watch of future activity.

Completion notifications always go to the caller's passive inbox. They include peer/thread IDs, terminal status (`completed`, `failed`, or `interrupted`), timestamp, a transcript command, and the delegate's closing message (truncated at 4000 characters). They do not immediately wake, steer, or queue a turn on the caller - `thread wait` and `inbox wait` are how you block on them, `--wake` how you hand off.

```bash
pockymoe thread wait PEER_ID          # usually all you need
pockymoe inbox wait --kind result
pockymoe transcript PEER_ID --limit 1 # when the closing message is not enough
```

`--notify-delivery queue` is no longer supported. Do not send direct/queue/steer messages to the parent just to report completion; keep results passive and let the parent collect them. Completion describes execution, not business success: read that turn and verify artifacts/exit codes before dependent actions. Using explicit inbox replies and automatic notifications together can produce two passive messages; avoid redundant reports.

After upgrade, still-pending legacy completion subscriptions also deliver to inbox, regardless of their stored choice. Already-enqueued input is not cancelled or moved automatically. Upgrading the runtime does not change behavior of an older Supervisor that is still running.


## Clean up your own child threads

`thread close` and `thread delete` both free a delegate's slot. Close keeps its
history readable and lets you reopen it by prompting; delete removes its Pockymoe
conversation for good. Close by default; delete when nothing about it is worth keeping.

After recording the result you need, delete a finished direct child with:

```bash
pockymoe thread delete CHILD_THREAD_ID
```

The Supervisor verifies your managed credential and the stored parent relationship.
You cannot delete yourself, a parent, sibling, another root, or a grandchild. `--from`
does not change the authenticated parent. A machine connection file alone cannot
authorize deletion; use your managed session, reconnecting it after a runtime upgrade
if its credential predates this feature.

Unused idle children may also be removed. Running/recovering children, active or
queued turns, and children that still own descendants are refused. There is no force
or recursive delete. Have each direct parent finish and clean up its own children
first. Deletion removes the child's saved Pockymoe conversation/mailbox and
releases its idle, independently owned harness process. Workspace files, branches,
worktrees, native harness history, and results already delivered to your inbox are
preserved. Save needed transcripts/artifacts before deleting; the Pockymoe
conversation cannot be restored by the CLI.
