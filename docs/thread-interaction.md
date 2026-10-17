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
verification of the peer's `relay-fingerprint`.

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
- **Wake.** `thread wait --wake` stores a one-shot `cli:wake:CALLER:ID` KV record (members must be the caller's descendants; at most 3 pending per caller). The interaction worker fires it when every member settles or one blocks, enqueueing a single continuation on the caller with the members' closing messages. This is an immediate, receiver-requested continuation; the delayed inbox reminder below is a separate fallback.
- **Task board.** Tasks are numbered per lineage root. `task claim` is a single transaction guarded on `status='pending'`; dependencies (`--after`) must be completed before a claim. A null claim reports `finished` (nothing pending, or nothing in progress that could unblock what is pending) and task counts; `task claim --wait` blocks while pending work is only blocked on in-progress tasks. `task done` mails the creator the result and mails assignees of newly unblocked tasks; `--assign` mails the assignee. All mail is passive.
- **Roles.** `--role R` reads `.pockymoe/agents/R.md` from the workspace, then `~/.pockymoe/agents/`; the pre-rename `.remote-codex/agents/` directories are still read after each. Front matter (`model`, `effort`, `agent`, `description`) supplies defaults the caller can override; the body is staged in KV and prefixed to the thread's first turn only.
- **Worktrees.** `--worktree [--worktree-branch B]` creates `../REPO.worktrees/SLUG` on branch `agent/SLUG` (or `agent/SLUG-N` when that branch already exists; an explicit branch is checked out as is) from committed HEAD before the harness session starts (and removes it if the session fails to start). Session start, resume, goal and capability calls use the worktree as cwd.
- **Tree.** `thread tree` lists the lineage depth-first with each thread's state, unread count, current task, role and worktree, plus task counts.

## Delegate questions in task history

A registered delegate wake captures the caller's original task turn and any
questions that caused the wake. That association follows the durable queue
entry into the accepted execution turn. The Web timeline nests contiguous
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
Tapping the speed shows the time to first token, prefill and decode speeds (see
[account composer](account-composer-shortcuts.md)). A prompt added while the turn
worked carries a merge mark and its time above it. Waiting for a wake and running
background agents show as small moving marks (an hourglass, a bot with its count)
whose wording is in their label.

A toggle stays where it was clicked while what it opens or closes changes size,
so it can be clicked again in place; the timeline does not jump to its end. An
opened turn keeps its work line at the top, and an opened long message keeps
"Show less" just above the composer. A task from another thread on this device
links to that thread. A workspace link brings its file or folder near the top of
the file tree; a link to a file that is not on the device shows its path instead
of an error.

A photo or file in a prompt, while it is written and once it is sent, is a small
tile: the file's page glyph in its kind's color with its extension (PDF, ZIP, …),
or the photo's thumbnail, over its name shortened to its start, end and extension.
Tapping a tile opens a card with the full name, the kind and size (or, once sent,
where it is), and what can be done: view a photo full size, open a sent file in
the files view, or remove it from a prompt being written. A prompt folded to one
line shows each tile as a one-line chip.

Entering a thread always shows its conversation: the files view opens only when
asked for (the Files button or a file link), and is not restored from the last
visit. Where the files view sits beside the conversation, the start of its tree's
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
(such as 5h and 7d; tapping them shows the details), how many threads are running on the account (a threads mark with a live dot and
the count, "running" where there is room), the connection and notifications.
Tapping the count opens a sheet with those threads, each with how long it has
run, then the finished threads that were opened before and not read since, and
a way to all threads. It counts every running thread of the devices the
navigation reads, also ones never opened. The composer (or the files prompt)
and a row under it form one bottom panel; the row holds the current thread,
Chat / Files / Terminal and More. Tapping the thread, or swiping up anywhere on
the panel, opens a sheet with the workspace's threads (the current thread's group
first, then running and unread), recent threads and New thread; notifications
stay on the bell. More holds search, the shortcut star,
Left-handed and the thread's sharing, automation and split controls; a row fixed
at its bottom holds the thread map, the workspace drawer, Home, Back and Settings,
so they show without scrolling. Dragging a sheet down from the top of its list
closes it. The row stays while typing, right above the keyboard. On a phone the
composer shows one line of a longer prompt until the prompt is focused; it then
opens to its full height, also while a dictation or an IME's underlined candidates
are still being composed, and stays open through its own buttons, the row, menus
and the keyboard closing. Only tapping the conversation (not scrolling it) folds
it back to one line, as does emptying the prompt. A split pins its two conversations in the
thread's place in that row: one tap brings the other forward, and tapping the
shown one opens the thread sheet.

The workspace drawer shows the app's mark and close, **All workspaces**, the
**Thread map**, shortcuts and recent conversations. Shortcuts keep the account's own order:
drag one with the mouse, long-press and drag on touch, or press Alt+↑/↓ on a
focused shortcut. The order is saved with the account (relay
`PUT /relay/account/workbench/shortcuts`, `favoriteOrder` in the workbench
snapshot; this browser's storage in local mode), so every device lists shortcuts
the same way. Until the first reorder they list by recency; a new shortcut goes
last. The drawer follows a sideways drag: rightwards from anywhere in the left 40% of the conversation or
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
tokens below the reasoning controls. Missing readings show a dash.

Thread loading uses a quiet spinner and placeholder lines until the initial
conversation is ready, including the secondary split pane. Expanding deferred
execution history shows an inline spinner until that request settles. Reduced
motion disables these animations. The thread tools bar keeps its direct actions
without a duplicate overflow menu.

## Thread map

The thread map shows every thread on the account's devices at once: devices hold
workspaces, workspaces hold threads, and agent threads are folded into the thread
that started them. A thread's area grows with its turns (as 1 + √turns, so a long
thread does not crowd out the rest). Its fill takes one of five light steps toward
the accent by how long its turns ran in the last 24 hours (none, up to 10 minutes,
an hour, three hours, more), light enough to read the title on; themes may set
`--thread-map-heat`. A running thread breathes slowly (a still tint under reduced
motion). A small mark names its agent: the theme's art for that agent where a pack
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

Agent threads are folded into the thread that started them. That thread's tile has
a small button with their count; it opens the tile up: the thread keeps a bar on
top (tapping it folds them back, its arrow opens the thread) and its agent threads
fill the rest, each a tile of its own. The map zooms, and every level answers:
tapping a device's or workspace's header row (its name, count and price) fills the
map with that block, from any level and growing from where it was; a block too small
to show what it holds opens when tapped anywhere; and a thread's tile opens the
thread wherever it shows. Hovering a tile or its price shows their details at any
level. The path above and the back arrow lead back, and on touch two fingers
pinching in go up a level, spreading them opens the block under them, and dragging
the map down closes it. A block too small for what it holds folds up. Go to (the list icon) reaches every block of the current level,
however small. Hovering or focusing a block shows its details. The map reads its
devices again every 30 seconds while it is open.

It opens from More on a phone, the workspace drawer, the rail on wider screens and
the workspace list's header. In relay mode it reads each of the account's own
devices through the relay: an offline device is a hatched block and one that
cannot be read offers Retry. In local mode it reads this Supervisor. Shared
devices and host-scoped Supervisors have no map.

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
- A workspace row opens that workspace's most recent thread directly.
- A recently opened thread (the last twelve, per device) shows its conversation
  at once and refreshes quietly. Pressing a thread row starts loading it early; a
  load that finishes within a moment never shows the loader.
- A thread opens at its end without first painting its top; messages on screen
  render their Markdown before the first paint.
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

On a phone, xterm cannot be drag-selected, so a long press on the terminal (or
Select in the key bar) shows its lines as plain text in the same place, with the
word under the finger selected; the phone's own handles and copy menu work there,
next to Select all, Copy and Done. Paste in the key bar reads the clipboard; where
the browser does not allow that (plain http, permission denied) it opens a field
to paste into and edit before sending.
