An agent can now stop a delegate that is on the wrong path and take back the
messages it queued for it.

- **`pockymoe thread stop NAME --reason "..."`** interrupts what a delegate is
  running, like the Stop button. `--clear-queue` first withdraws the messages
  the caller queued behind it, so the stop is not followed by stale work. The
  delegate keeps its history and runs again on its next message.
- **`pockymoe thread queue NAME`** lists the messages waiting for a delegate,
  oldest first, with ids, senders and a preview.
- **`pockymoe thread unqueue NAME MESSAGE_ID...`** (or `--all`) withdraws waiting
  messages before they run.
- **Limits**: a thread controls only threads under it in its lineage, never
  itself, a parent, a sibling or another conversation, and withdraws only
  messages it sent. Your own prompts and other senders' messages stay in the
  queue. Requests from other devices are refused.
- `pockymoe guide messaging` describes when to steer, when to stop, and how to
  replace a delegate's work.
- **Claude adapter 0.104**: Claude threads and the model list work again with
  `claude-agent-acp` 0.104, which had failed with "Probe Claude Agent models: No
  such file or directory". A later adapter change of this kind now only turns
  off the detailed context analysis for Claude instead of stopping sessions.
