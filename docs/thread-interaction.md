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

CLI connections use a loopback HTTP URL. Managed processes receive an opaque thread-bound bearer credential; the local connection file holds a machine credential for discovery and messaging. `POST /api/cli` rejects missing/invalid credentials even in local mode and rejects trusted Relay-forwarded requests. Child deletion verifies the authenticated parent. Managed peer sends bind attribution to the credential: omitting `fromThreadId` cannot bypass delivery checks and a conflicting `--from` is rejected. Machine credentials still support explicit attribution. Shared filesystem access is not a per-agent isolation boundary. Do not publish or print the connection file/token. Full thread URLs use `info.relayDeviceId` (with legacy `deviceId` compatibility) to select local handling or another device.

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
between them as one counted line. The file counts on the work line open only the
changed files. Command and tool output never opens by itself, and a row loads its
output only when clicked. Clicking an output, diff or code block opens it in a
floating viewer, and command rows carry a copy button.

A toggle stays where it was clicked while what it opens or closes changes size,
so it can be clicked again in place; the timeline does not jump to its end. An
opened turn keeps its work line at the top, and an opened long message keeps
"Show less" just above the composer. A task from another thread on this device
links to that thread. A workspace link brings its file or folder near the top of
the file tree; a link to a file that is not on the device shows its path instead
of an error.

On a phone, the file list and file viewer have a one-field prompt at the bottom.
It sends to the thread the files belong to, like Send in the composer, without
clearing the composer's draft.

On a phone (under 640 px wide) the controls sit at the bottom for one-handed use.
The top bar shows only status: the device monitor, how many other threads are
running, the connection and notifications. A row under the composer holds the
current thread, Chat / Files / Terminal and More. Tapping the thread, or swiping up
on it, opens a sheet with the workspace's threads (running and unread first),
recent threads, unread notifications, the workspace drawer and New thread.
Swiping it sideways opens the neighbouring open thread. More holds search, the
shortcut star, the thread's sharing, automation and split controls, Back, Home
and Settings. The row hides while a text field has focus. Setting
`localStorage['pockymoe.phoneDock']` to `off` restores the previous top bar and
thread tabs.

The compact model menu shows reported context use and capacity in thousands of
tokens below the reasoning controls. Missing readings show a dash.

Thread loading uses a quiet spinner and placeholder lines until the initial
conversation is ready, including the secondary split pane. Expanding deferred
execution history shows an inline spinner until that request settles. Reduced
motion disables these animations. The thread tools bar keeps its direct actions
without a duplicate overflow menu.
