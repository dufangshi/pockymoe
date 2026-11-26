# Outside agents over MCP

Pockymoe serves tools to outside agents. This is separate from the
[MCP servers passed to Pockymoe's harnesses](mcp-servers.md).

## Local clients

Run `pockymoe mcp serve --access supervised` as a subprocess. Its stdout contains
newline-delimited MCP JSON-RPC only; diagnostics go to stderr. It finds the
Supervisor using the same saved `cli.json` connection as the owner CLI. Use
`--cli-config PATH` for another installation. A managed thread credential cannot
start an outside-agent server.

Examples:

```sh
claude mcp add pockymoe -- pockymoe mcp serve
codex mcp add pockymoe -- pockymoe mcp serve
# Generic clients: command=pockymoe, args=["mcp", "serve"]
```

`--access read` permits inspection, `supervised` additionally permits creating,
sending to and stopping guarded threads, and `full` additionally permits yolo
threads, answering approvals and switching harnesses. The default is supervised. This is a ceiling;
tool arguments cannot widen it, and `tools/list` offers only the tools it allows.
`--workspace ID` (repeatable) limits the client to those workspaces, like the
workspace choice on the HTTP consent page. Git tools only read status and diffs.

The Supervisor issues a new CLI token each time it starts. A running
`pockymoe mcp serve` reads the saved connection again when a call is refused or
cannot connect, and retries for up to 30 seconds, so a Supervisor restart or
update does not end the client's session. Errors name the Supervisor's reason.

`--device DEVICE` uses the existing cross-device CLI trust boundary. Remote
connections advertise only tools supported by that boundary: directory/workspace
listing, threads, harnesses/models, reading, launching and waiting. Remote sending
requires full access because the peer status API does not attest a thread’s
approval mode; stop, approval, Git and event tools are local only.

## HTTP clients

Point an OAuth-capable MCP client at `http://127.0.0.1:PORT/mcp`, or a direct
HTTPS reverse proxy for this Supervisor. Set `POCKYMOE_MCP_ISSUER` to its canonical
origin when using HTTPS. Issuers and redirects must use HTTPS or loopback HTTP.
The relay does not forward this endpoint.

Discovery supports protected resource metadata, authorization server metadata
and dynamic registration. Public clients use authorization code with PKCE S256,
exact registered redirects and a `resource` parameter equal to the MCP URL.
The sign-in opens the Supervisor Web UI. Sign in as the owner, review the client
name and redirect, choose an access ceiling and optionally select workspaces,
then allow or deny. Shares cannot grant outside-agent access.

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

Workspace restrictions apply to both collections and individual targets.
Supervised clients cannot send to or stop a yolo thread. Unknown tools,
arguments, values and out-of-range limits fail closed. Milestones retain the
latest 1,000 turn-completed, turn-failed, approval and PR-change events with
their thread, turn and request IDs (never text). Poll with the returned cursor;
it survives Supervisor restarts. Events outside the grant are filtered.

Settings shows setup commands, connected grants, last use, revocation and recent
calls. Audit records contain client, tool, target, timestamp and success/error;
they omit prompts, replies and tokens. Retention is 30 days and at most 1,000
calls. No MCP credential is returned by the settings API.

The Rust server implements the JSON-RPC boundary directly, reusing the existing
Supervisor services and CLI trust boundary rather than introducing a second
SDK dispatcher. The supported JSON response transport needs no SSE SDK state.
