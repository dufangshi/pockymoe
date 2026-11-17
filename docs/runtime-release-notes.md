A performance and resource release: a device that is idle stays quiet, long
turns write far less, old files are cleaned up, and Codex usage is complete.

- **Codex usage includes sub-agents.** A turn that delegates (the `ultra` effort
  does by itself) showed only the main agent's tokens and cost. Every model
  request of the main agent and its sub-agents is now counted, each with its own
  model. Earlier turns keep their old totals.
- **New upstream models appear for Codex.** With a saved connection applied to
  Codex, the models that upstream lists are offered in the model picker even
  when the Codex binary does not know them yet (for example a newer GPT family).
  Without an applied connection the list is still Codex's own.
- **An idle device stops reading its disk.** A background check for Claude
  threads ran every two seconds over every thread ever opened; it was about 70%
  of a quiet device's disk reads.
- **Long turns write much less.** Streamed output is saved in small batches
  instead of once per piece, and a running turn's usage record is rewritten every
  20 seconds instead of on every tool event and every two seconds.
- **Disk space comes back.** Old runtime versions (the current and the previous
  one are kept), finished update folders, unused adapter installs and the adapter
  download cache are removed, starting five minutes after start and then daily.
  On one device this was about 6.5 GB.
- **A smaller, bounded database.** Its write-ahead file is capped, free space is
  returned when the device is idle, handled agent mail and old receipts expire
  after 30 days, and reading mail no longer scans its whole history.
- **Less memory on Linux**, by limiting the allocator's arenas, and Claude
  session files are read incrementally instead of whole after every turn.
- The relay no longer writes to its database on every device heartbeat.
