# Device web previews through the relay

## Usage

The thread toolbar’s **Port mappings** button is next to sharing. Enter an HTTP
port and optional label, then enable and open it in a new tab. Clicking a chat
link to `http://localhost:4013/...` or `http://127.0.0.1:4013/...` offers a
confirmation before enabling and opening that port. Cancelling makes no change.
Mappings belong to the device and persist across Supervisor restarts. Only the
device's account owner can manage them from the Web. A mapping that a thread's
agent creates with `pockymoe preview create` also opens for the people that
thread is shared with ([who can open a mapping](#who-can-open-a-mapping)).
Control access to a shared thread reaches mappings indirectly: the recipient can
have the thread's agent run `pockymoe preview create` or `preview stop`, and the
agent CLI uses the device's local API, which creates local mappings and preview
leases ([bound devices](#bound-devices)).

**Open** signs the current browser in to that preview. **Copy address** copies
the stable private address, without a login ticket. Browser navigation to that
address enters the Relay login/open flow automatically; API and WebSocket
requests without a preview session still receive 401. **Stop** revokes the mapping and
closes its connections; the local service keeps running. Re-enabling a stopped
port creates a new address. Supervisors predating this feature must be updated.

## Who can open a mapping

The device's account owner can open every mapping. When an agent runs
`pockymoe preview create` with its own thread credential, the mapping records
that thread (`threadId`; the CLI reports `access: "thread"` instead of
`"deviceOwner"`). The Relay then also opens it for anyone it currently gives
access to that thread, with read or control access: a share or thread grant on
the thread or one of its three nearest ancestors, a workspace share of the
workspace the device reports the thread in, or a device grant. Relay administrators cannot
open previews. A `fromThreadId` in the request does not set the thread; only the
credential does.

Mappings created with the machine CLI credential, from the Web or for a DSH
console record no thread and stay owner-only. For a port that is already
enabled, a `preview create` from another thread moves the mapping to that thread
and keeps its address; the machine credential and the Web keep the recorded
thread. The device reports the thread in its heartbeat, and a change restarts
the mapping's open connections.

The Relay checks this access for every launch, request and WebSocket upgrade,
and every 15 seconds on long-lived connections, against the thread the device
reports now. Revoking or expiring the share or grant ends access at the next
check.

Recipients open the CLI `openUrl` while its lease is live. They cannot ask the
device for a lease: `POST /api/port-mappings/{id}/lease` stays owner-only, as
does the Web's Port mappings button. After `leaseExpiresAt`, running
`preview create` for the port again (by the thread's agent or the owner) renews
the lease and prints a fresh `openUrl`.

## Deployment: DNS, proxy and TLS

Example configuration for the existing site:

```ini
POCKYMOE_PORT_PREVIEW_BASE_URL=https://lnz-study.com
```

This produces `https://p-<32-hex-mapping-id>.lnz-study.com`. Each mapping has its
own origin, so root-relative assets, client routing, application cookies and
WebSocket paths do not need a device/path prefix. The random ID is a route,
not an access credential; the Relay authenticates the account separately.

Before setting that environment variable on the Relay:

1. Add a wildcard DNS record, `*.lnz-study.com`, pointing to the same ingress as
   the Relay. An existing exact record such as `remote.lnz-study.com` takes
   precedence. Cloudflare can proxy the wildcard. Do not add records per port.
2. Configure the ingress/reverse proxy to route `p-*.lnz-study.com` to the
   **same Rust Relay HTTP listener** as `remote.lnz-study.com`. Preserve the
   original Host, allow WebSocket upgrades, disable response buffering for
   streaming, and choose long-lived connection timeouts. DNS alone does not
   install this proxy route. The preview Host must not be rewritten to `remote`.
3. Provide TLS for these first-level subdomains on the public edge and, when
   using Full (strict), on the origin. Cloudflare Universal SSL ordinarily covers
   the root and first-level wildcard; `p-id.preview.lnz-study.com` is deeper and
   needs a different certificate arrangement. A dedicated preview domain is also
   supported by choosing it as the base URL.
4. Add the variable to the Relay service environment (on the current server,
   `/opt/remote-codex-rust-relay/relay.env`) and restart that Relay service. This
   configures the Relay, not device Supervisors. Check `/relay/port-mappings/config`
   while signed in, then open a real mapping and test a WebSocket application.

Unset configuration disables opening/enabling from the Web UI and explains the
missing setup. Existing mappings can still be stopped. Mapping/session routing
lives in Relay memory and is restored from Supervisor heartbeats after a Relay
restart; preview tabs may need **Open** again to obtain a fresh gateway session.

For isolated local tests, use `http://preview.localhost:<relay-port>`; Chromium
resolves its subdomains to loopback. Plain HTTP is for local development only.

### Current production ingress

For the `lnz-study.com` preview ingress, Cloudflare proxies
`*.lnz-study.com` to the existing `remote.lnz-study.com` ingress; its active
Universal SSL certificate covers these first-level preview hosts. The origin
keeps the existing main-site HTTP ingress path. This setup does not install a
wildcard origin certificate or change the zone's SSL mode.

On the Relay host, Nginx Proxy Manager runs with host networking. Its mounted
`/opt/nginx-proxy-manager/data/nginx/custom/http.conf` includes
`/data/nginx/custom/remote-codex-preview.conf`. That separate file only matches
`p-<32 lowercase hex digits>.lnz-study.com`, preserving unrelated wildcard
domains and existing NPM-managed services:

```nginx
map $http_upgrade $rc_preview_upgrade {
    default upgrade;
    '' close;
}
server {
    listen 80;
    listen [::]:80;
    server_name "~^p-[a-f0-9]{32}\.lnz-study\.com$";
    access_log off;
    error_log /data/logs/pockymoe-preview-error.log warn;
    client_max_body_size 64m;
    location / {
        proxy_pass http://127.0.0.1:18791;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $x_forwarded_proto;
        proxy_set_header X-Forwarded-Scheme $x_forwarded_proto;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $rc_preview_upgrade;
        proxy_connect_timeout 15s;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_buffering off;
        proxy_request_buffering off;
        proxy_cache off;
        proxy_hide_header Cache-Control;
        proxy_hide_header Expires;
        add_header Cache-Control "private, no-store" always;
        add_header Referrer-Policy "no-referrer" always;
    }
}
```

Access logging is disabled for this route so one-use launch tickets do not enter
the origin's URL logs. `private, no-store` keeps authenticated preview responses
out of shared caches. Any Cloudflare cache rules must also respect this policy.

Validate and gracefully reload with
`docker exec nginx-proxy-manager-app-1 nginx -t` followed by
`docker exec nginx-proxy-manager-app-1 nginx -s reload`. The Relay environment
file contains `POCKYMOE_PORT_PREVIEW_BASE_URL=https://lnz-study.com`; Relay
deployments preserve that file. Only `remote-codex-rust-relay.service` needs a
restart when changing this variable. Existing device Supervisors reconnect.

The pre-configuration environment and custom HTTP configuration are backed up
under `/opt/remote-codex-rust-relay/preview-setup-backup-20261008`, with restricted
permissions. To undo this setup, restore those two files, validate/reload NPM,
remove the now-unused custom preview file, and restart the Relay. Do not copy
credentials from the environment backup into documentation or command output.

Production validation used an isolated temporary account, device Supervisor and
loopback service. Public HTTPS returned the HTML with `private, no-store`, SSE
delivered its first event, and WSS upgraded with an echoed message. Anonymous
access returned 401; stopping the mapping produced 404 after the next device
heartbeat. All test resources were removed. The 27 existing containers retained
their IDs, and existing ingress routes retained their pre-change HTTP statuses.

## Data path and authentication

```text
Browser -- HTTPS --> p-ID.example.com (Relay)
                       |
                       | dedicated binary WebSocket, initiated by Supervisor
                       v
                 Supervisor -- HTTP --> 127.0.0.1:4013
```

The implementation is an ordinary HTTPS reverse proxy; preview content is not
end-to-end encrypted and the Relay can read it. The existing encrypted chat/API
transport stays in use for mapping management. The device requires no inbound
port, and only its explicitly enabled loopback ports can be connected.

The main account session stays at the Relay. **Open** exchanges a one-use,
60-second launch ticket for a mapping-scoped, HttpOnly, Secure, host-only cookie
on the preview origin. The ticket is removed with a redirect before requesting
the application. The preview gateway checks the original account session and
its access to the mapping ([who can open a mapping](#who-can-open-a-mapping)) on
every HTTP request and WebSocket upgrade. Main account and gateway cookies are
filtered out of requests to the local application; the application cannot set
gateway cookies through its HTTP responses. Cross-origin
Origin headers are rejected. Stop, device disconnect and connection replacement
cancel active tunnels. A copied address does not grant another user access.
Long-lived connections recheck session validity every 15 seconds and close after
logout, account disablement, session expiry, ownership revocation or the end of
thread access.

Ordinary HTTP requests reuse a bounded pool of device data connections after the
previous response has been completely consumed. A connection belongs to one
mapping, device connection, account session and lease; it is never shared across
those authorization boundaries. Each request still checks the account session
and its access to the mapping. Idle connections expire after 10 seconds, with at
most six per capability, eight per device and 64 per Relay. Capacity pressure reclaims
idle connections before refusing new traffic; it never preempts active traffic.
Stop/disconnect cancels both active and idle connections. This uses the existing
raw TCP bridge and does not require a new device wire protocol.

HTTP bodies stream with backpressure over binary WebSockets rather than through
the chat control channel. Unfinished responses are not returned to the idle
pool. WebSocket upgrades use dedicated connections; SSE remains streamed.
There are at most 32 enabled mappings per device, 32 simultaneous preview
connections per device and 256 per Relay.
Connecting a data channel has a 10-second deadline; response headers have a
30-second deadline. A successful streaming body/WebSocket can continue beyond
the ordinary JSON API response deadline.

### Opening and latency

CLI `openUrl` links point at the Relay's
`GET /devices/{deviceId}/ports/{mappingId}` document route. With a live account
cookie that may open the mapping and an existing device lease with a future
`leaseExpiresAt`, the Relay redirects directly to a one-use preview launch URL;
redemption sets the preview-origin cookie and redirects to the application. No Web page, JavaScript bundle or cross-origin
API fetch is required for this handoff. Only top-level browser document
navigations can use it: iframe, fetch, prefetch, bearer and URL-token credentials
cannot create a launch this way. Missing/expired lease timestamps or account
authorization fall back to the configured Web sign-in/authorization flow. The existing
CSRF-protected `POST .../port-mappings/{mappingId}/open` remains available to the
Web UI and older clients. Copied preview links carry their lease to the document
route separately from the application path. Reserved authorization parameters
never reach the application.

Issuing an authorized launch starts up to **two** speculative HTTP connections
for exactly its account session, device lease, mapping and device connection.
The first HTTP request can join an in-flight handshake. Reopening the same
authority reuses its existing preparation/idle connections and their original
grant deadline, without extending an existing connection's authorization.
Prewarming sends no HTTP requests to the application, never evicts an existing connection and is bounded
by **four pending handshakes per device / sixteen globally**, in addition to the
ordinary idle/active pool limits. Failed handshakes fall back before sending any
application bytes; an abandoned launch leaves at most the normal ten-second idle
connections. Launch tickets expire after one minute, independently of the
preview grant's eight-hour maximum. Revocation and device disconnect checks also
apply to speculative connections. SSE and application WebSockets retain their
existing streaming/dedicated-connection behavior.

Measure first-time authorization separately from application requests. Cold
access can include sign-in and device approval; an already authorized CLI link
needs only server redirects. Prewarming overlaps the device/Relay handshake with
those redirects, so its benefit depends on their timing. Warm HTTP keepalive
avoids that handshake for subsequent requests; upstreams which explicitly close
their connection still require a new one.
The Relay listener, device data socket and local application socket disable
Nagle to avoid delaying small interactive writes. Upstream response generation, device upload
speed and the network route remain part of the latency. Application compression
headers are preserved. Do not enable shared caching or bypass per-request
authorization to improve speed.

For a reproducible local check, build `pockymoe` and the static Web, then run
`scripts/port-preview-benchmark.spec.ts` with
`--config=scripts/port-preview-benchmark.config.ts --project=desktop-chromium`.
Set `E2E_SECURITY_BINARY` and `E2E_PREVIEW_WEB_DIST` to those artifacts. The test
starts disposable accounts, a Relay, a Supervisor and an application; it measures
authorization, HTTP resources, SSE and WebSocket interactions at configurable
device/Relay delays. It does not inspect production traffic. Its delay proxy
models application bytes only, with loopback TCP and no TLS, so its timings are
not a prediction of a particular user's network. `E2E_PREVIEW_HANDOFF_RTT` sets
the device/Relay delay during the owner handoff, and
`E2E_PREVIEW_REDIRECT_DELAY` adds a controlled browser launch-redirect delay to
measure the overlap. `E2E_PREVIEW_EXPECT_SERVER_HANDOFF=0` is only for comparing a
historical Relay binary without the server handoff; normal runs assert zero Web
scripts and zero POSTs for an already authorized CLI link.

## Bound devices

On a device bound to an account key ([account key trust](account-key-trust.md)),
strict mode decides which mappings the device reports to the Relay and serves:

- **Local mappings**, made through the authenticated local API, such as
  `pockymoe preview create` (also when an agent runs it, including in a thread a
  share recipient controls) or the Supervisor's own Web UI. They survive browser
  removal and `pockymoe bind --reset`. An owner browser that enables the same port
  keeps the mapping local.
- **Owner mappings**, made from the Web through the Relay. Creating one needs a
  signed request from a registered owner browser; share recipients get 403
  `owner_required`. The mapping stays served while that browser's registration
  exists. Removing the browser deletes the mapping and closes its connections, and
  `pockymoe bind --reset` deletes it.
- Mappings created through the Relay before binding are not served while the
  device is bound, until an owner browser enables that port again.

A strict device also opens a data channel only under a live **lease** for that
mapping:

- **Open** first asks the device for a lease with a signed owner request,
  `POST /api/port-mappings/{id}/lease` → `{leaseId, expiresAt}`, then calls
  `POST /relay/devices/{deviceId}/port-mappings/{mappingId}/open?lease=<leaseId>`.
  The Relay keeps the lease with the preview session and adds `leaseId` to every
  `preview.open` frame it sends for that session.
- A lease lasts 8 hours. Asking again from the same browser renews the same lease.
  Stopping the mapping or removing the browser ends it early, and the device closes
  connections whose lease has expired or ended.
- `pockymoe preview create` creates a local lease with the mapping (creating the
  same port again renews it). `preview create`, `preview list` and `preview check`
  report `leaseId` and `expiresAt` and add `lease=` and `leaseExpiresAt=` to
  `openUrl` while it is live. The Web reuses that lease until `leaseExpiresAt` and
  then asks the device for a new one, which only the owner can do.
  Local leases survive browser removal and `pockymoe bind --reset`.
- The signed DSH console request returns a lease for the console mapping.

Without a lease, the Relay refuses an open with 403 `preview_lease_required` unless
the device reports that it is not strict, and the device ignores a `preview.open`
without a live lease. Unbound devices do not use leases; the Web opens without one
only when an unbound device's Supervisor has no lease route.

The Relay still terminates preview HTTPS and sees all preview traffic. It also sees
each lease ID it forwards and can open further connections to that mapping until
the lease ends. Enable only ports whose service you are willing to expose to the
Relay operator.

A Relay whose Web UI is on a separate origin
([self-hosting](self-host-relay.md#separate-web-origin)) still accepts the device
data channel at `/supervisor/port-stream/{ticket}`, so previews work there too.

## Compatibility and limits

The same Rust loopback transport works on macOS, Windows and Linux. On WSL or
containers, loopback means the Supervisor’s own network namespace. Start the
web service there; forwarding arbitrary LAN/Windows-host addresses is outside
this version. IPv6-only loopback services are tried when IPv4 connection fails.

The local upstream must use **HTTP**, not a self-signed HTTPS server or arbitrary
TCP service. Browser-to-Relay and Supervisor-to-Relay use TLS in a public setup.
Absolute localhost HTTP redirects are rewritten to the preview origin, and
application cookie Domain attributes are removed so cookies belong to that
mapping. Application CSP and frame restrictions are preserved.

A web application that hardcodes localhost in JavaScript, checks its public
origin, or uses a second port for hot reload may need application configuration.
Use same-origin WebSocket URLs or explicitly configure the app’s public HMR URL.
Do not disable its host/origin checks globally. Previews open in new tabs.

The gateway authenticates the preview account and validates the browser Origin
before forwarding. It then sends `Host: 127.0.0.1:<port>` and, if an Origin was
present, rewrites it to `http://127.0.0.1:<port>`. Ordinary Vite dev servers allow
IP addresses by default, so a preview hostname does not normally need adding to
`server.allowedHosts`. This is separate from listening on an accessible address:
the service must still accept connections from the Supervisor's loopback.

The public authority is retained in `X-Forwarded-Host`, with HTTPS indicated by
`X-Forwarded-Proto`. Frameworks that trust these headers or validate public URLs
can still require an explicit allowed origin/public URL configuration. Diagnose
the framework's actual rejection before changing its configuration. If a public
host needs allowing, prefer the exact mapping hostname; do not set
`allowedHosts: true` or allow every origin. A framework-specific setup
helper could supply this configuration when starting the service; such a helper
is not part of the current implementation.

See [Vite server host and origin options](https://vite.dev/config/server-options)
and [Next.js allowedDevOrigins](https://nextjs.org/docs/app/api-reference/config/next-config-js/allowedDevOrigins)
for framework-specific controls.

## Validation

`e2e/port-preview.spec.ts` starts isolated, real Rust Relay and Supervisor
processes plus a local HTTP/WebSocket service. It covers encrypted management,
owner isolation, binary POST/GET, redirects, cookie filtering, streamed responses,
WebSocket echo and stopping a live socket, plus confirmation/cancellation of
chat-localhost links. `preview::tests` covers origin/config validation, header
rewrites, reconnect cancellation, failed-upgrade resource cleanup and which
recipients open a thread's mapping until their access ends. Web unit tests
cover local link parsing, confirmation and unavailable configuration.
`e2e/account-key-trust.spec.ts` (preview leases) opens Web and CLI links on a
bound device, the no-lease path on an unbound one, and, on both, a thread
recipient's CLI link for a mapping that thread's agent made, before and after
the share.

## References

- [Cloudflare Universal SSL limitations](https://developers.cloudflare.com/ssl/edge-certificates/universal-ssl/limitations/)
- [Set-Cookie semantics](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Set-Cookie)
- [Vite host and WebSocket configuration](https://vite.dev/config/server-options.html)

## Agent CLI workflow

`pockymoe preview create --port 4013 --label "App preview"` reserves a
persistent address without requiring a listening service. Its JSON includes
`id`, `port`, exact `hostname`, `origin`, stable `url`, `openUrl`, and the local
preview lease (`leaseId`, `expiresAt`, see [bound devices](#bound-devices)), and
`access` with `threadId` ([who can open a mapping](#who-can-open-a-mapping)).
The `openUrl` enters the browser login/open flow; it contains no account
session or launch ticket. Creation for an existing port is idempotent. The CLI uses its
normal local Supervisor credential and obtains the configured base domain from
the authenticated Relay connection, rather than guessing a hostname.

Configure only required exact-host/origin entries, start HTTP on the mapped
loopback port, then run `pockymoe preview check 4013`. Use
`--path /app` for a particular HTTP route, or `--websocket-path /ws` to test a
known WebSocket handshake. Results distinguish connection failure/timeouts,
explicit host-check rejection, ordinary HTTP errors and WebSocket failures.
Body inspection is bounded; redirects are not followed and the probe cannot
leave loopback. This checks the local upstream, not public DNS/TLS/login or
actual browser hot reload. A rejected handshake can also mean the framework
requires a token/subprotocol, so do not diagnose it as a broken Relay by itself.

Use `preview list` to recover addresses and `preview stop PORT_OR_ID` to revoke
one. Stop does not terminate the application process. Through the Relay, a
mapping opens for the device owner and, when a thread's agent created it, for
that thread's recipients. The CLI acts with the device's local authority: an
agent in a thread that a share recipient controls can create, list and stop any
mapping and its local lease like any local user.
The embedded `pockymoe skill` documents the required agent workflow.
