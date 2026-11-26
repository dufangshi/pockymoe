# Switching a thread's harness

Open the model picker, choose **Switch harness**, select a harness and model,
and preview the history before confirming. A running thread offers **Stop and
switch**. Clear an active or paused goal first. Plan, sandbox, fast mode and
reasoning effort reset; the approval mode stays in effect. Harnesses unavailable
in an isolated execution environment are refused.

Each harness retains its own native session. Returning to a harness resumes its
saved session and transfers only journal turns after its last use. The thread's
identity and public conversation stay together; turns keep the provider, agent
and model that produced them. A compact divider shows the agents and catalog model names at each change. Its info
control shows the full names and settings reset note on hover, focus or tap.

## Portable history

The next prompt receives a hidden prefix built from saved public user/assistant
text, command outcomes, file-change summaries and current Git status. Reasoning,
attachments and raw tool state do not transfer. Delegate contracts are delivered
again. The original request, recent turns and failed command summaries have
priority. Large original turns can be abbreviated; omitted history remains
available with:

```sh
pockymoe transcript THREAD --view summary
pockymoe transcript THREAD --turn TURN_ID --view overview
```

The prefix defaults to 16,000 estimated tokens. The estimate counts one token per
UTF-8 byte. The model catalog's context window is used when known, otherwise
128,000 tokens, reserving at least 16,000 tokens or 25% for tools and instructions.
The user's next prompt is never shortened; a prompt that exceeds the allowance
is rejected. A preview describes the saved history at preview time; a running
turn can add text before Stop and switch finishes.

## CLI and API

Use the owner's local machine CLI credential (thread-scoped agent credentials
cannot switch harnesses):

```sh
pockymoe thread switch THREAD --agent claude --model MODEL --preview
pockymoe thread switch THREAD --agent claude --model MODEL
pockymoe thread switch THREAD --agent codex --model MODEL --reasoning-effort high
```

The provider defaults to `acp`. HTTP endpoints are owner-only:

- `POST /api/threads/{id}/harness/preview` and `/harness/switch`, with
  `{provider, agentId, model, reasoningEffort}`. The preview returns the exact
  prefix, byte/token estimate, included turn count, omitted turn IDs and whether
  it resumes a session.
- `POST /api/threads/{id}/harness/fork`, with `{target: {provider, agentId,
  model, reasoningEffort}, turnId?}`, copies journal history through the selected
  turn into a new thread, using portable history instead of a foreign native fork.
- `GET` / `PUT /api/harness-handoff/settings`, with `{tokenCap}` on PUT,
  changes the device's history cap (1,024–64,000).

Switching an imported thread adopts its durable journal for subsequent turns, so
its new harness does not require connecting to the old imported session first.
Harness session positions and turn identity survive Supervisor restarts. If a
saved native session has been removed outside Pockymoe, its harness may refuse
to resume it. External effects from earlier turns are never undone by switching.
