Tab switches show the conversation at once in any workspace; a busy relay
tunnel queues instead of reporting the device offline; no background loop asks
for the password; DSH can use a Responses-only upstream; the upstream page shows
each result where it was asked for. This release also carries 0.13.31, whose
release page showed the previous notes.

- **Tab switches without a flash**: switching conversations keeps the page,
  tabs and sidebar in place, and a draft typed in one tab is still there on
  return. Once a conversation shows, the other open tabs (up to 11, any
  workspace) are read one by one in the background, so a tab not yet visited
  (often one from another workspace) no longer waits for the device. A thread's
  relay access as last checked is reused while it is checked again.
- **Tabs**: the mouse wheel scrolls the tab strip sideways; a tab other than the
  open one shows its ✕ only when pointed at, floating over its end; a working
  tab's icon wears a ring (the accent while the thread runs, pink-violet while
  only its agent threads do); the agent count sits right after the icon,
  without a fill, and the sidebar's agent toggles lose their border. The split
  picker no longer jumps to the left edge.
- **"Device offline" that a refresh cured**: the Supervisor's and the relay's
  64-message socket queues overflowed in bursts and dropped the tunnel. They now
  hold 8192 messages, the Supervisor writes queued messages in batches, waits 15
  seconds instead of 5 before reconnecting and tells the relay when it gives up.
  A full relay queue answers busy (429), not offline; the Web UI asks once more
  for a read that meets 429 or 503, and clears a connection error once the
  thread loads again.
- **No background password prompts**: the 15-second device-protection check
  that asked to unlock the browser is gone. A device whose protection is not
  finished shows **Finish protection**, done when you press it.
- **Phone composer**: the tap that opens the folded composer only opens it, even
  when the open layout puts send, the model or attachments under the finger.
- **Upstreams**: a test, sync or save result shows on its connection, an apply
  result on its harness, an advanced-panel result under its buttons. Being used
  by a harness no longer moves a connection to the top or paints it green; the
  harness tags are small enough for several. DeepSeek Harness is offered
  connections that serve only the OpenAI Responses API (written as pi-ai
  `openai-responses`); where Chat Completions or Messages is served it keeps
  using that.

Validated with the supervisor, relay and runtime Rust tests, the Web and shared
UI tests, and browser checks in an isolated environment at phone and desktop
widths in every theme: tab switches measured frame by frame with 500 ms of
added latency, and a slow tap on the folded composer at every position.
