# Thread interaction

The `pockymoe` CLI can create peer threads, send prompts, inspect runtime state, and progressively read stored conversations. These operations use the Supervisor's existing threads, prompt queue, turns and history items. No database tables or separate Task/Message lifecycle are added.

## Commands

```sh
pockymoe skill
pockymoe thread self
pockymoe thread list --workspace WORKSPACE_ID
pockymoe thread status THREAD_ID
pockymoe thread delete CHILD_THREAD_ID
pockymoe thread models --provider acp --agent grok
pockymoe thread create --provider acp --agent grok \
  --model grok-4.6 --reasoning-effort xhigh --title helper
pockymoe thread send THREAD_ID --delivery queue --kind task --text-file task.txt --notify-on-complete
pockymoe transcript THREAD_ID
pockymoe transcript THREAD_ID --limit 5 --before-turn TURN_ID
pockymoe transcript THREAD_ID --turn TURN_ID
pockymoe transcript THREAD_ID --turn TURN_ID --view overview
pockymoe transcript THREAD_ID --turn TURN_ID --item ITEM_ID
pockymoe transcript THREAD_ID --turn TURN_ID --item ITEM_ID --raw
```

`thread create` also accepts `--text` / `--text-file` for an initial prompt. Without a managed caller, specify `--workspace`. Model IDs and reasoning levels must be advertised by the target harness. Creation inherits the caller's workspace and approval mode unless explicitly overridden. It does not copy the caller's history.

`thread list` lists the threads a person started (lineage roots), each with a count of its agent threads; `--all` lists every thread and `--group ROOT_ID` one root's agent threads. With `--workspace`, it also lists the agent threads that work in that workspace for a conversation started in another one, because there they are top-level. An agent thread folds into its topmost ancestor in its own workspace, so one that works where its root (or an earlier ancestor) works is not listed again. `totalCount` counts the same rows.

Ordinary `send` stores passive inbox mail without immediately starting or interrupting a turn. Explicit `queue --kind task` assigns executable work: idle recipients start automatically and busy recipients process it after current work. Urgent peer corrections/unblock requests may use direct/steer with an interrupt reason. The receiver sees the sender's Pockymoe thread ID and can use the same CLI to reply. `--request-id KEY` makes retries of the same send return the original receipt; reusing a key with different content is rejected. A send receipt does not mean execution has completed.

`--notify-on-complete` is optional per executable send. It requires a caller identity. When that receiving turn ends, the Supervisor stores a passive inbox result with the peer/turn IDs and completed, failed or interrupted status. It does not immediately wake the caller or queue a turn. Multiple opted-in messages steered into one turn keep independent subscriptions; cancelling an unconsumed queued prompt removes its subscription.

An agent can explicitly mail usable intermediate batches independently of the automatic turn-end option. Avoid duplicate completion reports, reflexive acknowledgement replies and notifying peers that do not depend on the change. See the bundled skill for handoff and waiting rules.

## Restricted child cleanup

`thread delete ID` requires a managed, identity-bound thread credential and removes
only that caller's direct child. Changing `--from` cannot grant deletion rights.
Local machine credentials continue to support discovery/messaging, but cannot delete
through `/api/cli`. This is a CLI authorization rule, not filesystem isolation of
agents sharing a trusted user account or a restriction on the person's Web API.

Finished or unused idle children can be deleted. Running/recovering children,
nonterminal turns, any queued input, and children with descendants are refused.
Checks and deletion serialize with admission/settlement and commit atomically;
new queued input arriving during harness release is rechecked and retained. There
is no recursive/force deletion. A child's own independently loaded process is
released; shared harness processes are refused to preserve other sessions.

Deleting a helper removes its Pockymoe turns/history/mailbox, revokes its CLI
credential and updates parent/root group counts. Terminal completion results are
delivered to the parent inbox before history removal. Existing result messages,
workspace files, worktrees, branches and native harness logs remain intact.

## Progressive reads

The default is the **latest 3 turns**, oldest first within the page, containing user input and **all** stored agent-facing text, including progress and final replies. This shares the Web's conversation query and storage, but does not apply the Web summary's final-reply-only folding. Status queries contain no conversation history.

`--turn` expands a paged item directory; `--item` reads its text/detail; `--raw` reads chunks of the exact stored JSON, including provider extensions. References lead to the existing attachment interfaces. Unknown or missing timestamps remain null. No unstored provider logs are synthesized.

Every response includes discovery/continuation commands when more content exists. The overview bounds turn count, item count and text preview size, with roughly 16,000 preview characters across a page; individual detailed reads return up to 8,192 Unicode characters. `truncated` is explicit. Follow `next`, `detail`, `expand`, `nextOffset` or `nextTextOffset` to continue. Text offsets count Unicode characters, not bytes. Active records are mutable; `observedAt`, `updatedAt` and turn status distinguish live reads from completed history. Re-read a growing item to fetch its latest tail.

## Local connection and identity

A serving Supervisor creates a private connection file next to its SQLite database, replacing the `.sqlite` suffix with `.cli.json`. Unix creation permissions are `0600`. A local shell can use:

```sh
pockymoe --cli-config /path/to/supervisor.cli.json thread list
pockymoe --cli-config /path/to/supervisor.cli.json --from THREAD_ID thread send OTHER_ID --text hello
```

Managed ACP processes receive `POCKYMOE_URL`, `POCKYMOE_TOKEN` and `POCKYMOE_THREAD_ID`, plus a PATH whose first entry provides `pockymoe` for the running binary. Release binaries carry a platform suffix, so the Supervisor links `pockymoe` in `<database>.cli-bin/`; otherwise an older global install could shadow newer commands. The same context reaches client-owned ACP terminal commands. Creation and session loading bind identity to the Pockymoe thread. A fork that shares a parent process is loaded independently when it needs a different CLI identity. The first user prompt in a loaded session includes a brief discovery hint for `pockymoe thread self`, `pockymoe skill` and passive inboxes. Later prompts are sent without that repeated prefix. Process reloads, identity changes and explicit context compaction rearm the hint; hidden control prompts never receive it. Automatic harness compaction relies on the harness preserving the summary, while the identity and CLI commands remain available through the process environment.

CLI connections use a loopback HTTP URL. Managed processes receive an opaque thread-bound bearer credential; the local connection file holds a machine credential for discovery and messaging. `POST /api/cli` rejects missing/invalid credentials even in local mode and rejects trusted Relay-forwarded requests. Child deletion verifies the authenticated parent. Managed peer sends bind attribution to the credential: omitting `fromThreadId` cannot bypass delivery checks and a conflicting `--from` is rejected. Machine credentials still support explicit attribution. Shared filesystem access is not a per-agent isolation boundary. The owner can still close a device or workspace to agents outside it ([agent isolation](agent-isolation.md)). Do not publish or print the connection file/token. Full thread URLs use `info.relayDeviceId` (with legacy `deviceId` compatibility) to select local handling or another device.

## Other devices

Cross-device operations stay on the local CLI connection and use end-to-end encrypted
relay requests between devices of the same owner. Access defaults to off on each
device. `device list` works before opting in and reports this device's `peerAccess`.
`device access` views it; `device access on|off` and `device trust DEVICE
--reset` require a local machine credential. Trust reset follows independent
verification of the peer's `relay-fingerprint`. A device shared with other
people who can control its agents cannot reach your other devices: its requests
fail with `peer_source_shared`, and `device list` marks it `sharedControl: true`
([agent isolation](agent-isolation.md#devices-shared-with-other-people)).

```sh
pockymoe device list
pockymoe device workspaces DEVICE
pockymoe thread list --device DEVICE
pockymoe thread backends --device DEVICE
pockymoe thread models --device DEVICE --workspace WORKSPACE_ID
pockymoe thread create --device DEVICE --workspace WORKSPACE_ID --title helper
pockymoe thread show DEVICE/THREAD_UUID
pockymoe transcript DEVICE/THREAD_UUID --limit 1
pockymoe thread send DEVICE/THREAD_UUID --text 'Review this' --attach ./file
pockymoe fs ls DEVICE --workspace WORKSPACE_ID [PATH]
pockymoe fs get DEVICE --workspace WORKSPACE_ID PATH --out ./download
pockymoe outbox
```

`DEVICE` is a relay ID or unique case-insensitive name. Qualified targets and Web
URLs require a thread UUID; lineage names and self/parent/root resolve only locally.
Remote creation requires a target workspace and does not inherit local lineage or
approval mode. Remote send supports all four delivery modes; completion results
return to the sender's local passive inbox. Incoming cross-device mail includes
`replyTo` for replies. Wait/wake/tree/task/close/delete/inbox remain local operations.

Up to 20 attachments are staged locally, uploaded to the target thread's incoming
directory, and listed in both the message and the receipt's `attachments`
(directories become zip files). `fs` provides
workspace-scoped read-only listing/downloads and verifies downloaded hashes.
Without `--out`, downloads go under the caller's thread `.temp` directory or the
local workspace's `.temp/downloads/` when no caller exists.

Only inbox/queue sends with retryable relay/offline/timeout failures enter the local
durable outbox. Its worker scans every five seconds, retries after 5 s, 15 s, 60 s,
5 min, then every 15 min, and expires records after seven days. Success removes the
record and staged files; expiry or a permanent error mails a passive status failure
to the local sender. See [the cross-device contract](cross-device-peer.zh.md).

The JSON facade has `operation` values `info`, `skill`, `list`, `show`, `status`, `backends`, `models`, `create`, `send`, `transcript`, the inbox operations, and the coordination operations described below (`resolve`, `tree`, `wait`, `wake`, `close`, `roles`, `inboxWait`, `task*`). It delegates to the existing runtime service; it does not connect directly to a provider app-server. Request DTOs use camelCase in `crates/protocol/src/interaction.rs`.

Optional notification subscriptions and explicit retry receipts use the existing KV table. Retry records store a content hash rather than duplicate prompt text. Subscriptions bind to the consumed turn in the queue-consumption transaction; turn completion and notification enqueue commit together. A lightweight worker resumes pending continuations and checks terminal subscriptions. Harness setup failures leave the accepted prompt queued, expose `lastError` and back off before retrying. An in-progress external operation interrupted by a process crash is not automatically re-executed by this feature.

## Validation

A Docker end-to-end check lives in [e2e/thread-interaction](../e2e/thread-interaction/README.md). The [skill](../skills/thread-interaction/SKILL.md) teaches discovery, handoff, waiting, creation and progressive reads. `pockymoe skill` fetches the running Supervisor's embedded guide, falling back to the CLI's copy if offline or talking to an older Supervisor.

## 0.12.32 delivery and inbox revision

This section supersedes the earlier default-delivery examples above. Ordinary `thread send` defaults to `--delivery inbox`; the create command's initial prompt defaults to `queue`. Explicit `queue` submits execution and explicit `steer` requires a running capable backend. Steering failures after durable acceptance return a held receipt; held input is excluded from background draining.

`inbox` / `inbox list` returns bounded unread previews; `inbox read ID` expands one message; `inbox ack ID...` marks handled messages while retaining history. `--thread ID` selects a mailbox. Listing and reading never trigger turns or mark messages processed. `status` includes `unreadMessageCount`. Mail uses namespaced records in the existing KV store, with no schema migration or new tables.

Since 0.12.48, completion subscriptions on executable messages always deliver to the passive inbox; `--notify-delivery queue` is rejected before acceptance. Still-pending legacy subscriptions also deliver to inbox, never waking or queuing the caller. Already-enqueued input is preserved. Parents actively inspect inbox/status/transcript at checkpoints. Passive mail rejects completion subscriptions because there is no corresponding receiving turn.

## Direct delivery (0.12.33)

`thread send ID --delivery direct --text TEXT` requests immediate handling. The server resolves idle to a durable continuation and running to steering within the acceptance transaction, after request-ID deduplication. Other states are rejected. The receipt includes `requestedDelivery` and the actual `delivery` (`queued`, `steered`, or `held`); a queued receipt is not proof that execution has started. An idle-route continuation keeps its original route if other work starts first.

Unsupported active steering is rejected without enqueueing. Accepted steering targets that specific turn; failures or a replaced/finished turn remain held, never silently becoming a continuation or steering a replacement turn. Retries reuse the original route. Existing unpinned steering from older versions retains its previous behavior.

Inbox remains passive by default. Peer direct/steer is reserved for urgent correction or unblocking, with `--kind task|question --interrupt-reason REASON`; use `queue --kind task` for assignments. Completion notification delivery is always inbox. Claude `CronCreate` / `/loop` can independently wake its native session while the harness process remains alive; that is not an automatic completion callback. See [scheduled history recovery](claude-scheduled-history.md) for visibility and persistence limits.

## Delayed unread inbox reminders

New inbox messages carry `readAt`, `remindAfter` (creation + 10 minutes) and
`remindedAt`. Every 30 seconds, the interaction worker checks for overdue,
unread, unacknowledged, non-superseded mail. If the recipient is open, has no
active/recovering turn and no queued or held input, it queues one normal durable
continuation summarizing the waiting subjects. The reminder and subsequent work
use the existing turn/history recording path.

A batch includes all currently unread, unreminded messages in that mailbox.
Marking the batch reminded and enqueueing the continuation are one transaction;
restarts cannot duplicate it. Further batches are spaced at least 10 minutes
apart. The reminder never acknowledges mail or reopens a closed thread.
Historical messages without `remindAfter` are not enrolled retroactively.

`inbox read` and inline messages returned by `inbox wait` record `readAt` and
cancel their reminder eligibility. Listing bounded previews does not. Reading
still does not acknowledge or remove messages: counts and the default list retain
unhandled messages until explicit `ack`. Use blocking wait or an explicit delegate
wake for required handoffs; the delayed reminder is a fallback, not a replacement.

## Peer communication policy and inbox snapshots

The runtime enforces peer delivery rules at acceptance, including calls from older
CLIs: queue requires `kind=task`; direct/steer requires `kind=task|question` and a
nonempty `interruptReason` (CLI `--interrupt-reason`, up to 500 characters). Rejected
messages are not stored or dispatched, and the error names the required delivery.
There is no silent downgrade. Human Web prompts and unattributed machine sends retain
their execution routes. Accepted legacy request-ID retries return their saved receipt;
omitted new fields preserve the pre-policy fingerprint.

`topicKey` (CLI `--topic-key`, up to 120 characters) opts a **full inbox status
snapshot** into replacing older active status from the same sender to the same
recipient on that topic. Results, questions, tasks and correlated replies cannot
coalesce. Replaced records retain their text/ack state and identify `supersededBy`;
read/list --all exposes them, while unread counts, notices, list and wait omit them.
The receipt reports `supersededMessageCount`. Publish snapshots in order, using stable
request IDs for retries; do not use this for patches or safety corrections.

Inbox list/wait accepts `fromThreadIds` and `kinds` (repeatable `--from-thread` and
`--kind` on the CLI). Filtering and `--new` exclusions happen before pagination/limits,
so a backlog cannot hide a matching result. The start-of-turn notice prioritizes
questions and tasks, then results, then status. Receive at natural checkpoints;
when blocked on results, include questions in the wait to avoid mutual deadlock.

## Coordination (0.12.53)

Runtime migration 10 (`agent_coordination`) adds `agent_name`, `agent_role`, `worktree_path` and `closed_at` to `threads`, plus the `agent_tasks` / `agent_task_deps` tables. Code: `crates/runtime/src/interaction/{agents,tasks}.rs`.

- **Names.** `thread create --name N` stores an address unique among the open threads of one lineage (partial unique index on `root_thread_id, agent_name` where not closed). The CLI resolves a non-UUID target, and `self`/`parent`/`root`, through the `resolve` operation from the caller's lineage.
- **Close.** `thread close` sets `closed_at`. The open-thread cap now counts `closed_at IS NULL` threads that are not failed or interrupted: a delegate whose turn finished is `idle` and keeps its slot until closed (previously nothing ever set a thread `completed`, so finished delegates held slots forever). Starting a turn clears `closed_at`; sending execution to a closed delegate checks the cap first. `--remove-worktree` runs `git worktree remove`, which refuses dirty checkouts. Closing also ends the delegate's harness process ([harness lifecycle](harness-lifecycle.md)). The CLI closes several targets per call and exits nonzero if any fails.
- **Wait.** `thread wait` (`wait`) blocks server-side until the listed threads settle (no in-progress turn and no queued continuation), any one blocks - a pending approval, or a `question` it mailed that has neither an `--in-reply-to` reply in its inbox nor an acknowledgement - or the timeout (default 300 s, max 1800 s) passes. It returns each thread's state and last turn's closing message. `inbox wait` (`inboxWait`) blocks until matching unacknowledged mail exists and returns it inline; `--new` excludes, by id, mail that was already waiting. Both wake on bus `thread.updated` events with a 1 s fallback tick; the CLI extends its HTTP timeout by the wait.
- **Wake.** `thread wait --wake` stores a one-shot `cli:wake:CALLER:ID` KV record (members must be the caller's descendants; at most 3 pending per caller). The interaction worker fires it when every member settles or one blocks; the header line says which (`have settled` or `is blocked`), followed by a line saying the report adds information to the delegated task rather than giving a new instruction, then each member's state and closing message. If the caller is running a turn and its harness takes steers (Claude also while it waits on a native subagent), the report is steered into that turn; if steering fails, or the caller is idle or its harness cannot steer, it is queued as a single continuation. Its prompt origin is `system`, so it is not counted as the user's speech. Until it fires, the caller's thread detail lists it in `pendingWakes` (members with name and thread status), shown above the composer as **Waiting for wake**; a queued wake report names its source in the queue instead of its header. This is an immediate, receiver-requested continuation, not an automation; the delayed inbox reminder below is a separate fallback.
- **Task board.** Tasks are numbered per lineage root. `task claim` is a single transaction guarded on `status='pending'`; dependencies (`--after`) must be completed before a claim. A null claim reports `finished` (nothing pending, or nothing in progress that could unblock what is pending) and task counts; `task claim --wait` blocks while pending work is only blocked on in-progress tasks. `task done` mails the creator the result and mails assignees of newly unblocked tasks; `--assign` mails the assignee. All mail is passive.
- **Roles.** `--role R` reads `.pockymoe/agents/R.md` from the workspace, then `~/.pockymoe/agents/`; the pre-rename `.remote-codex/agents/` directories are still read after each. Front matter (`model`, `effort`, `agent`, `description`) supplies defaults the caller can override; the body is staged in KV and prefixed to the thread's first turn only.
- **Worktrees.** `--worktree [--worktree-branch B]` creates `../REPO.worktrees/SLUG` on branch `agent/SLUG` (or `agent/SLUG-N` when that branch already exists; an explicit branch is checked out as is) from committed HEAD before the harness session starts (and removes it if the session fails to start). Session start, resume, goal and capability calls use the worktree as cwd.
- **Tree.** `thread tree` lists the lineage depth-first with each thread's state, unread count, current task, role and worktree, plus task counts.

## Delegate questions in task history

A registered delegate wake captures the caller's original task turn and any
questions that caused the wake. That association follows the durable queue
entry into the accepted execution turn. A wake steered into a running turn is
already part of that task's turn and reads there as a one-line **Delegate wake**
notice. The Web timeline nests contiguous
continuations under the original task's steps, with a secondary fold titled by
the delegate name and question subject. Opening that fold loads the full
question and that round's execution history; opening the parent task alone does
not fetch all continuation details. Viewing a question does not acknowledge or
answer its inbox record.

Result notifications from a registered wait have a separate **Delegate wake**
label and bell icon. The collapsed row names the triggering delegates; expanding
it shows each source, a localized execution state and its closing report, then
**Work after wake** above the receiving agent's execution history. `idle` means
execution ended, not that the task succeeded. The UI reads reports from the
original wake envelope when older devices supply only state metadata, and shows
an explicit missing-report message if no report was included.

The physical execution turns and their billing remain separate. The task header
adds known usage and costs across its visible continuations; an unknown cost is
not treated as zero. The latest active progress or final reply stays outside the
fold, while older responses stay with the question that caused them. Pending
human approvals remain accessible outside the folded history. A new human task
is a display boundary. If pagination has not loaded the parent, a continuation
has its own question fold until the parent arrives.

For older registered wakes, history reads can project this association from
the system wake envelope, saved inbox questions and local delegate lineage.
This does not import lost messages or rewrite old execution records.

Long human prompts use a six-line text preview, bounded by 600 Unicode
characters, 160 words or eight explicit lines. Expand/Show less controls reuse
message disclosure state and keep the scroll position anchored. Attachments
remain visible in the preview, and Copy always copies the complete prompt.
A message's time and Copy appear on hover, or when it is tapped on a phone, at
its bottom right edge (a prompt's row end in the Terminal pack, never over its
❯). They float over a fade of the message's surface, so they stay readable over
the text they cover, and nothing moves.

When a turn fails, its prompt carries a red mark, and tapping the mark shows the
error under the prompt (the readable message first, then the raw text). The
thread page does not repeat that error above the thread. A reply that only
repeats the error, such as the one Claude writes for an API error, is left out of
the transcript. Claude's "<synthetic>" model name for that reply does not replace
the model that ran.

## Substantial answers during work

The timeline keeps substantive agent answers at their chronological positions,
with separate folded work sections before and after them. Explicit final replies
remain visible. When ACP does not distinguish answers from progress, an agent
message qualifies at 600 Unicode characters, or at 300 characters with a Markdown
heading and three paragraphs, or with a three-line Markdown table. This is a
presentation heuristic, not a completion signal. Short progress still uses the
latest-update preview. Lazy history summaries preserve qualifying answers;
commands and their outputs continue to load on demand.

A working turn reads open and stays short: the run of operations it is in shows
its rows and its first small edits, and each earlier run folds into one line of
counts and duration once narration or another run follows. When the turn ends,
its work folds into the work line ("Worked for 10s · ✓4 ✕1 · edited 1 file +2 −1";
counts are icons with numbers, explained after the pointer rests on one for a
second). Opening that line shows the narration with each run folded; opening a
run lists its operations. A folded turn with several answers shows the work
between them as one counted line; tapping it opens that stretch alone, under the
line (its narration, its last run open), while the rest of the turn stays folded.
A folded turn shows the prompts that started it above its work line and the
prompts added while it worked below it, so opening or folding the turn never
moves what is above the line, and every toggle stays where it was clicked. The file counts on the work line open only the
changed files. Command and tool output never opens by itself, and a row loads its
output only when clicked. Clicking an output, diff or code block opens it in a
floating viewer, and command rows carry a copy button.

Each turn opens with a head line above its prompt: a dot with the time the turn
has worked (green and pulsing while it works, red when it failed) and the cost
control, which opens the token and price breakdown. On a wide screen (640 px and
up) the same line goes on with the rest; on a phone tapping the head opens it on
one more line under it: the start time (tapping it adds the date), the model, shortened
("claude-opus-5-5" reads "opus 5.5", "gpt-6-astra" "6 astra"), with its reasoning
effort, the combined output speed, and how many prompts were added while it worked.
Tapping the speed shows how many model calls the turn made and the time to first
token, prefill and decode speeds (see [account composer](account-composer-shortcuts.md)).
While the turn works the head already shows the speed so far, from the model calls
whose output has been reported; its details say so, and their call count includes
the call in progress. A prompt added while the turn
worked carries a merge mark and its time above it. Waiting for a wake and running
background agents show as small moving marks (an hourglass, a bot with its count)
whose wording is in their label.

Settings > Preferences > Text size (12–22 px, 16 by default, stored in this
browser) scales the conversation and the prompt as a whole in every theme pack,
whatever sizes the pack gives them: the host sets `--chat-zoom` (size ÷ 16) and the
conversation's content and the prompt field are zoomed by it, so lines still wrap
at the same width and the prompt still opens when its text no longer fits on one
line. Settings, menus, the toolbar and buttons keep their size.

A toggle stays where it was clicked while what it opens or closes changes size,
so it can be clicked again in place; the timeline does not jump to its end. An
opened turn keeps its work line at the top, and an opened long message keeps
"Show less" just above the composer. A task from another thread on this device
links to that thread, named by its title when the page knows the thread (by its
short ID otherwise, and always for another device's). A workspace link brings its file or folder near the top of
the file tree; a link to a file that is not on the device shows its path instead
of an error.

A sub-thread (a thread another thread created) opened on its own has one line
pinned above its conversation: a branch mark, "Sub-thread · from" and the title of
the thread that created it as a link, that thread's workspace when it is another
one, and Back to parent at the end; on a phone the workspace is left out and Back
is an icon. The line takes its own row, so it stays while the conversation scrolls
and covers no message. It names only the direct parent. A parent that is gone, or
that the reader may not open, reads "Parent unavailable" without a link.

A photo or file in a prompt, while it is written and once it is sent, is a small
tile: the file's page glyph in its kind's color with its extension (PDF, ZIP, …),
or the photo's thumbnail, over its name shortened to its start, end and extension.
Tapping a tile opens a card with the full name, the kind and size (or, once sent,
where it is), and what can be done: view a photo full size, open a sent file in
the files view, or remove it from a prompt being written. A prompt folded to one
line shows each tile as a one-line chip.

Entering a thread always shows its conversation: the files view opens only when
asked for (the Files button or a file link), and is not restored from the last
visit. Opened over a split, the files view keeps the split beside it and closes back
to it; opened after the split was closed, it leaves the split closed. Where the files view sits beside the conversation, the start of its tree's
toolbar has window controls as on a Mac: the red one closes the view and the amber
one minimizes it. Minimized, the tree stays as a strip at the right edge and the
editor folds into a small tab on the strip's left edge; that tab, the amber control
or opening another file from the tree brings the editor back.

An item's menu in the tree (its ⋯ button) lists short one-line actions: Download,
Copy relative path, Copy absolute path, Rename, Delete and Properties. Properties
shows what a file manager does: the kind, the folder it is in, the size (and a
loaded folder's item count), when it was created, modified and last opened, its
permissions and whether it is read-only or a symbolic link. The device's tree
listing reports the times and permissions; an older Supervisor leaves them out
and says so.

On a phone, the file list and file viewer have a one-field prompt at the bottom.
It edits the same draft as the composer, so text typed in either shows in the
other, and sends it to the thread the files belong to, like Send in the composer,
together with any files already attached in the composer.

On a phone (under 640 px wide) the controls sit at the bottom for one-handed use.
The top bar shows only status: the device monitor, the subscription usage windows
(such as 5h and 7d; tapping them shows the details), how many conversations are running on the account (a threads mark with a live dot and
the count, "running" where there is room), the connection and notifications.
Tapping the count opens a sheet with those conversations, each with how long it has
run, then the finished ones that were opened before and not read since, and
a way to all threads. It counts every running thread of the devices the
navigation reads, also ones never opened, by conversation.

An agent thread is never listed on its own. Wherever one appears (this sheet,
the sidebar, the drawer, the thread sheet, the tab bar, notifications), its
conversation's root comes with it and the agent threads fold under the root's row;
the root's dot says when its agent threads run. Notifications from agent threads
sit under their conversation's title. The composer (or the files prompt)
and a row under it form one bottom panel; the row holds the current thread,
Chat / Files / Terminal and More. Tapping the thread, or swiping up anywhere on
the panel, opens a sheet with the workspace's threads (the current thread's group
first, then running and unread), recent threads and New thread; notifications
stay on the bell. More holds search, the shortcut star,
Left-handed and the thread's sharing, automation and split controls; a row fixed
at its bottom holds the thread map, the workspace drawer, Home, Back and Settings,
so they show without scrolling. A window a tool in More opens (native subagents,
automations, split view, sharing, export) rises as its own sheet over a scrim,
never inside the tool grid. Dragging a sheet down from the top of its list
closes it. The row stays while typing, right above the keyboard. On a phone the
composer shows one line of a longer prompt until the prompt is focused; it then
opens to its full height, also while a dictation or an IME's underlined candidates
are still being composed, and stays open through its own buttons, the row, menus
and the keyboard closing. Only tapping the conversation (not scrolling it) folds
it back to one line, as does emptying the prompt. The page never zooms on a
phone: focusing the prompt does not zoom it in, and neither a pinch nor a double
tap zooms it; images and the thread map keep their own pinch zoom. The tap that opens it only
opens it: if the open layout puts a button (send, the model, attachments) under
the finger, the rest of that tap does not press it. A split pins its two conversations in the
thread's place in that row: one tap brings the other forward, and tapping the
shown one opens the thread sheet.

The workspace drawer shows the app's mark and close, **All workspaces**, the
**Thread map**, then shortcuts (folded until opened), **This workspace** and, from
halfway down after a gap, recent conversations elsewhere; each half scrolls on its
own. The desktop sidebar is laid out the same way. The tab bar and **This workspace**
list the conversations started in the current workspace, each with all its agent
threads wherever they work; each row names the workspace its own thread works in. An agent thread working here for a conversation started
elsewhere belongs to that workspace; while it is open it shows inside its
conversation. Shortcuts keep the account's own order:
drag one with the mouse, long-press and drag on touch, or press Alt+↑/↓ on a
focused shortcut. The order is saved with the account (relay
`PUT /relay/account/workbench/shortcuts`, `favoriteOrder` in the workbench
snapshot; this browser's storage in local mode), so every device lists shortcuts
the same way. Until the first reorder they list by recency; a new shortcut goes
last.

On the desktop the tabs are the conversations the reader opened, like an
editor's files: opening one adds its tab after the active one; ✕ (or a middle
click) closes it, and closing the open one moves to its neighbour (or home when
it was the last). They are kept in this browser (`pockymoe.open-tabs.v1`, up to
40) and can be dragged into another order. A tab held and pulled down onto the
conversation opens split: while it is held, both halves say what a drop there
does (the half under the pointer deepens); on the right it opens as the split, on
the left it becomes the conversation and the open one moves to the right. Esc
cancels the drag. Switching tabs, to another device's conversation too, keeps
the page in place (the app is never remounted for another device: its plugins
and capabilities swap in, from what it answered last) and shows the conversation at once: a conversation read in this visit is
shown as last read and refreshed quietly, and once a conversation shows, the
other open tabs (up to 11, any workspace) are read one by one in the background.
Each tab's agent threads are a small robot and count right after its icon, on
the tab's own background. Recent chats in the sidebar therefore
start folded; shortcuts and **This workspace** start open, and every fold is
remembered (`pockymoe.sidebar-sections.v1`). The sidebar's width follows a drag
on its edge (200–420px, Arrow keys on the focused edge, double-click resets;
`pockymoe.sidebar-width`). Its lists scroll against the very edge, rows are
compact, and each row's menu is a vertical ⋯ shown when the row is pointed at
(always shown without hover). Phones keep the tabs of the current workspace.

This workspace's conversations keep an order of their own, the same in the
sidebar's **This workspace**, the phone's tab bar and its thread sheet: a mouse drags
one once it moves, a touch after resting still for 0.7 s (the item sinks under a
deepening tint, then lifts; the sheet does not slide away meanwhile). An agent
thread stays with its conversation. The order is kept in this browser per device
and workspace (`pockymoe.workspace-order.v1`); new conversations follow it by age.
A conversation's agent threads are a small robot and count inside its row that
opens them below it; the row keeps its place, and the thread sheet keeps its height
and scrolls rather than growing upward. Both sections of the thread sheet fold.

On the desktop the tabs row is the only bar: search at its left (Ctrl/⌘+F opens
it, except in the terminal and code editors, which keep their own find; the open
field covers the tabs), then the tabs, New thread and the thread tools, with the
connection and notifications at the right. The sidebar switch, Back and Forward
moved to the rail under the brand, which leads home. The thread tools menu holds
the thread's actions, then its panels (agents, automations, plugins, split); it
stays mounted while closed so the windows they open live on, and keeps clear of
the screen edges. Phones keep their top bar; it and the tab bar never start a text selection, so a
long press there only drags a tab. The drawer follows a sideways drag, with or
without the one-handed layout: rightwards from anywhere in the left 40% of the conversation or
the files opens it, leftwards on it or on the dimmed page closes it, and so does
a tap outside. Browsers cannot keep the screen edge from the system back gesture
(Android gesture navigation, Safari's swipe back), so a drag that starts on the
very edge belongs to the system; starting a little further in opens the drawer.
Rightward drags on code blocks that can still scroll back, on the composer panel
and in the terminal are left to them, as are leftward drags on the right side.

Settings > Preferences > One-handed phone layout (on by default, stored in this
browser) turns it off and restores the previous top bar, thread tabs, composer
usage and split switches. Its Hand choice (also More > Left-handed) mirrors the
panel for the left thumb: More, the views, Send, the turn strip, the files
prompt's send button and the thread sheet's New thread move to the left.

Above the composer, at its right edge, one strip holds previous turn, next turn
and the latest reply; the latest-reply button takes an accent tint (not Send's
fill) only while the reader is away from the end. An opened message's "Show
less" rides just above the composer, or just above the strip where the strip is
under it (in either hand), so neither covers the other and no gap opens.

The compact model menu shows reported context use and capacity in thousands of
tokens below the reasoning controls. Missing readings show a dash. Picking a model
or a reasoning effort keeps the menu open, so the other can be set right after.

Thread loading uses a quiet spinner and placeholder lines until the initial
conversation is ready, including the secondary split pane. Expanding deferred
execution history shows an inline spinner until that request settles. Reduced
motion disables these animations. The thread tools toggle opens a dropdown
under it instead of a second row: its head is the workspace path (shortened in the
middle, with copy) and the thread's name beside its shortcut star; below are the
thread actions (share as link, sharing permissions, download transcript, and port
mappings on remote devices), each an icon with its name. Arrow keys move through it,
Esc or a press outside closes it, and an action closes it before opening its window.

## Thread map

The thread map shows every thread on the account's devices at once: devices hold
workspaces, workspaces hold threads, and agent threads are folded into their
conversation's root thread. An agent thread working in another workspace than that
root has a tile of its own under the workspace it works in, with the agent threads
it started there folded in, so its time and tokens count where the work happened;
a wide tile says which thread started it ("from …"). A thread's area grows with
its turns (as 1 + √turns, so a long thread does not crowd out the rest). Its fill takes one of five light steps toward
the accent by how long its turns ran in the last 24 hours (none, up to 10 minutes,
an hour, three hours, more), light enough to read the title on; themes may set
`--thread-map-heat`. Running threads have their own color (orange; themes may set
`--thread-map-live`): a ring and a pulse that rises quickly and fades slowly, a
halo outside the tile and a glow in from its edge, so it shows on any fill (a still
ring under reduced motion). Devices and workspaces with agents running inside take
that color on their edge and in the dot by their count. A running thread, or one
whose agent threads run, weighs at least as much as a long thread, so it and its
workspace and device get room; a block too small for everything in it still shows
the agents running there (up to four), with the rest counted under them. A small mark names its agent: the theme's art for that agent where a pack
has it, else a colored monogram.

A tile shows what its width and height leave room for, and leaves out the rest:
its title (one to three lines); the thread's age as a pie that fills and warms from
cyan (today) through blue, violet and amber to bronze (older than three months),
with the age in words on a wide tile; the agent and model with "running" or when it
was last active; the person's latest prompt; the turns, the time all its turns ran
and the time they ran in the last 24 hours (side by side, stacked on a tall narrow
tile, or "69 turns · 25 h total" on one line); the tokens and API price of all its
turns, the price opening the same breakdown as a turn's; and the last 24 hours hour
by hour as bars, each full when the thread ran the whole hour. Small tiles keep the
agent mark and title, the smallest only their color. Device and workspace bars and
the map's summary add up the API price.

A tile with agent threads folded in has a small button with their count; it opens
the tile up: the thread keeps a bar on top (tapping it folds them back, its arrow
opens the thread) and its agent threads
fill the rest, each a tile of its own. The map zooms, and every level answers:
tapping a device's or workspace's header row (its name, count and price) fills the
map with that block, from any level and growing from where it was; a block too small
to show what it holds opens when tapped anywhere; and a thread's tile opens the
thread wherever it shows. Hovering a tile or its price shows their details at any
level. The path above and the back arrow lead back. Beside each device and
workspace breadcrumb, an arrow link opens the device's workspace list or that
workspace's unselected page; modified clicks retain normal browser behavior.
On touch two fingers
pinching in go up a level, spreading them opens the block under them, and dragging
the map down closes it. A block too small for what it holds folds up. Go to (the list icon) reaches every block of the current level,
however small. Hovering or focusing a block shows its details. The map reads its
devices again every 30 seconds while it is open.

It opens from More on a phone, the workspace drawer, the rail on wider screens and
the workspace list's header. In relay mode it reads each of the account's own
online devices through the relay, using `relay/portal` connectivity. Offline
devices and their cached threads are excluded; an online device that cannot be
read offers Retry. In local mode it reads this Supervisor. Shared devices have
no map; a host-scoped Supervisor shows only the threads that tunnel may open.

Each Supervisor reports turns and run time at `GET /api/thread-activity?hours=24`
(1–168, default 24; owner only):
`{ "since", "hours", "threads": [{ "threadId", "turns", "activeMs", "totalMs", "hourlyMs", "lastPrompt", "tokenUsage", "priceEstimate", "pricePartial" }] }`.
`turns` counts every turn the thread ever started, `totalMs` the time they all ran,
`activeMs` the part inside the window and `hourlyMs` that part hour by hour (absent
when zero); `lastPrompt` is the newest prompt with text, whitespace collapsed and
cut to 240 characters; `tokenUsage` and `priceEstimate` add up the turns' tokens and
API price (USD, by kind, at the device's model prices), and `pricePartial` says some
turns used a model without a price and are left out. A turn without a completion
time counts up to now only while it is in progress and its thread runs, so imported
or crashed turns add none. A
Supervisor without the endpoint still appears, with equal areas and no color.

## Switching threads and pages

Moving around keeps the screen steady:

- Home, the workspace list and the workspace drawer navigate inside the app; only
  a modified or middle click opens a new page.
- Opening a workspace from its list, navigation or map uses
  `/threads?workspaceId=…` (under `/devices/:id` in relay mode). It shows an
  unselected workspace with its conversations and their agent threads in the
  sidebar. The main area offers a new thread. A phone has no sidebar, so there
  the page itself lists the workspace's conversations (the same rows as the
  thread sheet) under a one-row header with the workspace's name and New
  thread, and the dock shows the workspace's name without a status mark.
  Loading, polling and refreshing never select a conversation automatically;
  the reader opens one explicitly.
- A recently opened thread (the last twelve, per device) shows its conversation
  at once and refreshes quietly. Pressing a thread row starts loading it early; a
  load that finishes within a moment never shows the loader.
- A thread opens at its end without first painting its top; messages on screen
  render their Markdown before the first paint.
- Sending shows on the send key: it turns into a spinner until the request is
  answered, with no tint over the prompt. A prompt shows once, where it goes:
  starting a turn, in the conversation at once; steered into the running turn,
  in the conversation; queued after the running turn, under Queued from the
  moment it is sent. Queued lists only what the Supervisor holds
  (`thread_pending_steers`, `continuation`), so it is the same on every device
  and survives a reload; the page's own copy only bridges the request.
  A steer takes its place in the running turn as soon as the Supervisor marks it
  delivered, without passing through a separate bubble below the conversation;
  the page counts the thread idle the moment its turn ends, so a prompt sent
  right then starts a turn rather than steering into the finished one.
- The files view's tree comes back with the folders that were open, all at
  once: a reload reads them a level at a time, each level's folders together,
  before the tree shows, instead of the tree growing (and jumping) folder by
  folder. A shortened file name shows in full in a small bubble: after the
  pointer rests on it, or while a finger holds it (it goes when the finger
  lifts). The bubble sits over or under the row and never covers the ⋯ column.
- Earlier turns load above what is on screen without moving it: the turn at the
  top of the screen stays where it was through the loading row, the insertion
  and the new turns' rendering. It is corrected only when content changes size;
  the reader's own scrolling (a swipe and its momentum) is never pulled back,
  and a jump to a turn or to the latest ends the hold.
- A turn the reader folded or opened stays that way while the page is open, also
  after leaving the thread and coming back; a search result opens its turn once, and
  folding it again sticks while the thread keeps working.
- Each page load paints the last theme and background before the app starts, so
  a reload does not flash white or the other color mode.

## Terminal clipboard

The terminal follows the usual desktop terminal conventions, adjustable in
Settings > Plugins > Terminal settings (shown while the terminal plugin is
enabled, stored in this browser):

- Ctrl+C copies a selection and interrupts only without one; Ctrl+Shift+C copies
  too where the browser does not keep that shortcut for itself.
- Ctrl+V pastes at a shell prompt and stays a key in full-screen programs that use
  the alternate screen (vim, less, htop); it can be set to always paste.
  Ctrl+Shift+V always pastes.
- Copy on select is on by default: releasing a mouse selection copies it.
- Right-click opens Copy, Paste, Select all, Copy last output and Clear, or can be
  set to copy a selection and otherwise paste.
- A paste of several lines into a shell without bracketed paste asks first, since
  the shell would run the lines one by one.
- On a Mac ⌘C and ⌘V copy and paste, and Ctrl stays with the terminal.
- On a touch keyboard the terminal's field takes text as any field does, so input
  methods work (pinyin and other candidates, predictions, autocorrect): the shell
  gets what changed, a composition once it is chosen. Enter, arrows, Tab, Esc and
  modified keys go to the terminal as keys.
- Output replayed on attaching or redrawn from a snapshot answers no queries:
  programs asked for the cursor position or colours when it first ran, and those
  answers must not arrive at a prompt as typed text.

The shell runs with `TERM=xterm-256color` and `COLORTERM=truecolor`, since the
terminal is always xterm.js, and without the variables that name the Supervisor's
own terminal (`TMUX`, `TERM_PROGRAM`, …). Without any locale (a launchd or systemd
service has none) it gets `LANG=en_US.UTF-8` on macOS and `C.UTF-8` elsewhere. A
service-started Supervisor has no `TERM`, and zsh then cannot move the cursor
left, so Delete looked like it typed spaces.

On a phone, xterm cannot be drag-selected, so a long press on the terminal (or
Select in the key bar) shows its lines as plain text in the same place, with the
word under the finger selected; the phone's own handles and copy menu work there,
next to Select all, Copy and Done. Paste in the key bar reads the clipboard; where
the browser does not allow that (plain http, permission denied) it opens a field
to paste into and edit before sending.
