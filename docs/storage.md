# Storage: disk use and history cleanup

Settings → Resources has two parts: **Memory** holds the harness release rules
([harness lifecycle](harness-lifecycle.md#settings)), and **Disk** shows what this
device's Supervisor database holds and cleans up old command output, command
text and file-change diffs.

## What takes the space

Thread history lives in `thread_history_items`, one JSON item per transcript
entry. Command executions dominate: on a measured device (640 MB database,
449 MB of history) they were 83% of history. Most of that is their output in
`detailText` (`Tool/Kind/Status/Input` sections, then `Output:` or `Result:`).
File changes are the next largest part: their `detailText` holds the diff.
Opening a thread does not read it: the summary view loads the last turns in
compact form and fetches a tool's detail only when it is opened.

## Scan

A scan runs only on request (**Scan**), on its own read-only SQLite connection,
so the shared connection stays free; it takes about a second for 450 MB of
history. It reports the database file with its WAL, free pages inside the file,
history, command and file-change bytes, output and diffs already cleaned up, thread and conversation
counts, and conversations ordered by size. A conversation is a root thread with
its agent threads (UI principle 7). The result is kept in memory and read in
pages of up to 100.

## Each command is stored once

The command is the item's `text`. Older versions also stored it as
`previewText` and as the `command` (`cmd`, `argv`) key of the detail's
`Input:` section, which made up about a third of command bytes on a measured
device. The ACP mapper and session import no longer do: a command's input keeps
only its other keys (description, timeout, …) and is left out when none
remain, and an imported command's detail is `Output:\n…`. Readers show `text`
when there is no preview. Every cleanup pass also removes these copies from the
items it rewrites, and once per database a `dedupe` job removes them from all
history (three minutes after start, before the first automatic cleanup; the key
`storage.commandInputDedupe.v1` records it). That pass loses nothing. On a copy
of a real database it rewrote 67,663 items in 8 seconds and removed 50 MB,
37% of command bytes.

## Cleanup

`StorageSettingsDto` is stored in `storage_settings`:

| Field | Meaning |
| --- | --- |
| `retentionDays` | 1–3650; `null` turns automatic cleanup off. The one age for everything below (old name `commandOutputRetentionDays` is accepted) |
| `commandOutput` | remove command output (default on) |
| `commandText` | `keep`, `truncate` to `commandTextChars` (16–10000, default 512) characters plus `…`, or `remove` |
| `fileDiffs` | remove file-change diffs (default off) |

Saving with an age starts a cleanup at once; afterwards one runs three minutes
after start and then hourly. **Clean up now** starts one with the saved
settings after a confirmation. The scanned list has a menu on every
conversation (⋯, right click or long press) that cleans up that conversation —
its root thread and agent threads — now, whatever the age of its turns:
command output, diffs, commands shortened to the saved length, or commands
removed (`POST /api/storage/threads/{id}/clean` with a `StorageCleanupDto`).

A cleanup changes items whose turn ended (or, for turns that never recorded an
end, started) before the cutoff and is no longer running:

- **Command output.** It keeps the detail header (tool, kind, status) and
  removes the input and output, recording `outputPrunedAt` and
  `outputPrunedBytes`. A detail without a header is removed whole. The
  transcript then shows "Output cleaned up on DATE (was SIZE)" instead of
  loading the detail.
- **Command text.** Shortening keeps the first characters and sets
  `textPruned: "truncated"`; removing empties `text` and sets
  `textPruned: "removed"`, so the transcript shows only that a command ran.
  Either drops `previewText`.
- **Diffs.** It removes a file change's `detailText` and records
  `outputPrunedAt`/`outputPrunedBytes`; the changed paths and line counts stay.
  The transcript says the diff was cleaned up.

Watches read the input of tool calls only, never of commands. Search indexes
only messages, so it is unaffected.

### Running in the background

Cleanup and reclaiming are jobs: their routes answer `202` with the job at once,
and `GET /api/storage` reports it (`job`: action, state, items to look at and
looked at, items changed and bytes removed, sizes before and after a compaction,
error, and `threadId` for a conversation's cleanup). Settings → Resources shows
a progress bar — on the conversation's row for its own cleanup — and follows it
every second, also after a reload. One job runs at a time; starting another
answers 409 `storage_busy`, except that saving settings (or the hourly pass)
while a job runs starts the saved cleanup when that job ends. The log records
each job's scope and options (`storage job started`) and its result. The latest job is kept until the Supervisor restarts.

A cleanup reads its candidates on its own read-only connection, then rewrites
them on the shared connection in transactions of about 20 ms of work, pausing
15 ms between them. The shared connection uses `synchronous=FULL`, and its
automatic checkpoint would copy the log inside one of those commits while every
other user waits; during a cleanup it is off, and a passive checkpoint runs each
second on a separate connection instead, outside the shared lock. The setting is
restored when the job ends. On a copy of a real database (3 days, 58,508 items,
8 concurrent request loops) the slowest request waited 289 ms, median 8 ms,
instead of 1.7–3.2 s and 52 ms with one-step cleanup; the log stayed near 12 MB.
With every option on, a 3-day cleanup of a later copy rewrote 63,788 items in
5 seconds and removed 47 MB (diffs went from 29 MB to 6 MB); the slowest of
5,909 concurrent requests waited 63 ms, p99 49 ms.

## Reclaiming disk space

SQLite keeps removed data as free pages that new data reuses; the file does not
shrink by itself. **Reclaim** runs `VACUUM` and a WAL checkpoint after a
confirmation. Every database use waits meanwhile: on the measured device it took
about 7 seconds and the file went from 647 MB to 460 MB after a 7-day cleanup.

## Interfaces

| Route | Body/response |
| --- | --- |
| `GET /api/storage?offset=&limit=` | `StorageOverviewDto`: `deviceName`, the last scan's page (`null` before a scan) and the running or last `job` |
| `POST /api/storage/scan` | `StorageScanDto` (first page) |
| `GET`/`PUT /api/storage/settings` | `StorageSettingsDto`; `retentionDays` must be present, missing options take their defaults |
| `POST /api/storage/prune` | `202` `StorageJobDto`; 409 `retention_off` without a saved age |
| `POST /api/storage/threads/{id}/clean` | `StorageCleanupDto` → `202` `StorageJobDto`; 404 for an unknown thread |
| `POST /api/storage/compact` | `202` `StorageJobDto` |

Starting a job while one runs answers 409 `storage_busy`.

They follow the device owner like the resource settings; grant holders get
`grant_denied`, and a Treer Agent UI tunnel does not offer them. Types:
[`crates/protocol/src/storage.rs`](../crates/protocol/src/storage.rs) and
`packages/shared/src/storage.ts`.
