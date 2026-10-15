# Native subagent inspection

The native agents button now supports both Claude Agent/Task and Codex native
subagents, including both the direct Claude provider and its ACP adapter. Its list retains completed agents; select an agent to inspect its task,
model, status, recent assistant messages, tool commands and results, token total,
estimated USD cost, start time and latest native transcript update. Cost details
support mouse hover and touch. The panel refreshes every three seconds while
open; running children keep the toolbar count fresh even after their parent turn
finishes. Hidden pages pause reads and refresh when visible again.

## Data sources and API

The device Supervisor reads the native session files without starting, resuming
or writing a harness session. Claude parent Agent/Task receipts connect the tool
call ID to `<parent-session>/subagents/agent-<native-id>.jsonl`. Native SDK task
notifications provide completion when a transcript is not yet available. Codex
parent spawn calls/events and its read-only `state_*.sqlite` thread source metadata
identify children belonging to the exact parent session. The indexed rollout
path avoids repeatedly scanning the sessions directory; older installations can
use the existing rollout discovery fallback.

Claude SDK notifications can identify a child by its native task ID without a
tool-call ID. Terminal notices are matched to known children; `stopped` is shown
as interrupted and freezes the runtime at the notification timestamp. Human text
quoting a notification does not change state. Lifecycle timestamps keep older
child records from reviving a stopped agent during initial discovery, delayed
flushes or transcript replacement. Actual newer child work can resume normally.

- `GET /api/threads/{threadId}/subagents`: `{ agents, refreshing }`.
- `GET /api/threads/{threadId}/subagents/{agentId}`: `{ agent, items, hasEarlierItems, historyMode: "lazy-v1" }`.
- Add `?before=SEQUENCE` to fetch the previous page (30 compact entries).
- Add `?itemId=ITEM_ID` to fetch one selected record body; IDs and paths remain scoped to the parent.

The managed parent thread determines provider, native session and available
children. The caller cannot provide a transcript path or inspect a child of a
different parent. Relay shared-thread access allows these two read routes within
the shared parent; it grants no additional control routes.

Readers cache incremental complete JSONL records for up to 64 parent sessions,
continue large histories in bounded refresh batches, and rebuild after process
restart from retained native files. A truncated file resets its reader. An
incomplete trailing record is retried on a later read. There is no new database
migration or change to provider execution semantics.

## Accounting and limits

Codex inherited parent messages and cumulative usage are excluded using the
child's own metadata/start time and usage baseline. If the first cumulative
snapshot lacks a baseline, its last-call usage avoids charging inherited totals.
Subsequent snapshots use cumulative deltas, including counter-reset handling.
Claude usage snapshots are deduplicated by message ID rather than summed each
time the same message is persisted. Existing pricing handles cache usage and
Codex fast/priority tier. Amounts are API price estimates, not provider invoices.
Unavailable usage or pricing remains unknown rather than a fabricated zero.

The list shows the task name, status, one latest-activity line, creation time and
relative event-update age (with the absolute time available on hover). Full prompt,
IDs and accounting are reserved for the selected detail.

The reader indexes compact entry previews and original JSONL byte positions instead
of caching complete execution bodies. The selected agent loads a 30-entry directory;
older entries page backwards without the former 200-entry cutoff. Record bodies are
read only on explicit expansion, and include the original command and result without
the former 16,000-character display limit. Continuous operations appear under a
collapsed operation summary, keeping assistant paragraphs readable. Older devices
without `historyMode` retain compatible inline expansion until migrated. Native files that were deleted or not persisted
cannot be reconstructed. Live runtime discovery still supplies legacy cards when
a native transcript is unavailable; an older device Supervisor needs a runtime
update for the new read endpoints. The panel exposes observed transcript activity,
not private model reasoning or an inferred percentage complete.

## Validation

Targeted runtime tests cover both providers, inherited usage/history, cumulative
and repeated snapshots, missing pricing, priority-tier accounting, native source
parent matching, partial JSONL, truncation, restart reconstruction and large
history continuation. Supervisor HTTP integration reads real fixture transcripts
and rejects other-parent detail access. Relay ACL tests cover the new scoped
routes. Component tests cover details, completion, fallback, stale responses and
idle-parent child polling. Browser tests cover desktop/touch inspection, usage
breakdowns, wrapping, navigation and retained completion; the existing Claude
background-agent browser regression remains covered.
