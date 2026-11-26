# Native harness protocols

Codex and Claude run natively by default: Pockymoe speaks Codex's
`codex app-server` JSON-RPC and Claude Code's SDK stream directly. Their ACP
adapters (codex-acp, claude-agent-acp) stay available under the ACP agent, next
to the other ACP agents. Both protocols use the harness's own session store
(Codex rollouts, Claude JSONL) and the same `codex::` / `claude::` session IDs.

## Choosing a protocol

A thread's protocol is chosen when it is created and pinned on the thread:
`harnessProtocol` (`native` or `acp`) beside `agentId` (`codex`, `claude`); it is
absent for ACP-only agents. In the new-thread form, Codex and Claude are native;
**ACP agent** reveals ACP Codex, ACP Claude and the other ACP agents, and threads
on an adapter are labelled "ACP · Codex" / "ACP · Claude Code".

Clients name the choice with an agent key: `codex` and `claude` run natively,
`acp:codex` and `acp:claude` use the adapters, any other id is an ACP agent. No
agent means native Codex. The key is accepted as `agentId` by
`POST /api/threads/start`, thread import, the models and capabilities queries
(`?agentId=acp:codex`), harness switch and fork targets, `pockymoe thread create
--agent`, delegates and the MCP `thread_launch` and `thread_switch` tools.
`GET /api/agent-runtimes/acp/agents` lists native `codex` and `claude` first
(`acpAgent.harnessProtocol` `native`, `transport` `native`; ready when the base CLI
is installed, no adapter needed), then the ACP agents, with the adapters keyed
`acp:codex` / `acp:claude` (`acpAgent.baseAgentId`).

The protocol stays with the thread across reloads, Supervisor restarts and
forks. A loaded session keeps its runtime until it is released; an unloaded one
follows the protocol its thread recorded. The harness switch picker moves a
thread between Codex and ACP Codex (or Claude and ACP Claude) without a handoff:
both read the same store, so the thread keeps its session, history and settings,
and the transcript records the change. Switching to another agent hands off
history as before ([harness-switching.md](harness-switching.md)).

Threads created before threads recorded a protocol run natively from their next
load. The native runtimes passed the promotion gate below, including ACP →
native → ACP on one session, so these conversations continue with their full
history; switch one to ACP Codex or ACP Claude to keep it on the adapter.

There is no device-wide protocol setting: a device default would make "Codex"
mean a different runtime per device and for delegates, the CLI and MCP clients.
The former `GET` / `PUT /api/harness-protocols/settings` endpoint is gone and a
leftover `harness-protocols.json` beside the database is ignored.

The direct Codex backend supports thread start/resume/fork, rollback, streaming
turns, image inputs, model and effort selection, plan mode, steering, interrupt,
approval requests and explicit compaction. Its Rust process transport uses the
same execution-context and process-cleanup implementation as ACP. Create the
thread under the ACP agent when a feature is only available there.

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

The process starts with the Agent SDK's environment
(`CLAUDE_CODE_ENTRYPOINT=sdk-ts`, `CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS=1`),
like claude-agent-acp. One reader per process owns its output: frames go to the
running turn, and frames between turns (a background task finishing after a
stop, a scheduled wake) only update the session's task and idle state; their
callbacks are answered so Claude never waits (hooks continue, tool permissions
are denied because nobody can approve them). Nothing from between turns reaches
the next turn.

A turn is complete when Claude has answered one of the prompts the turn sent
(its prompt and steering messages, matched by `user_message_uuids`), no SDK task
(background Bash, Monitor, a backgrounded Agent) is running, and the session is
idle. Background work therefore holds the turn, shows one waiting row, and the
task's notification and Claude's follow-up answer stay in the same turn, as on
ACP. A task left running after a turn keeps the session busy for release.

Steering sends the message into the running turn with priority `now`, or
`later` while a question or permission card is open, as claude-agent-acp does;
an error-shaped result from the cycle the steer interrupted does not fail the
turn.

Stop sends `interrupt` and keeps the process: the turn ends `interrupted` when
Claude settles it, and only a stop that does not settle within 10 seconds ends
the process. Tools refused by the stop are `interrupted`, not failed. An error
result (an API error, a refusal, a turn or budget limit) fails the turn with
Claude's own message and keeps the process; only a broken stream marks the turn
`recovering`. Whatever did not finish takes the turn's status, so no tool stays
running in history.

Tool rows render as claude-agent-acp renders them for Pockymoe (titles such as
`Read path (1 - 40)`, `grep "x" path`, `Fetch url`; the same kinds, locations and
content), with what ACP drops: command output as text with its exit status,
whole-file Edit and Write diffs from Claude's structured result (applied to the
original file, so line numbers and counts are exact), MultiEdit diffs, and a
`Permission denied: <reason>` result. A subagent's own text stays inside its
Agent result; its tool calls are attributed to it. A backgrounded Agent stays in
the subagent list until its task ends. TodoWrite and the Task tools update the
plan instead of adding rows. An approved plan-mode plan becomes the turn's plan
card, as on ACP (where claude-agent-acp sends it as an ACP `plan_update`). Compaction is a compaction item with Claude's
summary, and the context meter drops to the retained size. API retries (the first and the last attempt), model
refusal fallbacks, Claude warnings and MCP servers that failed to start appear
as notices.

Permission requests use Claude's own wording (`Bash: <command>`, its
description and decision reason). The choices follow claude-agent-acp: **Yes**,
**Yes, and …** (Claude's suggested update, such as allowing edits for the
session or a rule for that command, named exactly; offered only when the label
can describe it), and **No**. An answer that matches no choice is a denial.
ExitPlanMode offers to start implementing, to start with edits auto-accepted, or
to keep planning. The product policy answers first, like ACP: yolo or full access
approves (except questions and plan exits), read-only refuses everything except
reads and questions. Guarded work runs in Claude's `auto` mode when the account
offers it (the ACP adapter's default), otherwise `default`; plan runs in `plan`
and full access in `bypassPermissions`. Claude can leave a mode by itself (a plan
approval), so each turn re-asserts the product mode.

Usage: on the host, billing comes from the same Claude JSONL reader as ACP
(every request once, subagents included), so token counts and cost match ACP.
Inside an environment, and when no JSONL is found, the stream bills: each API
message counted once, with its final `message_delta` counts. Context occupancy
streams from the main agent's latest request, against the window Claude reports
(`get_context_usage` at load, then the result's `modelUsage`).

In an isolated environment native Claude starts the
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

SDK-hosted MCP servers and SDK callback hooks are not registered; other control
callbacks fail closed. Claude's own configured subprocess MCP servers, hooks,
agents and project instructions continue to load normally, and the device MCP
registry is passed at launch. `rewind_files` is deliberately not exposed: the
product's Git checkpoints own file restore and its undo boundary.

Capabilities and the toolbox are computed as the ACP Claude adapter computes
them, with Claude's slash commands and skills from initialize and
`commands_changed`, plus input during a turn, fork at a reply and local import.
Models come from Claude's own list (efforts per model, medium by default), or
from a saved upstream connection's catalog, as on ACP. Fast mode is Claude's
`fastMode` flag setting where the account offers it. Context inspection, when
enabled, asks Claude itself (`get_context_usage`) after each model request and
at turn end, on the host and in environments; ACP needs its Node bridge and
works on the host only.

The composer's `/mcp` panel lists the MCP servers Claude itself is running for
the thread (`GET /api/threads/{id}/mcp-servers`, from Claude's `mcp_status`),
with each server's live state: failed (with Claude's error), sign-in needed,
starting or off. Server configurations (URLs, headers, commands, environment)
are never returned, and shares cannot read the list, because it names the
owner's MCP servers and account connectors. ACP reports no MCP state, so the
panel stays empty there.

Plan allowance also updates live during a turn: each `rate_limit_event` from
the API (5-hour and 7-day utilization and reset) feeds the shared subscription
cache and refreshes the composer meter, which ACP drops.

The mapping is tested against real Claude Code 2.1.322 sessions recorded with
`haiku` and replayed through the whole runtime
(`crates/runtime/src/native/claude/fixtures`).

## Promotion gate

Native became the default for Codex and Claude by the owner's decision on
2026-11-26, with the rows below passing except the release matrix; ACP stays one
choice away under the ACP agent. Re-run these checks on each release candidate
and keep the evidence with the release notes. Status as of 2026-11-26:

| Check | Codex | Claude |
| --- | --- | --- |
| Shared session store: ACP → native → ACP on one thread, same session ID, history recalled | pass | pass |
| Tool use, file edit with diff, guarded approval, interrupt, steering, resume after a Supervisor restart | pass | pass |
| Rewind (native rollback or fork-swap) and Git turn checkpoints | pass | pass |
| Isolated environment: API broker, restart and resume, rewind, guarded approval, protocol switch on one thread, no host credentials or homes | pass (scripted upstream through the real broker) | pass (real model with a native file login; scripted upstream through the real broker) |
| Isolated environment with an approved native file login | gated, not run (no ChatGPT file login on the lab host) | pass |
| Turn lifecycle events identical to ACP (`thread.turn.started` / `completed`) | pass | pass |
| Token usage, context, cost and plan allowance identical to ACP for the same prompts; allowance live during a turn | pass (scripted upstream, real CLI) | pass (same JSONL billing reader as ACP; output counts equal in the side-by-side lab; allowance live from `rate_limit_event`) |
| Plan and performance modes reach the model | pass (Codex Plan Mode instructions, fast service tier) | pass (plan) |
| Every app-server method, server request and notification classified against codex-acp, with a test for each mapped one (2026-11-26 audit) | pass: 281 entries; one minor gap (MCP tool progress text) | see the Claude audit |
| Same scripted prompts side by side with a real model: tools, edit, image, sub-agent, interrupt, `/status`, `/mcp`, `/review`, compaction, restart, escalation approval, Plan Mode | pass: same outcome on every step; every remaining difference explained (native shows sub-agent rows; ACP's Guardian auto-review, missing Plan Mode and title requests were fixed after the audit) | see the Claude audit |
| Extended items render as on ACP (MCP, web search, images, review, hooks, notices) | pass in mapping tests; review mode in the real lab; no real MCP server or image-model run | pass (replays of real sessions; real side-by-side lab for tools, questions, images, subagents, background shells, plan) |
| AskUserQuestion, explicit compaction, Write diffs in a real lab | compaction in the real lab | pass |
| SDK callback hooks and SDK-hosted MCP | not applicable | not applicable: neither path registers them for Pockymoe; Claude's own hooks and MCP servers run unchanged; MCP form elicitation answered |
| Background work outside a prompt | not run | pass (background Agent and shell hold the turn and their follow-up stays in it; frames between turns never reach the next turn; release waits for tasks) |
| Every SDK frame, control request and tool shape of the installed CLI mapped or explicitly ignored, with replay tests of real sessions | not applicable | pass (Claude Code 2.1.322) |
| Side-by-side real lab, same prompts on both protocols, every difference explained | not run | pass (2026-11-26, haiku) |
| A full release matrix (macOS, Windows/WSL, mobile hosts) | not run | not run |

Recommendation after the Codex audit: native Codex now matches or exceeds ACP on
every row run on Linux (live turn settings, sub-agent rows, warnings as notices).
The ACP behaviours the audit found against the product's settings (guarded
escalations approved by Codex's own reviewer, Plan Mode turns without Codex's
Plan Mode, hidden title requests) are fixed on ACP too.

A failing row on a release candidate is a release blocker for that harness's
native runtime. For Claude, the isolated rows were
re-run on the audited runtime (Dokploy in Docker-in-Docker, native file login,
haiku): tools, Write diff, a denied and an allowed approval, stop and continue
on the same process, a background Agent, resume after a Supervisor restart and
ACP ↔ native on one thread all passed. The release matrix remains to be run.

### What native Claude gives beyond ACP

Measured against the ACP path as Pockymoe runs it (claude-agent-acp 0.106.0):
command exit status; exact whole-file diffs for Edit, Write and MultiEdit; API
retry, model fallback and Claude warning notices (ACP drops the retries); live
plan allowance during a turn; the reason a permission was denied; context
inspection without a Node bridge, also inside environments; live MCP server
state in the `/mcp` panel; permission cards in
Claude's own words with its reason; no
Node adapter process per session. ACP still has two things native does not
offer: the plan-approval "clear context" restart and a bypass choice inside the
plan card (full access stays a product setting). Re-run the isolated rows whenever
the environment image's harness versions change: both runtimes pin the versions
they were reviewed against.
