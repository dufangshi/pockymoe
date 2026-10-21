Fork a thread at any finished turn from its reply (Codex, Claude Code and
DeepSeek Harness); sending shows on the send key and each prompt lands once,
where it goes; earlier turns load without moving the page; a workspace opens
without picking a thread for you; the files tree comes back whole after a
reload; and Safari no longer asks to unlock when a photo loads.

- **Fork at a turn**: a finished turn's reply has a fork button beside its copy
  button (shown on hover, focus or a tap on the reply). After a confirmation it
  forks the thread with history up to that turn and opens the fork; the source
  is unchanged. Claude Code forks at the reply's message (adapter 0.86.0 or
  later); DeepSeek Harness forks inside its own process (the npm installation)
  and the source thread reloads on its next use. Other harnesses still fork at
  the latest turn only and show no button.
- **Sending**: the prompt no longer turns amber while it is sent; the send key
  spins until the request is answered. A prompt starting a turn appears in the
  conversation at once, a steer appears in the conversation, and a prompt queued
  after the running turn appears under Queued from the moment it is sent,
  instead of passing through Queued first. Queued is the Supervisor's queue, the
  same on every device.
- **Earlier turns**: loading them keeps the turn at the top of the screen in
  place through the loading row, the insertion and the new turns' rendering.
- **Workspaces and the thread map**: opening a workspace shows its own page and
  its threads (on a phone, listed on the page) without selecting one. The map
  leaves out offline devices, and its path has a small button to open the
  device or workspace shown.
- **Files view**: after a reload the tree shows the folders that were open all
  at once instead of growing folder by folder. A shortened file name shows in
  full in a bubble on hover, or while held on touch (it never covers the ⋯
  column); the long press there no longer opens the row menu.
- **Safari photos**: images go through the service worker, which in Safari
  cannot always read the browser key the page holds; it read the key as missing
  and opened "Unlock this browser" whenever a photo loaded. The worker now hands
  such a read to the page, and only a page opens the prompt.

Validated with the runtime fork, adapter, DSH and mapper tests, Supervisor
checks, the Web and shared UI suites, real-harness forks on Codex, Claude Code
and DSH, and browser checks in every theme at phone and desktop widths (send
flow and earlier-turn loads measured frame by frame with added latency).
Installing this release is a separate device update; the Web parts are live on
publication.
