# Native harness protocols

ACP remains the default harness protocol. A device can instead connect directly
to Codex's `codex app-server` JSON-RPC protocol. Both paths use Codex's own rollout
store and keep the same `codex::` session identifiers. Release the loaded harness
before changing protocols for an existing conversation; the next load uses the
new selection. A setting change never moves an active session between processes.
Loaded threads also keep their backend's model list and capabilities. The
Supervisor caches the selection for all its routers and refreshes it after a
successful settings API update; manual file edits take effect after a restart.
Unowned sessions with unrecognized prefixes use ACP, including on a bound provider.

In Settings → Resources, choose ACP or Native for each harness.

The device owner can also read or replace the selection through
`GET` / `PUT /api/harness-protocols/settings` with
`{"codex":"acp","claude":"acp"}` (each value accepts `acp` or `native`).
The settings file sits beside the Supervisor database, with the extension
`harness-protocols.json`. Missing settings default to ACP. Shares cannot access
this endpoint. A native backend must be registered for the selected harness;
otherwise it continues through ACP.

The direct Codex backend supports thread start/resume/fork, rollback, streaming
turns, image inputs, model and effort selection, plan mode, steering, interrupt,
approval requests and explicit compaction. Its Rust process transport uses the
same execution-context and process-cleanup implementation as ACP. Native mode
is experimental: transcript and lifecycle parity must be validated before making
it the default. Keep ACP selected when a native feature is unavailable.

Native Codex also exposes goals and uses the same local rollout reader as ACP
for per-request billing, including subagent requests. Context occupancy streams
from the native token counter; opt-in host context inspection uses the existing
local trace collector. Isolated execution uses environment-scoped session IDs
and the native token counter rather than reading host rollout files.

Both native runtimes emit the turn lifecycle events ACP emits
(`thread.turn.started`, the turn's completed items, `thread.turn.completed` with
the same status vocabulary). Generation speed, per-request context steps, relay
activity, outside-agent MCP events and the desktop map depend on them.

Plan allowance (5h/7d windows) is the device account read described in
[model prices and subscription usage](model-pricing-and-subscription-usage.md#oauth-allowance).
During a turn, native Codex also forwards the app-server's
`account/rateLimits/updated` pushes and the rollout's recorded limits, so the
composer meter updates while the turn runs; ACP gets the same windows from the
rollout. Token usage, context and cost come from the same rollout reader and
match ACP request for request.

Extended items render as codex-acp renders them: MCP calls as
`mcp.<server>.<tool>` tool rows with their arguments and result, dynamic tools
with their content, web searches titled by query or page, viewed images as file
reads, generated images with the revised prompt and saved path, and a finished
review as the reviewer's answer. Sleep, function-output, hook-prompt and
review-start bookkeeping items stay hidden. Blocking or stopping hooks, model
reroutes, configuration warnings and deprecation notices appear as notices.
Explicit compaction holds the session until Codex reports it finished, like
ACP's hidden `/compact` turn. Command output streams into the row while the
command runs, including what the agent types into an interactive command.

Approval cards offer the decisions codex-acp offers: allow once, allow for the
session, allow commands with the same prefix from now on, allow or block a
network host, decline and continue, or decline and stop the turn; permission
requests can be granted for the turn, for the turn with strict review, or for
the session. Guarded escalations always ask the user (the approval reviewer is
the user, never Codex's own reviewer). A card closes when Codex settles the
request itself or the turn ends. `/review`, `/review-branch <branch>` and `/review-commit <sha>` start
Codex's review mode; `/mcp`, `/skills` and `/status` answer from the
app-server without a model call. A Plan Mode turn that produces a plan asks
"Implement this plan?"; yes leaves Plan Mode and runs the implementation in the
same turn, as on ACP.

Beyond ACP, native Codex changes the effort, fast tier or (when Codex accepts
it) model of a running turn from its next model request
(`controls.liveModelSwitch`; the app-server starts with Codex's
`step_model_switching` feature; ACP rejects such a change), shows retrying
connections, Codex warnings and MCP servers that failed to start as notices, reports plan
allowance live, and rewinds in place. The full method-by-method coverage
(every app-server method, server request and notification, with the ACP
behaviour and the test that proves it) is kept with the audit evidence; its
summary: 37 handled as before, 18 fixed or added in the 2026-11-26 audit, one
minor gap (MCP tool progress text is not shown), and 225 not applicable to a
thread runtime (account sign-in, Codex app organisation, plugins, voice,
filesystem and process APIs Pockymoe provides itself).

On ACP, Codex keeps the same product guarantees: guarded selects codex-acp's
`workspace-write` mode (the user reviews escalations; its `agent` mode is Codex's
Auto review), Plan Mode sets codex-acp's `collaboration_mode` option, and when
codex-acp leaves Plan Mode after an approved plan the thread follows it. The
Codex bridge refuses codex-acp's ephemeral title threads, so ACP spends no model
requests the transcript does not show (Pockymoe keeps its own titles).

Client-defined dynamic tools, external token refresh and attestation callbacks
are unsupported and receive a protocol error. MCP elicitation currently
supports forms. Agent catalog installation metadata still describes the ACP
adapter, even when the native runtime is selected.

`CODEX_COMMAND` selects the host Codex command; isolated execution resolves
`codex` inside the environment. Upstream credentials remain in the existing
harness configuration or isolated launch environment. Protocol settings contain
no credentials.

### Isolated environments

Native Codex runs `codex app-server` through the environment worker with the
same projection as codex-acp: the API broker's short-lived capability and
worker-written provider configuration, or an approved native file login. A file
login starts the image's `codex-subscription` launcher (ChatGPT, file store,
`openai` provider) and requires Codex 0.167.0, as ACP requires codex-acp
2.5.0. Native forks are hidden, like ACP. The image's Codex 0.167.0 sends no
tools for models it does not know (for example `gpt-6.1-sol`) on either
protocol; choose a model that release knows, or rebuild the image.

Codex releases that expose `thread/revert` use paginated native turn IDs for
rewind. This preserves Codex history boundaries without rewriting rollout files.

Regenerate the checked-in protocol schema and Rust wire structs with
`scripts/generate-codex-schema.sh` using the installed Codex binary. The generator
includes experimental fields for collaboration and native settings.

## Claude Code

The native Claude backend speaks Claude Code's stream-json control protocol
straight from Rust (`--input-format stream-json --output-format stream-json`),
without the Agent SDK or a Node bridge. `CLAUDE_COMMAND` selects the executable.
It uses the same `claude::` identifiers and Claude JSONL session store as ACP;
release the loaded session before switching protocols. Resume and fork preserve
that store. Forking at an ACP reply resolves its API message id to the persisted
Claude message UUID.

The backend streams text and thinking, maps tool calls to the shared transcript
items, forwards TodoWrite plans and subagent attribution, and accepts image
attachments. It discovers models at initialization and uses control requests
for model, effort and permission-mode changes. Guarded requests use the normal
thread approval UI. Approval replies route by their native backend prefix even
when the device protocol setting changes; stale requests return that backend's
validation error. Interactive questions and plan exits still require a human
in full-access mode. Read-only callbacks reject execution and write tools.
Interrupt, steering, compaction, idle release and crash detection use the native
connection. An unexpected EOF marks execution uncertain and never fabricates a
successful turn. A restarted process resumes the same native session.

Native Claude is experimental. In an isolated environment it starts the
image's `claude` through the environment worker and takes exactly the ACP
adapter's route (`acp/isolated_claude.rs`): the API broker capability or the
approved native file login, pinned both in the process environment and in the
CLI's `--settings` tier, which is where the Agent SDK puts `options.env` and
`options.settings`. API-key, OAuth-token, helper and alternate-provider
selectors are cleared; project rules, tools and settings sources are kept. The
CLI must report Claude Code 2.1.318 (the image's Agent SDK 0.3.318 binary) and
the thread model must be one it advertises. Host credentials, the host MCP
registry and host Claude homes are never used; native forks are hidden, like
ACP. A host native Claude process stops through its Host guardian, whose
receipt is the proof a migration into an environment requires.

The runtime registers one SDK callback hook, PreToolUse for Write, which reads
the file just before Claude overwrites it (through the worker inside an
environment) so the transcript diff is the real overwrite, as claude-agent-acp's
preview hook does. It never blocks or changes the tool. SDK-hosted MCP servers
are not implemented; other control callbacks fail closed. Claude's own
configured subprocess MCP servers, hooks, agents and project instructions
continue to load normally, and the device MCP registry is passed at launch.
Autonomous work outside an active prompt and complete TaskCreate/TaskUpdate
plan projection remain qualification requirements. Keep ACP selected for those
workflows. `rewind_files` is deliberately not exposed: the product's Git
checkpoints own file restore and its undo boundary.

Native usage deduplicates assistant snapshots by API message id, keeps child
usage in billing, and keeps the latest parent response as context occupancy.
When Claude reports the context window in the turn result it updates the normal
context meter. TaskCreate/TaskUpdate/TaskList results project the known task list;
Edit tool results carry the exact replacement snippet as a diff. A Write is
diffed against the contents read by the PreToolUse hook (empty for a new file);
a file it cannot read faithfully (binary, over 1 MiB, outside the environment's
working directory) shows the write without an invented diff.

## Promotion gate

ACP stays the default. Native can become the default for a harness only when
every check below passes for that harness on the release candidate, with the
evidence kept with the release notes. Status as of 2026-11-26:

| Check | Codex | Claude |
| --- | --- | --- |
| Shared session store: ACP → native → ACP on one thread, same session ID, history recalled | pass | pass |
| Tool use, file edit with diff, guarded approval, interrupt, steering, resume after a Supervisor restart | pass | pass |
| Rewind (native rollback or fork-swap) and Git turn checkpoints | pass | pass |
| Isolated environment: API broker, restart and resume, rewind, guarded approval, protocol switch on one thread, no host credentials or homes | pass (scripted upstream through the real broker) | pass (real model with a native file login; scripted upstream through the real broker) |
| Isolated environment with an approved native file login | gated, not run (no ChatGPT file login on the lab host) | pass |
| Turn lifecycle events identical to ACP (`thread.turn.started` / `completed`) | pass | pass |
| Token usage, context, cost and plan allowance identical to ACP for the same prompts; allowance live during a turn | pass (scripted upstream, real CLI) | not compared side by side |
| Plan and performance modes reach the model | pass (Codex Plan Mode instructions, fast service tier) | pass (plan) |
| Every app-server method, server request and notification classified against codex-acp, with a test for each mapped one (2026-11-26 audit) | pass: 281 entries; one minor gap (MCP tool progress text) | see the Claude audit |
| Same scripted prompts side by side with a real model: tools, edit, image, sub-agent, interrupt, `/status`, `/mcp`, `/review`, compaction, restart, escalation approval, Plan Mode | pass: same outcome on every step; every remaining difference explained (native shows sub-agent rows; ACP's Guardian auto-review, missing Plan Mode and title requests were fixed after the audit) | see the Claude audit |
| Extended items render as on ACP (MCP, web search, images, review, hooks, notices) | pass in mapping tests; review mode in the real lab; no real MCP server or image-model run | not applicable |
| AskUserQuestion, explicit compaction, Write diffs in a real lab | compaction in the real lab | pass |
| SDK callback hooks and SDK-hosted MCP | not applicable | partial: Write preview hook; no SDK MCP |
| Background work outside a prompt | not run | not qualified |
| A full release matrix (macOS, Windows/WSL, mobile hosts) | not run | not run |

Recommendation after the Codex audit: native Codex now matches or exceeds ACP on
every row run on Linux (live turn settings, sub-agent rows, warnings as notices).
The ACP behaviours the audit found against the product's settings (guarded
escalations approved by Codex's own reviewer, Plan Mode turns without Codex's
Plan Mode, hidden title requests) are fixed on ACP too. Make native the Codex
default once the release matrix passes; until then ACP stays the default.

A failing or unrun row keeps ACP the default. Re-run the isolated rows whenever
the environment image's harness versions change: both runtimes pin the versions
they were reviewed against.
