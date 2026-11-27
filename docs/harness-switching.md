# Switching a thread's harness

Changing a thread's agent is picking a model of another agent in the composer's
model picker ([Model and reasoning effort](model-settings.md)); there is no
separate step. Choose the agent on the picker's rail, or search, and its models
are listed. A short note above the foot says what moves with it: "Hands over N
turns · ≈T tokens", or "Resumes its session · N new turns" when the agent was used
in this thread before, with **Preview** (the exact text; its Git state reads as a short summary there,
while Copy all and the agent get it unchanged), and one line on what
resets. Picking a model of an idle thread switches it at once; the picker closes
and the timeline shows a divider at the next prompt. While a turn runs, a pick
is marked and **Stop and switch** interrupts the turn, waits for it to settle and
switches. A refused switch (an active or paused goal must be cleared first)
keeps the picker open with the reason on its mark.

Codex and ACP Codex (or Claude and ACP Claude) are one agent on two protocols
([native harness protocols](native-harnesses.md)): switching between them keeps
the session, its history and every setting, hands nothing over and records the
change in the timeline. The rest of this page is about switching to another
agent.

Plan, sandbox, fast mode and reasoning effort reset; the approval mode stays in
effect. The effort slider under the note shows the target model's levels (the
pending pick's, else the agent's default model's); the level chosen there
applies with the switch when the picked model offers it, else its default. Harnesses unavailable in an
isolated execution environment are refused. The rail lists only agents ready on
the device and is shown only to the thread's owner; a split pane offers the same
picker for its own thread and device.

To keep the original thread, fork instead: the composer's fork panel (`/fork`)
has a **Fork into** row of agents. Forking into another agent copies the history
through the latest or a chosen turn into a new thread on that agent's default
model, with the portable history below. The Harness settings window holds only
the harness's own settings.

Each harness retains its own native session. Returning to a harness resumes its
saved session and transfers only journal turns after its last use. The thread's
identity and public conversation stay together; turns keep the provider, agent
and model that produced them. Each switch is saved immediately as a standalone
transcript entry, including repeated idle switches and switches before the first
prompt. Entries survive reload and export and do not count as model turns or change
handoff deltas. A compact divider shows the agents and catalog model names at each
change. Its info control shows the full names and settings reset note on hover,
focus or tap.

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
  `{provider, agentId, model, reasoningEffort}`; `agentId` is an agent key
  (`codex`, `acp:codex`, `opencode`). The preview returns the exact
  prefix, byte/token estimate, included turn count, omitted turn IDs and whether
  it resumes a session.
- `POST /api/threads/{id}/harness/fork`, with `{target: {provider, agentId,
  model, reasoningEffort}, turnId?}`, copies journal history through the selected
  turn into a new thread, using portable history instead of a foreign native fork.
- `GET` / `PUT /api/harness-handoff/settings`, with `{tokenCap}` on PUT,
  changes the device's history cap (1,024–64,000).

Switching an imported thread adopts its durable journal for subsequent turns, so
its new harness does not require connecting to the old imported session first.
Thread detail responses include `harnessSwitches`, an ordered list of standalone
`harnessSwitch` history items. Each item has a stable ID, `createdAt`,
`harnessSwitch` metadata and an `afterTurnId` anchor (`null` before the first
prompt). Existing switch markers on old user messages remain supported.

Harness session positions and turn identity survive Supervisor restarts. If a
saved native session has been removed outside Pockymoe, its harness may refuse
to resume it. External effects from earlier turns are never undone by switching.
