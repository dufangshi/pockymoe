# Model and reasoning effort

A thread's model and reasoning effort are chosen in the composer's model menu or
in Settings → Session. Both show the same list from `GET /api/threads/{id}/models`
and write through `PATCH /api/threads/{id}/settings`.

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
list again when the model menu or the session settings open and after a switch,
so learned levels show up there.

## Changing them during a turn

The composer and Settings both accept a change while a turn runs, and it goes to
the live session at once. When it takes effect depends on the harness, reported as
`controls.liveModelSwitch` in the thread's capabilities:

- **Claude** (`liveModelSwitch: true`): the running query takes the new model or
  effort from its next model call, inside the current turn.
- **Others** (Codex and the rest): the harness keeps the choice for the next turn.

While a turn runs, the model menu and Settings say which applies.

## What a turn shows

A turn starts with the thread's model and effort. Usage reports then record what
actually ran: the turn's `model` is the last model its own agent called, and
Claude usage carries `tokenUsage.models`, the runs of consecutive calls per model in
order (subagents' calls are excluded). A turn with one run shows
`sonnet 4.6 · max`; a turn switched mid-way shows `opus 4.7 → sonnet 4.6 · max`,
and tapping it lists the calls each model made. An effort changed mid-turn becomes
the running turn's effort, since Claude's usage reports do not carry it. Turns
recorded before `tokenUsage.models` existed show their single model.
