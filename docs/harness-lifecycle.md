# Harness lifecycle: loading and release

Each loaded thread owns one harness process tree: the ACP adapter the
Supervisor spawns (`codex-acp`, `claude-agent-acp`, `grok agent`, `dsh`, …) and
everything it starts. A Codex tree is `codex-acp` → our `app-server` bridge →
the npm `codex` launcher → the native `codex app-server`; a Claude tree is
`claude-agent-acp` → the bundled `claude` binary → one copy of each configured
MCP server. A live native Codex process holds the thread's writer lock, so the
Codex app reports the thread as in use until the process exits.

The runtime records each load on its session (`acp/release.rs`); the
Supervisor decides when to release (`service/harness_release.rs`).

## When a harness loads

| Load reason | Trigger |
| --- | --- |
| `create` | A new thread |
| `view` | `GET /api/threads/{id}/capabilities` without `load=false` (the thread page), a harness panel action |
| `turn` | A prompt submitted while the thread was idle, a goal |
| `queue` | A prompt that waited behind a turn, or came from another thread or the CLI |
| `automation` | An automation run |
| `resume` | Resume/Connect, recovery of a turn interrupted by a Supervisor update |
| `fork` | Loading the source of a fork; a Claude/Grok fork served by its source's process |
| `other` | Anything outside these paths |

Reading thread details and history, `capabilities?load=false`, settings
changes on an unloaded thread and native subagent views never start a process.
The record also keeps the load time, the last activity (prompt, steer, turn
end, approval, goal or harness action), whether a prompt was sent since the
load, and the adapter's root PID.

## What keeps a harness loaded

A harness is never released automatically while a page displays its thread or
while it is busy. Busy reasons (`HarnessBusyReason`):

- `turn`: a running turn, or a thread that is `running` or `recovering`;
- `pendingRequest`: an approval or question waiting for an answer;
- `queuedInput`: queued prompts that will be delivered (held CLI steers do not
  count), or a steer being handed to the running turn;
- `backgroundTask`: Claude SDK background tasks (Bash, Monitor, async Agent;
  see [Claude background waits](claude-background-wake.md)), DSH work outside a
  turn, ACP terminals still running, or any process in the tree that is not the
  adapter, agent, bridge or an MCP server, such as a dev server or build the
  agent started;
- `subagent`: subagents of the running turn, or Claude/Codex
  [native subagents](native-subagents.md) still marked running;
- `goal`: an active goal.

An operation that loads or uses a harness (turn admission, capability load,
reconnect, fork, settings change, compact, goal control) holds the thread while
it runs, so a release never ends a process between its check and its request.

## Release rules

**Viewing.** A harness loaded with reason `view` and never prompted is
released 10 seconds after the last page displaying the thread stops, provided
the viewer left after the load started and the thread is not busy. Showing it
again or prompting it within those 10 seconds cancels the release. A prompted
thread follows the idle rule.

**Idle.** Every minute, a thread that is not displayed, not busy and idle for
the configured time (default 10 minutes) is released. Idle time counts from the
last activity or from the moment the last page stopped displaying it,
whichever is later.

**Limits.** After every load and every minute, if more threads are loaded than
`maxLoadedThreads`, or the harness trees use more than
`maxHarnessMemoryBytes`, the least recently used threads that are neither
displayed nor busy are released (reason `limit`). The load that triggered the
check and loads younger than 30 seconds are kept. Limits never block a
prompt: without a releasable thread the Supervisor stays over the limit and the
monitor reports `overThreadLimit` / `overMemoryLimit`.

**Shared processes.** A Claude or Grok fork is served by its source's process
until it is loaded on its own. Such a process is released only when every
session on it may be released, and all of them are released together.

**Explicit, delete and close.** `POST /api/threads/{id}/release` releases at
once (reason `explicit`), including busy threads other than a running turn.
Deleting a top-level thread stops its running turn and releases it (`deleted`);
closing an agent thread releases it (`closed`); deleting a child thread
releases it as before. Delete and close end only that thread's session when it
shares a process. A load that no longer belongs to any thread is released once
settled and a minute old. Restarting a harness from Settings and DSH Restart
report `explicit`. `POST /api/threads/{id}/disconnect` keeps its old behavior and
does not release anything.

## Release action

1. When the adapter advertises `session/close`, send it for each session
   (codex-acp then unsubscribes the native thread and drops its writer lock).
2. Send SIGTERM to the adapter's process group, wait up to 2 seconds for the
   adapter to exit, then SIGKILL the group. Windows ends the process tree.
   Commands the agent started in the group end with it; ACP terminals the
   Supervisor ran for that process are stopped too.
3. Remove the sessions and emit `thread.runtime.released`.

The next request that needs the session resumes it as usual, a few seconds
slower, with fresh MCP servers.

## Settings

`HarnessResourceSettingsDto`, stored in the Supervisor database
(`harness_resource_settings`, migration 17). `null` disables a rule.

| Field | Default | Valid values |
| --- | --- | --- |
| `idleReleaseMinutes` | 10 | 1–1440 |
| `maxLoadedThreads` | `null` | 1–200 |
| `maxHarnessMemoryBytes` | `null` | at least 256 MiB |

Saving applies the rules at once.

## Interfaces

Types: `crates/protocol/src/harness_resources.rs`, mirrored in
`packages/shared/src/harnessResources.ts`. Relay paths add the usual
`/relay/devices/{id}` prefix.

| Method and path | Result | Access |
| --- | --- | --- |
| `GET /api/device/processes` | `HarnessProcessesDto`, sampled on request, cached 2 s, `Cache-Control: no-store` | Owner and device grants, like `/api/device/metrics`; thread and workspace grants get 403 |
| `GET /api/harness-resources/settings` | `HarnessResourceSettingsDto` | Same |
| `PUT /api/harness-resources/settings` | The saved settings; invalid values return 400 `invalid_settings` | Owner and device grants with control |
| `POST /api/threads/{id}/release` | `ReleaseThreadResultDto`; body `{"interrupt": true}` interrupts a running turn first, otherwise a running turn returns 409 `turn_running`; an unloaded thread returns `released: false` | Like `/interrupt`: thread control |
| `GET /api/threads/{id}/capabilities?load=false` | Session capabilities when loaded, provider capabilities otherwise, without starting a process | Unchanged |

WebSocket (`/ws`, and relay clients through the same session code):

- The client sends `{"type":"thread.view","threadIds":[…]}` with the complete
  set of threads the page displays whenever it changes, after reconnecting, and
  an empty list once the tab has been hidden for 60 seconds. Read-only reference
  panels are not listed. The Supervisor keeps the set per connection and clears
  it when the connection, relay client or tunnel ends. Shared connections count
  only threads their grant can read. At most 64 IDs are kept.
- The Supervisor emits `thread.runtime.loaded` with `{loadReason}` and
  `thread.runtime.released` with `{reason}` (`view`, `idle`, `limit`,
  `explicit`, `deleted`, `closed`).

## Memory measurement

`HarnessProcessesDto.metric` names the measure. Linux reads PSS from
`/proc/<pid>/smaps_rollup`, so shared pages are divided among processes and the
sums are meaningful; if any process cannot be read, every value falls back to
RSS. Other platforms report RSS from `sysinfo`. Each loaded thread lists its
tree: the root is the `adapter`; `claude` and native `codex` binaries and
`codex-*` helpers other than `codex-linux-sandbox` are `agent`; our `app-server` bridge and the npm `codex`
launcher are `bridge`; a process whose command line contains `mcp`, and its
descendants, is `mcp`; everything else is `tool`. A process name is the
executable plus one short distinguishing argument, never the command line,
which may hold secrets. `harnessBytes` counts each process once; `other` lists
Supervisor descendants outside loaded threads (probes, shells, ACP terminals).

## Tests

`cargo test -p pockymoe-runtime release` covers the rules, display state,
settings, the scripted ACP release (`tests/fixtures/release_agent.py`) and
process classification. `cargo test -p pockymoe-supervisor processes` and
`thread_view` cover grants, the WebSocket message and the HTTP endpoints.
