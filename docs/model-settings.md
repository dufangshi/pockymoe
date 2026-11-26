# Model and reasoning effort

A thread's model and reasoning effort are chosen in the composer's model picker,
opened from the model key (a popover on desktop, a bottom sheet on phones):

- A search box with a Harness settings icon beside it, then the models of the
  agent in view under a header with the
  agent's mark and name. Rows show the model's context window and its price per
  million input and output tokens from the device's pricing catalog
  (`GET /api/config/model-pricing`, matched like the cost estimate); a model the
  catalog does not know leaves those cells empty. The current model is checked
  and scrolled into view.
- The current model's effort below the list: one row of chips with the level
  names, the chosen one in its effort colour; ←/→, Home and End choose.
- For the thread's owner, a rail of the device's ready agents on the left, the
  current one first and marked with a dot; ←/→ move between agents. Picking a
  model of another agent moves the thread to it (see
  [Switching harnesses](harness-switching.md)). Typing in the search lists
  matching models of every agent, grouped by agent.
- The settings icon beside the search opens Harness settings: the harness plugin
  snapshot and DSH panel on desktop and phone, including Treer scoped surfaces.
  It has no access-level control or capability notice.

Both model selectors use `GET /api/threads/{id}/models` and write through
`PATCH /api/threads/{id}/settings`; other agents' models come from
`GET /api/agent-runtimes/acp/models?agentId=…&cwd=…`.

## Model discovery and refresh

Existing threads use `GET /api/threads/{id}/models`; the creation form uses
`GET /api/agent-runtimes/{provider}/models?agentId=…&cwd=…`. Both reach the
same runtime discovery path. The Web requests these lists again when opening
its selectors; it does not maintain a static model table.

| Harness | Source without an applied saved connection |
| --- | --- |
| Codex | codex-acp's config options and model state, built from app-server `model/list` |
| Claude | claude-agent-acp session config options (the harness/SDK supplies its models) |
| DeepSeek Harness | DSH's own ACP config options and harness info, preserving provider/model tuples |
| Cursor | ACP config options, or `cursor/list_available_models` |
| Grok, OpenCode | ACP session options/model state, with `grok models` / `opencode models` as command fallbacks |
| Gemini, Copilot, custom ACP | ACP session options/model state |

A live session's model snapshot lasts for that session. Without one, discovery
opens and closes a prompt-free ACP probe when a cwd is supplied. Learned Claude
efforts and visited-model markers live in memory per harness (and isolated
environment generation), until Supervisor exit. They are not
a persistent model catalog and have no binary-version expiry.

For an explicitly applied saved connection, except DSH, the runtime requests
that upstream's model endpoint instead. Its in-memory cache is scoped by harness
and a fingerprint of the entire applied profile, expires after 60 seconds, and
is invalidated on harness restart/apply or legacy upstream discovery. Grok's
native capability probe also expires after 60 seconds and includes profile and
model metadata in its revision. DSH continues using its projected native catalog.
The saved connection's recorded metadata in `<database>.upstreams/connections.json`
is separate: it records the last explicit discovery, without time expiry; an
endpoint/protocol edit discards it. It is not the runtime's 60-second list cache.

Codex API-key mode can return only its bundled catalog even when the provider
serves newer IDs. Refreshing that same catalog cannot discover those IDs.
For an applied saved connection, Pockymoe merges upstream model IDs into the
owned app-server bridge's `model/list` before codex-acp validates them. Native
entries retain their metadata; discovered entries use declared effort metadata
when available. Before switching, Pockymoe asks codex-acp to refetch the catalog,
then applies the model config option, so an existing session can select newly
advertised models. The bridge uses its own 60-second profile-bound discovery
cache. No prompts are sent during discovery or switching.

This merge applies to local owned Codex bridges, not isolated harnesses or
unbound native/OAuth configurations. Save and explicitly apply the actual
Responses connection to enable upstream discovery; merely saving another
connection must not change a harness's routing or models. The upstream must list
the concrete ID on its model endpoint. A provider advertising an ID does not
prove that account can successfully complete a request with it. Codex accepts
unknown IDs using fallback model metadata; provider compatibility and access
still determine whether a turn succeeds.

## Effort levels per model

Each model lists the efforts it supports. An ACP session's effort option
(`thought_level`) only covers the model the session is on, and Claude's levels
differ by model (for example Opus 4.7 has `xhigh`, Sonnet 4.6 stops at `max`,
Haiku has none). The runtime therefore learns each model's levels:

- from the config options of every session it opens or holds, for the model each
  is on;
- for Claude (`effort_options_follow_model` in `acp/adapter.rs`), from a probe
  session that switches through the models no session has shown yet. It sends no
  prompt and runs at most once per model per Supervisor process: in the
  background when a live session supplied the list, inline when the list came
  from a probe anyway.

A model not learned yet keeps the current model's levels. The Web fetches the
list again when the model picker or Harness settings opens and after a switch,
so learned levels show up there.

DSH's ACP model values are opaque `[provider, model]` tuples. The provider is
part of routing and is preserved when looking up a readable name in the shared
Relay catalog. Shared short names (for example `Opus 5.5`) take precedence over
endpoint display names in model menus and DSH's projected definitions. DSH labels
retain their provider prefix, and gateway variants retain their suffix. Endpoint
context limits and explicit effort capabilities remain authoritative. An
explicit empty endpoint effort list disables the control. The menu then uses
DSH's actual per-model capabilities; it does not offer additional levels that
DSH would reject. A selected model's default effort is not written as a default
for the entire provider: sibling models may support different levels.

Some gateways expose fixed-effort IDs such as `gemini-4-argon-high`. Exact IDs
and account aliases take precedence. Reference pricing and short labels otherwise
fall back to the longest known model after removing trailing hyphen-separated
modifiers, such as `deepseek-v4.1-flash-abliterated`. The variant remains a separate
selectable wire ID. This reference match does not establish native reasoning
capabilities: only a recognized effort suffix explicitly supported by the known
base supplies an effort hint, and a fixed-effort DSH variant does not inherit
another effort selector.

## Changing them during a turn

The composer and Harness settings window both accept a change while a turn runs, and it goes to
the live session at once. When it takes effect depends on the harness, reported as
`controls.liveModelSwitch` in the thread's capabilities:

- **Claude** (`liveModelSwitch: true`): the running query takes the new model or
  effort from its next model call, inside the current turn.
- **Codex on the native protocol** (`liveModelSwitch: true`): the running turn
  takes a new effort or fast tier from its next model request; a new model too
  when Codex accepts the switch (it refuses models it has no metadata for, or
  that change the turn's safety policy). A refused change, and any change in
  an isolated environment, applies from the next turn.
- **Others** (Codex through ACP and the rest): the harness keeps the choice for the next turn.

While a turn runs, the model picker and Settings say which applies.

## What a turn shows

A turn starts with the thread's model and effort. Usage reports then record what
actually ran: the turn's `model` is the last model its own agent called, and
Claude usage carries `tokenUsage.models`, the runs of consecutive calls per model in
order (subagents' calls are excluded). A turn with one run shows
`sonnet 4.6 · max`; a turn switched mid-way shows `opus 4.7 → sonnet 4.6 · max`,
and tapping it lists the calls each model made. An effort changed mid-turn becomes
the running turn's effort, since Claude's usage reports do not carry it. Turns
recorded before `tokenUsage.models` existed show their single model.

See [Switching harnesses](harness-switching.md) for changing an existing thread’s agent with a preview and portable history.
