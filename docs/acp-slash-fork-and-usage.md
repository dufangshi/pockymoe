# ACP slash commands, forks and Grok usage

The Rust runtime keeps ACP as the conversation transport. Harness adapters fill
capability and command differences; the Node supervisor is not needed.

## Session toolbox

`available_commands_update` can arrive before `session/new` or `session/load`
returns, and can change between prompts. The ACP multiplexer stores the latest
list by process ID and raw session ID, independently of the running turn.
`GET /api/threads/{id}/capabilities` returns that session's negotiated capabilities
and toolbox. The page refreshes this snapshot after the thread's turn and
connection events and shortly after the session opens
([web-live-updates.md](web-live-updates.md)).
Native slash commands remain ACP prompts; product actions such as fork use their
structured HTTP action. A command advertised by one session does not enable it
in another session or in the provider's default capabilities.

Individual `/$skill` invocations are excluded from the toolbox in both the
adapter and the shared UI (including responses from older supervisors). Skills
remain callable in prompts. Fork entries also require the host to provide an
authorized action handler. Fork errors remain visible in the menu for retry,
and navigating between fork panels cannot clear an in-flight operation lock.

## Fork support

| Harness | Latest fork | Selected completed turn | Transport |
| --- | --- | --- | --- |
| Codex | Yes | Yes | Thin bridge to the app-server owned by codex-acp |
| Claude Code | Yes | No | Standard ACP `session/fork` |
| Grok Build | Yes | No | `_x.ai/session/fork`, then ACP `session/load` |

Codex ACP currently does not implement `session/fork`. The Rust bridge runs as
codex-acp's `CODEX_PATH`, forwards its stdio unchanged, and adds an authenticated
loopback control connection. Only `thread/fork` and read-only `thread/turns/list`
are allowed on this connection. Prompt, load, cancel and all conversation events
still belong to ACP. The source is never loaded by a second native app-server.
Historical forks use the selected native `lastTurnId`; they do not roll back or
modify the parent. This requires native Codex support for these app-server APIs.

Fork and prompt take an exclusive operation lock on the source session. Forks
inherit product settings, receive a new provider session ID and local thread,
and can be loaded after a supervisor restart. `forkAt` is advertised separately
from latest fork; Claude/Grok do not show a historical-fork action they cannot
perform. These forks share the workspace directory, as native conversation
forks do; they do not create a Git worktree.

## Grok token costs

Grok reports live response usage through `_x.ai/session_notification`; saved
history uses `_x.ai/session/update`. Its adapter converts both to the common
usage representation. Live response deltas include cache reads in canonical
input totals. Final turn usage already includes those reads and replaces the
accumulated snapshot, so it is not billed twice. Reasoning remains part of output
tokens. Subscription allowance (`_x.ai/billing`, the 7d badge) is separate.

When old Grok turns have no usage, the history reader matches completed native
updates to local turns by prompt and timestamp and persists the missing usage.
Incomplete JSONL records are ignored. Cost estimates use the existing model
pricing catalog and its user overrides; native logs must still be available.

## Verification

Rust tests cover protocol mapping, usage normalization, history recovery, command
updates, HTTP actions and capability access.

After `cargo build -p pockymoe`, `node scripts/verify-acp-forks.mjs` runs real
harnesses against an isolated database. It verifies inherited context, parent
isolation, Codex historical boundaries, restart continuation and Grok usage
backfill. It requires installed/authenticated harnesses and makes small real
model requests. `FORK_TEST_AGENTS=codex,claude,grok` selects the harnesses and
`FORK_TEST_PORT` overrides the test port. Evidence is saved under `.local/`.
