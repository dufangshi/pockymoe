# Native harness protocols

ACP remains the default harness protocol. A device can instead connect directly
to Codex's `codex app-server` JSON-RPC protocol. Both paths use Codex's own rollout
store and keep the same `codex::` session identifiers. Release the loaded harness
before changing protocols for an existing conversation; the next load uses the
new selection. A setting change never moves an active session between processes.

The device owner reads or replaces the selection through
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

`CODEX_COMMAND` selects the host Codex command; isolated execution resolves
`codex` inside the environment. Upstream credentials remain in the existing
harness configuration or isolated launch environment. Protocol settings contain
no credentials.

Regenerate the checked-in protocol schema and Rust wire structs with
`scripts/generate-codex-schema.sh` using the installed Codex binary. The generator
includes experimental fields because rollback and collaboration use them.
