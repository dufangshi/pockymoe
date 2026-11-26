# Outside agents over MCP

Pockymoe serves tools to outside agents. This is separate from the
[MCP servers passed to Pockymoe's harnesses](mcp-servers.md).

## Local clients

Run `pockymoe mcp serve --access supervised` as a subprocess. Its stdout contains
newline-delimited MCP JSON-RPC only; diagnostics go to stderr. It finds the
Supervisor using the same saved `cli.json` connection as the owner CLI. Use
`--cli-config PATH` for another installation. A managed thread credential cannot
start an outside-agent server.

Settings → Outside agents shows these commands with this Supervisor's address.
Each one was run as written with the client version in brackets:

```sh
# Claude Code (2.1): user scope, so it works in every project, not only the current one
claude mcp add --scope user pockymoe -- pockymoe mcp serve
# Codex CLI (0.169): global by default
codex mcp add pockymoe -- pockymoe mcp serve
# Gemini CLI (0.71): its default project scope fails outside a trusted folder
gemini mcp add --scope user pockymoe pockymoe mcp serve
# MCP Inspector (2.13), one-shot calls
npx -y @modelcontextprotocol/inspector --cli pockymoe mcp serve --method tools/list
```

Cursor (agent CLI 2026.11) reads `~/.cursor/mcp.json`:

```json
{"mcpServers":{"pockymoe":{"command":"pockymoe","args":["mcp","serve"]}}}
```

Add `--access read` or `--access full`, or `--workspace ID`, after `mcp serve`.

`--access read` permits inspection, `supervised` additionally permits creating,
sending to and stopping guarded threads (guarded is the harness's normal agent
mode, Codex `agent` and Claude `auto`, in which the harness itself decides what
needs approval; both edit files and run ordinary commands without asking), and `full` additionally permits yolo
threads, answering approvals and switching harnesses. The default is supervised. This is a ceiling;
tool arguments cannot widen it, and `tools/list` offers only the tools it allows.
`--workspace ID` (repeatable) limits the client to those workspaces, like the
workspace choice on the HTTP consent page. Git tools only read status and diffs.

The Supervisor issues a new CLI token each time it starts. A running
`pockymoe mcp serve` reads the saved connection again when a call is refused or
cannot connect, and retries for up to 30 seconds, so a Supervisor restart or
update does not end the client's session. A read-only call (by the tool's
`readOnlyHint`) cut off by a restart, such as a long `thread_wait`, is sent
again; any other call reports that it may have run, so the client checks before
repeating it. Errors name the Supervisor's reason.

`--device DEVICE` uses the existing cross-device CLI trust boundary.
`pockymoe_devices` lists the owner's devices; its `deviceId` values are what
`--device` takes. Remote connections advertise only tools supported by that
boundary: devices, workspaces, threads, harnesses and models, reading,
launching and waiting. Remote sending requires full access because the peer
status API does not attest a thread's approval mode; stop, approval, Git,
harness switching and events work only on the device itself, and a call to them
says so. Results have the local shapes, except that a remote `thread_wait`
returns a summary `transcript` instead of the thread detail. `--workspace`
limits remote lists and thread calls too. A remote launch uses ACP and names
the thread after the first line of its prompt unless `title` is given.

## HTTP clients

Point an OAuth-capable MCP client at `http://127.0.0.1:PORT/mcp`, or a direct
HTTPS reverse proxy for this Supervisor. Use the address Settings shows: OAuth
clients check that the authorization server's issuer matches the URL they were
given, so `localhost` does not stand in for `127.0.0.1`.

```sh
# Claude Code: add, then sign in (or use /mcp inside a session)
claude mcp add --scope user --transport http pockymoe-http http://127.0.0.1:PORT/mcp
claude mcp login pockymoe-http
# Codex: adding starts the sign-in; `codex mcp login pockymoe-http` repeats it
codex mcp add pockymoe-http --url http://127.0.0.1:PORT/mcp
```

Cursor: put `{"mcpServers":{"pockymoe-http":{"url":"http://127.0.0.1:PORT/mcp"}}}`
in `~/.cursor/mcp.json`, then run `cursor-agent mcp login pockymoe-http`.
Clients that take a fixed header instead of OAuth (Gemini CLI `-H`, MCP
Inspector `--header`) need an access token, which expires after 15 minutes;
prefer stdio for them. Set `POCKYMOE_MCP_ISSUER` to its canonical
origin when using HTTPS. Issuers and redirects must use HTTPS or loopback HTTP.
The relay does not forward this endpoint.

Discovery supports protected resource metadata, authorization server metadata
and dynamic registration. Public clients use authorization code with PKCE S256,
exact registered redirects and a `resource` parameter equal to the MCP URL.
The sign-in opens the Supervisor Web UI. Sign in as the owner, review the client
name and redirect, choose an access ceiling and optionally select workspaces,
then allow or deny. Shares cannot grant outside-agent access. An approved
sign-in whose client never collects its code expires after five minutes and is
not listed as a connected client.

Access tokens expire after 15 minutes. Refresh tokens rotate on every use and
expire after 30 days; replaying a used refresh token or authorization code
revokes its grant. Errors use the OAuth codes (`invalid_grant`, `invalid_target`,
`invalid_redirect_uri` …); once the client and redirect are verified, sign-in
errors return to the client's redirect. `resource` may be omitted (it defaults
to this MCP URL) and unknown scope values are ignored. Registration (30 a
minute), sign-in (60) and token and revoke requests (120) are limited per
Supervisor, failed attempts included, with HTTP 429 and `Retry-After`. Grants
expire after 30 days. Tokens and authorization codes are stored as hashes in a
separate `*.mcp.sqlite` database beside the Supervisor journal. Revoke a client
in **Settings → Outside agents**. Back up the sidecar to preserve grants;
omitting it from a restore revokes those grants. MCP tokens never authorize the general HTTP
API, WebSockets or local CLI bridge.

The server supports MCP 2025-03-26, 2025-06-18 and 2025-11-25. Streamable HTTP
uses JSON POST responses, `Mcp-Session-Id` sessions and DELETE termination. GET
returns 405; there is no unsolicited SSE stream, and JSON-RPC batches are
refused. After initialization a missing `MCP-Protocol-Version` header means the
session's negotiated version; an unsupported one is refused. HTTP Origin
headers must match the configured issuer. Against DNS rebinding, `/mcp`, the
OAuth endpoints and the consent API accept only an IP address, `localhost` or
the issuer's host name in `Host`. Cancellation notifications cancel an
outstanding call in that same session; cancelling a wait does not stop the agent.

## Tools and records

`tools/list` describes input/output schemas and read-only/destructive annotations.
Tools cover device identity, workspaces, harnesses and models, thread search,
thread/turn reading, bounded waits, launch, inbox/queue/steer, stop, approvals,
Git status/diff, checkpoint turn diffs, harness switching with handoff preview and milestone polling. `thread_wait` waits at most ten minutes
and returns status and the latest transcript, including the final reply.

`thread_read` pages back with `beforeTurnId` set to the returned
`nextBeforeTurnId`; the `pockymoe transcript …` hints in its replies are the CLI
equivalents. Arguments are read by their schema type, so `30` and `"30"` are the
same cursor or limit. Refusals say what the grant allows and how the owner can
widen it.

Workspace restrictions apply to both collections and individual targets.
Supervised clients cannot send to or stop a yolo thread. Unknown tools,
arguments, values and out-of-range limits fail closed. Milestones retain the
latest 1,000 turn-completed, turn-failed, approval and PR-change events with
their thread, turn and request IDs (never text). Poll with the returned cursor;
it survives Supervisor restarts. Events outside the grant are filtered.

Settings shows setup commands, connected grants (access, workspaces by name, last
use), revocation after a confirmation and recent calls with the client's name. Audit records contain client, tool, target, timestamp and success/error;
they omit prompts, replies and tokens. Retention is 30 days and at most 1,000
calls. No MCP credential is returned by the settings API.

The Rust server implements the JSON-RPC boundary directly, reusing the existing
Supervisor services and CLI trust boundary rather than introducing a second
SDK dispatcher. The supported JSON response transport needs no SSE SDK state.
