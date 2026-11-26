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
ACP's hidden `/compact` turn.

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
plan instead of adding rows. Compaction is a compaction item with Claude's
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
| Token usage, context, cost and plan allowance identical to ACP for the same prompts; allowance live during a turn | pass (scripted upstream, real CLI) | pass (same JSONL billing reader as ACP; output counts equal in the side-by-side lab; allowance live from `rate_limit_event`) |
| Plan and performance modes reach the model | pass (Codex Plan Mode instructions, fast service tier) | pass (plan) |
| Extended items render as on ACP (MCP, web search, images, review, hooks, notices) | pass in mapping tests; no real MCP, image or review run | pass (replays of real sessions; real side-by-side lab for tools, questions, images, subagents, background shells, plan) |
| AskUserQuestion, explicit compaction, Write diffs in a real lab | compaction waits for completion (scripted) | pass |
| SDK callback hooks and SDK-hosted MCP | not applicable | not applicable: neither path registers them for Pockymoe; Claude's own hooks and MCP servers run unchanged; MCP form elicitation answered |
| Background work outside a prompt | not run | pass (background Agent and shell hold the turn and their follow-up stays in it; frames between turns never reach the next turn; release waits for tasks) |
| Every SDK frame, control request and tool shape of the installed CLI mapped or explicitly ignored, with replay tests of real sessions | not applicable | pass (Claude Code 2.1.322) |
| Side-by-side real lab, same prompts on both protocols, every difference explained | not run | pass (2026-11-26, haiku; isolated rows were not re-run after this audit's changes) |
| A full release matrix (macOS, Windows/WSL, mobile hosts) | not run | not run |

A failing or unrun row keeps ACP the default. For Claude, the remaining blockers
are the release matrix and a re-run of the isolated-environment rows on the
current runtime.

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
