# Storage: disk use and command-output retention

Settings → Resources has two parts: **Memory** holds the harness release rules
([harness lifecycle](harness-lifecycle.md#settings)), and **Disk** shows what this
device's Supervisor database holds and can remove old command output.

## What takes the space

Thread history lives in `thread_history_items`, one JSON item per transcript
entry. Command executions dominate: on a measured device (640 MB database,
449 MB of history) they were 83% of history. Most of that is their output in
`detailText` (`Tool/Kind/Status/Input` sections, then `Output:` or `Result:`).
Opening a thread does not read it: the summary view loads the last turns in
compact form and fetches a tool's detail only when it is opened.

## Scan

A scan runs only on request (**Scan**), on its own read-only SQLite connection,
so the shared connection stays free; it takes about a second for 450 MB of
history. It reports the database file with its WAL, free pages inside the file,
history and command bytes, output already cleaned up, thread and conversation
counts, and conversations ordered by size. A conversation is a root thread with
its agent threads (UI principle 7). The result is kept in memory and read in
pages of up to 100.

## Command-output retention

`StorageSettingsDto.commandOutputRetentionDays` (1–3650, `null` keeps output) is
stored in `storage_settings`. Saving a value starts a cleanup at once; afterwards
one runs three minutes after start and then hourly. **Clean up now** starts one with
the saved value after a confirmation.

A cleanup changes command executions whose turn ended (or, for turns that never
recorded an end, started) before the cutoff and is no longer running. It keeps
the command text and the detail header (tool, kind, status) and removes the
`Input:` section, which repeats the command, and the output. It drops
`previewText` when it equals `text` and records `outputPrunedAt` and
`outputPrunedBytes` on the item; items an earlier version cleaned up lose their
input on the next pass. A detail without a header is removed whole. Watches read
the input of tool calls only, never of commands. The transcript then shows
"Output cleaned up on DATE (was SIZE)" instead of loading the detail. Search
indexes only messages, so it is unaffected.

### Running in the background

Cleanup and reclaiming are jobs: their routes answer `202` with the job at once,
and `GET /api/storage` reports it (`job`: action, state, items to look at and
looked at, items changed and bytes removed, sizes before and after a compaction,
error). Settings → Resources shows a progress bar and follows it every second,
also after a reload. One job runs at a time; starting another returns the
running one. The latest job is kept until the Supervisor restarts.

A cleanup reads its candidates on its own read-only connection, then rewrites
them on the shared connection in transactions of about 20 ms of work, pausing
15 ms between them. The shared connection uses `synchronous=FULL`, and its
automatic checkpoint would copy the log inside one of those commits while every
other user waits; during a cleanup it is off, and a passive checkpoint runs each
second on a separate connection instead, outside the shared lock. The setting is
restored when the job ends. On a copy of a real database (3 days, 58,508 items,
8 concurrent request loops) the slowest request waited 289 ms, median 8 ms,
instead of 1.7–3.2 s and 52 ms with one-step cleanup; the log stayed near 12 MB.

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
| `GET`/`PUT /api/storage/settings` | `StorageSettingsDto`; the key must be present |
| `POST /api/storage/prune` | `202` `StorageJobDto`; 409 `retention_off` without a saved value |
| `POST /api/storage/compact` | `202` `StorageJobDto` |

They follow the device owner like the resource settings; grant holders get
`grant_denied`, and a Treer Agent UI tunnel does not offer them. Types:
[`crates/protocol/src/storage.rs`](../crates/protocol/src/storage.rs) and
`packages/shared/src/storage.ts`.
