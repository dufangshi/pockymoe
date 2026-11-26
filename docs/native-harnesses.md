# Native harness protocols

ACP remains the default harness protocol. A device can instead connect directly
to Codex's `codex app-server` JSON-RPC protocol. Both paths use Codex's own rollout
store and keep the same `codex::` session identifiers. Release the loaded harness
before changing protocols for an existing conversation; the next load uses the
new selection. A setting change never moves an active session between processes.

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

The experimental path has remaining differences: account rate-limit pushes are
not displayed, and extended image, review and hook items retain their native
details in generic transcript rows. Client-defined dynamic tools, external token
refresh and attestation callbacks are unsupported and receive a protocol error.
MCP elicitation currently supports forms. Agent catalog installation metadata
still describes the ACP adapter, even when the native runtime is selected.

`CODEX_COMMAND` selects the host Codex command; isolated execution resolves
`codex` inside the environment. Upstream credentials remain in the existing
harness configuration or isolated launch environment. Protocol settings contain
no credentials.

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

Native Claude is experimental. Isolated environments reject this backend before
launching a host process; select ACP there. SDK-hosted MCP servers and callback
hooks are not implemented; unsupported control callbacks fail closed. Claude's
own configured subprocess MCP servers, hooks, agents and project instructions
continue to load normally, and the device MCP registry is passed at launch.
Autonomous work outside an active prompt, complete TaskCreate/TaskUpdate plan
projection and rich edit-diff parity remain qualification requirements. Keep ACP
selected for those workflows. `rewind_files` is deliberately not exposed: the
product's Git checkpoints own file restore and its undo boundary.

Native usage deduplicates assistant snapshots by API message id, keeps child
usage in billing, and keeps the latest parent response as context occupancy.
When Claude reports the context window in the turn result it updates the normal
context meter. TaskCreate/TaskUpdate/TaskList results project the known task list;
Edit tool results carry the exact replacement snippet as a diff. A whole-file
Write has no trustworthy previous contents in this protocol mapping, so its
transcript reports the write without inventing an addition-only diff.
