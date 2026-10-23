# Auth And Connectivity Modes

Pockymoe supports three backend connectivity modes through `POCKYMOE_MODE`.

The default remains compatible with the existing supervisor behavior.

## Startup host policy

Host mode is independent of `POCKYMOE_MODE` and its authentication rules. Select it
with `POCKYMOE_HOST_MODE=standalone|hosted`, or `--host-mode standalone|hosted` on
`pockymoe supervisor`, `start`, `relay-supervisor` or `device-run`. Invalid values
fail startup. The default, `standalone`, preserves all existing subsystems.

`hosted` is for a Supervisor embedded in an external control plane that owns
networking, identity and device management. It disables the relay tunnel,
account binding and browser/device enrollment, device peers (including the
outbox worker), and port previews. Local threads, ACP harnesses, automations,
inbox, tasks, workspace files and the local HTTP/WebSocket API remain available.

The capability set is derived once at startup and is exposed as
`hostCapabilities` in `GET /api/config/runtime` and `/healthz`:

```json
{"hostMode":"hosted","hostCapabilities":{"relay":false,"accountBinding":false,"devicePeer":false,"previews":false}}
```

The existing platform `capabilities` object keeps its meaning. Web entry points
use the host capabilities; disabled HTTP routes and CLI operations return
`feature_disabled_by_host_mode` (HTTP 403). This includes `device`, `fs`, `outbox`,
`preview`, remote thread targets and `thread --device`. Local inbox delivery is
still enabled. The local CLI discovery file also records the capabilities so
`bind` cannot change the account trust of a hosted installation.

The Web reads the capabilities before mounting a local Supervisor's routes and
shows a retry message if it cannot. Through a relay it reads them from the device
in the page address; relay pages that belong to no device (devices, account,
settings) use the full set, and a device that cannot answer keeps the full set.

A `device-run` private JSON config can save `POCKYMOE_HOST_MODE`; an explicit
`--host-mode` overrides it. Other saved device settings keep their existing
precedence. Hosted device configs do not require a relay agent token, while
standalone configs still do. Changing host mode requires restarting the
Supervisor. Hosted mode defaults updates to `host-managed`; an explicit
`POCKYMOE_SUPERVISOR_RESTART_STRATEGY` overrides that default.

Hosted mode preserves local CLI token checks, login requirements selected by
`POCKYMOE_MODE`, and existing owner authorization. Host mode alone does not accept
host-supplied identity or grant anonymous access. The optional Treer binding scope
is described below. Choose `server` or `relay` authentication
and its required credentials when exposing a listener beyond a trusted network.

## Modes

### `local`

This is the default when `POCKYMOE_MODE` is unset.

The Supervisor serves its own Web UI and API on one computer and, optionally,
its network. [Local mode](local-mode.md) describes the one-line install
(`setup.sh` without relay arguments), options and limits.

The Supervisor listens on `POCKYMOE_HOST`:`POCKYMOE_PORT` (also `HOST`/`PORT`;
default `127.0.0.1:8787`). An installed release serves its own Web UI and keeps
its data in `~/.remote-codex` without further settings.

No login is required until the owner sets an **access passcode** (Settings →
Device). The passcode is stored as an scrypt hash in `kv`; browsers then sign in
with `POST /api/auth/login {password}` and receive the same session cookie or
bearer token as server mode, signed by a secret that a new passcode replaces
(signing out every other browser). `GET /api/auth/session` reports
`passcode: true`. `PUT /api/auth/passcode {passcode}` sets or changes it and
`DELETE /api/auth/passcode` removes it; ten wrong passcodes within a minute pause
sign-in for that minute. The local CLI keeps working with its token, and
`pockymoe passcode off` removes a forgotten passcode with the computer owner's
CLI credentials (not a thread's scoped token). Hosted (Treer) Supervisors cannot
set a passcode.

### `server`

Legacy. A reachable Supervisor with one administrator from environment variables.
For new deployments use a [relay](self-host-relay.md), or local mode with a
passcode on a trusted network; existing `server` deployments keep working.

Required environment variables:

```bash
POCKYMOE_MODE=server
POCKYMOE_ADMIN_USERNAME=admin
POCKYMOE_ADMIN_PASSWORD=change-me
POCKYMOE_SESSION_SECRET=at-least-16-characters
```

In this mode:

- `POST /api/auth/login` accepts the configured admin username and password.
- `GET /api/auth/session` returns the current auth state.
- `POST /api/auth/logout` clears the session token.
- All other `/api/*` routes require auth.
- `/ws` requires auth before the websocket is accepted.

The first implementation uses a signed bearer/session token suitable for simple single-admin deployments. It is intentionally minimal and should be replaced or extended before multi-user SaaS usage.

Clients can authenticate in two ways:

- Same-origin web clients can rely on the `remote_codex_session` HttpOnly cookie set by login.
- Native clients should send `Authorization: Bearer <token>` on REST calls.
- Browser websocket clients cannot set custom headers, so `/ws?token=<token>` is accepted for websocket authentication.

### `relay`

The supervisor runs on a private machine and connects outward to a public relay. Mobile and web clients connect to the relay, and the relay forwards traffic to the private supervisor over that outbound tunnel.

This mode exists because the private supervisor often cannot accept inbound public connections due to NAT, firewalls, or dynamic IPs. The durable connection must therefore be initiated by the supervisor:

```text
Mobile app / browser
  -> public relay
    -> existing outbound supervisor-to-relay websocket
      -> home Pockymoe supervisor
```

Expected environment variables:

```bash
POCKYMOE_MODE=relay
POCKYMOE_ADMIN_USERNAME=admin
POCKYMOE_ADMIN_PASSWORD=change-me
POCKYMOE_SESSION_SECRET=at-least-16-characters
POCKYMOE_RELAY_SERVER_URL=wss://relay.example.com
POCKYMOE_RELAY_AGENT_TOKEN=rcd_REPLACE_ME
```

Relay mode still requires the local supervisor admin configuration, but public
clients authenticate against the relay user system. Requests forwarded over the
supervisor-initiated tunnel are not asked to send the private supervisor admin
password over the public edge. A supervisor that is not bound to an account key
trusts them as relay-authorized requests. A bound supervisor accepts only
encrypted requests signed by a browser certified by its owner's account key; see
[account key trust](account-key-trust.md).

The public relay server is packaged in the main `pockymoe` npm package and
is started with `pockymoe relay`.

On the public server, run the relay server:

```bash
npm install -g pockymoe
POCKYMOE_ADMIN_USERNAME=admin
POCKYMOE_ADMIN_PASSWORD=change-me-now
POCKYMOE_RELAY_SESSION_SECRET=replace-with-at-least-32-random-characters
POCKYMOE_RELAY_DATA_DIR=/var/lib/remote-codex-relay
POCKYMOE_RELAY_REGISTRATION_ENABLED=true
HOST=0.0.0.0
PORT=8788
pockymoe relay
```

`pockymoe relay` requires:

- `POCKYMOE_ADMIN_USERNAME`
- `POCKYMOE_ADMIN_PASSWORD`

It should normally also be given:

- `POCKYMOE_RELAY_SESSION_SECRET`
- `POCKYMOE_RELAY_DATA_DIR`
- `POCKYMOE_RELAY_REGISTRATION_ENABLED`
- `HOST`
- `PORT`

Private device HTTP previews are optional. Set
`POCKYMOE_PORT_PREVIEW_BASE_URL` only after configuring wildcard DNS, TLS
and ingress routing; see [device web previews](port-previews.md).

Google and GitHub OAuth are enabled only when both credentials for that
provider are configured. `POCKYMOE_PUBLIC_BASE_URL` must be the public
HTTPS relay origin so the generated callback exactly matches the provider
configuration:

```bash
POCKYMOE_PUBLIC_BASE_URL=https://relay.example.com
POCKYMOE_GOOGLE_OAUTH_CLIENT_ID=...
POCKYMOE_GOOGLE_OAUTH_CLIENT_SECRET=...
POCKYMOE_GITHUB_OAUTH_CLIENT_ID=...
POCKYMOE_GITHUB_OAUTH_CLIENT_SECRET=...
```

The callback paths are `/relay/auth/oauth/google/callback` and
`/relay/auth/oauth/github/callback`. OAuth identities are stored in
`relay_user_identities`. When registration approval is enabled, a new OAuth
identity enters `relay_pending_registrations`; approval creates the user and
identity in one transaction.

An OAuth login starts in one of two ways. Both check that the provider is
enabled and configured, store the pending login in `relay_oauth_challenges`
and set the `remote_codex_oauth` browser cookie (`Path=/relay/auth/oauth`,
10 minutes):

- `GET /relay/auth/oauth/{provider}/start` redirects to the provider. Sign-in
  buttons use it; it carries no invite code.
- `POST /relay/auth/oauth/{provider}/start` with the JSON body
  `{ "inviteCode": "..." }` (field optional) answers
  `200 { "url": "<provider authorization URL>" }`, and the browser then opens
  `url`. Create account uses it. The relay keeps only a SHA-256 hash of the
  trimmed code with the pending login (`relay_oauth_challenges.invite_hash`);
  the code never travels in the provider round trip, logs or audit records.
  When an invite code is configured and a non-empty code does not match, the
  answer is `403 { "code": "invalid_invite_code", "message": "The invite code
  is not valid." }`. An empty or missing code is accepted here and decided by
  the callback, because an existing linked account needs none.

The callback signs in an identity that is already linked, whatever the
registration settings are. For a new identity it checks, in order: registration
open (`Registration is currently disabled.`), then, when an invite code is
configured, the hash stored at the start against the current code (missing:
`An invite code is required to create an account. Choose Create account and
enter the code.`; different, for example a code changed during the login:
`The invite code is not valid.`), then the email is not already used, and only
then creates the account or the pending registration. Refusals return to
`/relay-portal?oauthError=<message>`; a pending registration returns to
`/relay-portal?oauthPending=1`.

In the Web, **Create account** on a relay with an invite code puts the code
field first: the password form and **Continue with Google/GitHub** both need
it. The Google and GitHub buttons there send the code with the POST start;
without a code they say so beside the field, and a wrong code shows "The invite
code is not valid." there without leaving the page. Without an invite code, and
on the sign-in card, the buttons are plain links to the GET start. The admin
console's registration settings say that each setting applies to Google and
GitHub sign-up as well.

Resulting workflow for the three registration settings:

| Open registration | Require admin approval | Invite code | Password sign-up | New Google/GitHub identity | Existing linked OAuth account |
| --- | --- | --- | --- | --- | --- |
| Off | either | either | Refused | Refused | Signs in |
| On | Off | None | Account created | Account created and signed in | Signs in |
| On | Off | Set | Created with the code; refused without it or with a wrong one | Created with the code entered at Create account; refused without it or with a wrong one | Signs in, no code |
| On | On | None | Waits for approval | Waits for approval | Signs in |
| On | On | Set | Waits for approval with the code; refused without it | Waits for approval with the code; refused without it, and no request is queued | Signs in, no code |

Older relays ignored the invite code for Google/GitHub sign-up: rows with an
invite code set behaved like the rows without one in the new-identity column.
Email verification does not change any row: the Rust relay refuses to enable
`emailVerificationEnabled`, and an OAuth identity is accepted only with an
email the provider reports as verified.

On the private machine that will run Codex and access local workspaces, run the
relay-connected supervisor backend:

```bash
npm install -g pockymoe
POCKYMOE_ADMIN_USERNAME=admin
POCKYMOE_ADMIN_PASSWORD=change-me-locally
POCKYMOE_SESSION_SECRET=at-least-16-characters
POCKYMOE_RELAY_SERVER_URL=wss://relay.example.com
POCKYMOE_RELAY_AGENT_TOKEN=rcd_REPLACE_ME
HOST=127.0.0.1
PORT=8787
pockymoe relay-supervisor
```

`pockymoe relay-supervisor` sets `POCKYMOE_MODE=relay` for the child
supervisor process. It requires:

- `POCKYMOE_ADMIN_USERNAME`
- `POCKYMOE_ADMIN_PASSWORD`
- `POCKYMOE_SESSION_SECRET`
- `POCKYMOE_RELAY_SERVER_URL`
- `POCKYMOE_RELAY_AGENT_TOKEN`

When running a relay-connected supervisor beside another local Pockymoe
service, also set separate values for:

- `PORT`
- `DATABASE_URL`
- `WORKSPACE_ROOT`

`POCKYMOE_RELAY_SERVER_URL` is a websocket base URL. Use `ws://host:port`
for a plain relay port, or `wss://relay.example.com` when the relay is behind
TLS.

In a source checkout, run the Rust relay with `cargo run -p pockymoe -- relay`.

The relay server also serves the built web frontend when
`apps/supervisor-web/dist/index.html` is present in the installed package or
source checkout. `POCKYMOE_RELAY_WEB_DIST_DIR` can override that path. The
relay injects a bootstrap config into `index.html` so the browser uses
`/relay/...` APIs instead of trying to contact a local supervisor directly. When
`POCKYMOE_WEB_ORIGIN` names a different origin, the relay serves only its API and
the Web UI is hosted separately; see
[self-hosting a relay](self-host-relay.md#separate-web-origin).

Relay mode is a separate transport layer from the normal supervisor API. The relay should:

- authenticate relay users at the public edge,
- authenticate private supervisor tunnels with per-device tokens,
- maintain a user-owned device registry,
- expose a portal for users to create/manage devices and shares,
- expose an admin panel for the configured admin user,
- multiplex REST-like requests and websocket events over the outbound supervisor connection,
- preserve request IDs and backpressure,
- enforce a strict allowlist for shared session operations,
- avoid exposing the home supervisor directly to the public internet.

The current implementation establishes the supervisor-initiated outbound tunnel, heartbeat, multi-device relay registry, relay user accounts, session sharing, REST request multiplexing, and a websocket event bridge:

- the home supervisor validates `POCKYMOE_RELAY_SERVER_URL` and `POCKYMOE_RELAY_AGENT_TOKEN`,
- the home supervisor requires the configured admin username/password for Pockymoe API access,
- the home supervisor connects outward to `/supervisor/tunnel`,
- the relay authenticates that tunnel with a per-device token created in `/relay-portal`,
- optional `POCKYMOE_RELAY_SUPERVISOR_TOKEN` remains as a legacy bootstrap token path,
- `/healthz` on the relay reports whether a supervisor is connected.
- OAuth login supports existing Google/GitHub identity rows and approval flows.
- relay users register with email, username, and password at `/relay-portal`,
- relay users create devices and configure the returned `rcd_...` token on the private supervisor as `POCKYMOE_RELAY_AGENT_TOKEN`,
- clients call `GET|POST|PATCH|DELETE /relay/devices/:deviceId/api/...` on the public relay,
- `/relay/api/...` remains a compatibility path that selects the first accessible connected device,
- the relay authenticates clients with relay user sessions,
- the relay forwards allowed requests to the home supervisor through the outbound websocket tunnel,
- the home supervisor executes the request against its loopback Axum API and returns the response over the tunnel.
- clients can connect to `GET /relay/devices/:deviceId/ws` on the public relay,
- `/relay/ws` remains a compatibility path that selects the first accessible connected device,
- the relay authenticates websocket clients with relay user sessions,
- thread and shell event envelopes are forwarded from the home supervisor to relay websocket clients,
- relay websocket clients can use `supervisor.ping` and receive `supervisor.pong`,
- relay websocket client messages are routed through the home supervisor websocket plugin handlers, so terminal plugin messages such as shell attach, input, resize, clear, and detach can use the relay path.

Tunnel liveness and queues:

- the supervisor pings every 3 seconds and reconnects after 15 seconds without a
  frame from the relay, or when a write takes longer than 15 seconds. On giving
  up it sends a Close frame, so the relay drops the connection at once.
- the supervisor writes queued frames in batches with one flush.
- each socket queue (supervisor outbound, relay per device, relay per browser
  client) holds 8192 messages. A burst waits in the queue; only a peer that has
  stopped reading fills it. The supervisor logs "relay tunnel queue overflowed"
  in that case.
- a request that finds the device's relay queue full gets `429` (busy), not
  `503` "device is offline".
- the Web UI asks once more for a read (`GET`/`HEAD`) that meets `429` or
  `503`, but never repeats a write.
- a connection error shown on a thread clears itself once the thread loads
  again.

## Relay Users, Devices, And Sharing

The relay server stores users, devices, shares, grants, and settings in
`POCKYMOE_RELAY_DATA_DIR/relay-store.sqlite`. Before replacing a Node 0.11
relay, stop it and run `pockymoe relay-migrate --data-dir <path> --dry-run`,
then repeat without `--dry-run`. Migration uses SQLite Online Backup and keeps
`relay-store.pre-rust-0.12.sqlite`; it never deletes the old Rust
`relay.sqlite`. The first admin user
is seeded from `POCKYMOE_ADMIN_USERNAME`,
`POCKYMOE_ADMIN_PASSWORD`, and optional `POCKYMOE_ADMIN_EMAIL`.

Relay portal:

- `/relay-portal` lets users log in or register with email, username, and password.
  The browser derives an authentication key and an encrypted account vault from
  the password, so the relay never receives it; registration also shows a
  one-time account recovery code. An account created before account keys fails
  its first ordinary sign-in and is then offered **Continue with this password**,
  which sends its password once, and then secures the account.
- registered users can create multiple devices.
- creating a device returns a permanent `rcd_...` device token; it can be copied again by the device owner and is revoked only when replaced or the device is deleted. Creating a device and fetching, issuing or replacing its token need a sign-in or verification within the last 10 minutes.
- each device token maps one running private Pockymoe supervisor to that user.
- users can share a single `threadId` on one device with another username.
- invited users see those entries under Shared With Me and can continue the shared thread.

Relay admin:

- `/relay-admin` is available to the seeded admin user.
- admin can view all users and devices.
- admin can enable or disable normal users.
- admin can enable or disable open registration.

Shared session access is intentionally narrower than owner access. Device owners
can access their device's full allowed relay API surface. Invited users can only
use the shared thread routes and the websocket stream is scoped to the shared
thread. A device bound to an account key enforces the same rules itself from
owner-signed grants, and recipients use their own browser certificates.

The first relay forwarding layer is intentionally narrow:

- it allows `/api/*` and `/healthz` targets only,
- it is intended for JSON/text REST calls,
- it relays supervisor event stream output and websocket plugin commands,
- it does not yet optimize binary downloads, file streams, or large multipart uploads,
- it does not yet implement request backpressure beyond per-request timeout cleanup.

Those capabilities should be added on top of the same outbound tunnel rather than by exposing the home supervisor directly.

## Security Notes

- Use `local` only on trusted loopback, LAN, or VPN networks.
- Use HTTPS/TLS in `server` mode when accessed by mobile clients.
- Do not reuse the admin password as `POCKYMOE_SESSION_SECRET`.
- Keep destructive actions behind existing confirmation flows even after login.
- For mobile apps, store tokens in Keychain or Android Keystore.

Relay MFA, trusted browsers, device fingerprints, encrypted transport, and upgrade recovery are described in the [security operations guide](./relay-security-operations.zh.md); account keys and device binding in [account key trust](./account-key-trust.md). Persist the relay session secret alongside a secure database backup: it also derives the key protecting authenticator secrets.

## Treer hosted hub

`POCKYMOE_HOSTED_BY=treer` enables the hub's Treer attach and AIS interfaces and
requires `POCKYMOE_HOST_MODE=hosted`; startup rejects other combinations.
Hosted mode defaults Supervisor restarts to `host-managed` unless
`POCKYMOE_SUPERVISOR_RESTART_STRATEGY` is explicitly set. Connectivity mode still
controls ordinary requests without Treer markers.

The local attach WebSocket requires loopback and the 0600 `treer-attach.token`
file alongside the hub database. Each live attach owns one top-level thread.
Treer credentials live in memory and enter only that thread's harness environment;
delegated threads receive no Treer identity. Disconnect pauses new inputs and queued
execution, interrupts the current turn, and releases its harness through the normal
release API. Thread history and queued work remain available for reattachment.

AIS dispatch requires a live Agent ID and interface instance. Controller semantic
calls require `Host: agent-interface`, `x-treer-interface-source: controller-api`,
and no Origin or Sec-Fetch headers. Agent UI calls require `proxy-ui` provenance
and the hub's loopback Host authority. These calls can access only their bound
thread and descendants, with scoped thread/workspace lists and a small set of
read-only page dependencies. Management, device APIs, global settings writes and
other threads remain forbidden; WebSocket events are filtered by the same scope.
Approval/question replies verify that the request belongs to the addressed thread.
The UI entry redirects to a relative thread URL so external path prefixes survive.
Queued AIS submissions retain durable receipts. Completion notifications use the
bound Agent identity through `treer message send`, with a stable idempotency key
and durable exponential retry scheduling capped at 60 seconds. Delivery failures
never rerun a completed prompt.

Headers on other transports never create this scope. Invalid or incomplete Treer
markers fail closed, including when the integration is disabled. Validated Treer
UI scope supplies the embedded session in server mode without a second login.
Provenance headers defend against ordinary browser requests; they are public
markers and do not authenticate hostile processes running under the hub's OS user.
Treer's Proxy/Controller must strip client markers and inject its own target-bound
headers. App-wide human authorization and cross-Agent browsing require a separate
Treer authorization contract; the Agent UI scope does not grant either.
