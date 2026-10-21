A working turn no longer goes missing steps after a weak connection or a
backgrounded tab: what the page missed comes back without a reload. Releases
now also carry the setup scripts.

- **Missed steps come back**: a working turn's summary carries its messages, and
  its tool calls, commands and edits came from one detail read plus live events.
  Events lost while the connection was down or the tab was suspended were never
  read again, so the turn showed fewer steps (and a wrong command count) until a
  reload. The summary now says how many steps the turn has; when the page holds
  fewer, it reads the turn's detail again, and a failed read is retried when
  that count changes.
- **No silent gaps**: a Supervisor socket that falls behind its thread events
  used to skip them and stay open. It now closes, so the page reconnects and
  reads the thread again (over the relay, the browser's connection closes too).
- **Back from the background**: returning to a tab while a turn runs reads the
  thread once, even when its socket still looks open; a suspended phone's socket
  can stop delivering without closing.
- **Setup scripts in every release**: `setup.sh` and `setup.ps1` are release
  assets alongside the binaries.

Validated with the Supervisor socket tests (a lagging client is now
disconnected), the timeline tests (missed steps are read again, once per
reported count), new end-to-end scenarios for a lost connection and a suspended
tab that fail without the fix, and a comparison of the live page with a
reloaded one under dropped connections, offline periods and suspension.
Installing this release is a separate device update.
