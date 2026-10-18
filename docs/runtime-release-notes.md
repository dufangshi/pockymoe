Settings show what the database holds on disk and can clean up old command
output; clicks no longer leave controls highlighted; the thread map works
inside Treer; the composer is reorganised in two layers.

- **Disk in Settings → Resources**: Resources now has Memory (the release rules,
  aligned in one card) and Disk. Disk names the device and, on Scan, measures
  the database: file and reclaimable size, history and command share, threads,
  and conversations by size with their command part, loading more as you scroll.
- **Command-output cleanup**: set a number of days and Pockymoe removes the output
  of older commands while keeping each command, its status and input; opening one
  says it was cleaned up. It runs at once and then hourly; Clean up now and
  Reclaim disk space (which rewrites the file and pauses for a few seconds) ask
  first. On a measured device, 7 days removed 148 MB of 449 MB of history.
- **No click residue**: after a mouse click, hover-only controls such as a
  message's copy button hide again when the pointer leaves, and pressed buttons
  return to normal. Keyboard focus keeps its ring; touch taps still reveal
  actions. This is now UI principle 16, followed by every theme pack.
- **Thread map in Treer**: a Treer Agent tab offers the thread map again, rooted at
  this hub and limited to the threads that tab may open, including their activity.
- **Composer in two layers**: folded it is one line with stop and send; editing
  gives the text the whole width with the tools in a row at the foot.

Validated with the storage, migration, grant and Treer tunnel Rust tests, the Web
and shared UI tests, a focused browser run in three themes light and dark, the
Treer hub browser check, and a scan, cleanup and compaction on a copy of a real
database. Windows Device Manager is unchanged. Installing this release is a
separate device update; publication does not restart an existing Supervisor.
