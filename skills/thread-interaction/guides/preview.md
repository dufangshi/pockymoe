# pockymoe guide preview

Web previews: show a local web app to the user.

## Show a local website to the user

When the user asks to view an app, or your task calls for delivering a runnable
web preview, reserve its address **before starting the HTTP service**:

```bash
pockymoe preview create --port 4013 --label "App preview"
# Read JSON: hostname (exact host), origin, url and openUrl.
# Configure the framework if needed, then start HTTP on 127.0.0.1:4013.
pockymoe preview check 4013
pockymoe preview list
# Optional: probe a known WebSocket endpoint (not a complete browser HMR test).
pockymoe preview check 4013 --websocket-path /ws
# Revoke when no longer needed; this does not stop the HTTP process.
pockymoe preview stop 4013
```

- Creation reserves a mapping, **not a running server**. Repeating it for an
  enabled port keeps the same address. Stop then recreate gets a new address;
  update exact-host configuration accordingly. Avoid stopping another task's
  mapping; mappings are device-wide, not thread-local. Creating an enabled port
  from this thread makes this thread decide who else can open it.
- Bind to loopback in the Supervisor's own network namespace. For Vite use a
  fixed port (`--strictPort`) so automatic port fallback cannot break the mapping.
- If the installed framework requires a host/origin allowlist, add only the
  returned `hostname` or `origin`, matching that setting's expected format;
  preserve existing entries. Examples: Vite `server.allowedHosts`, webpack
  `devServer.allowedHosts`, Django `ALLOWED_HOSTS`; Next.js dev-origin restrictions
  use `allowedDevOrigins`. Inspect the installed version. Do not use `*`,
  `allowedHosts: true`, disable host checks or permit a whole shared domain.
  The gateway forwards a loopback Host and original `X-Forwarded-Host`, so many
  apps need no host change. Do not edit allowlists without a framework need.
- By default the app also receives `Origin: http://127.0.0.1:<port>` (and a
  same-origin Referer rewritten to match), not the preview origin. An app that
  checks its own public origin (CSRF middleware, an Origin allowlist, OAuth
  redirect checks) then rejects POSTs or WebSockets that work on localhost:
  create the mapping with `--origin preserve` and allow the returned `origin`.
  A 403 the gateway itself returns carries `x-pockymoe-gateway`.
- Diagnose before changing configuration: `serviceNotRunning` means check the
  process/port/bind address; `hostRejected` means inspect the precise rejection;
  `httpError` alone does not prove an allowlist problem. A successful HTTP check
  does not certify public DNS, TLS, account access or hot reload.
- If the page works but hot reload fails, inspect the actual browser WebSocket
  endpoint, protocol, port, auth token and subprotocol. Prefer same-origin WS.
  For confirmed public-endpoint issues, Vite uses `server.ws` (older versions:
  `server.hmr`); use the assigned host, public scheme/port, never the user's
  localhost. WebSocket probes require the actual endpoint and may fail when
  framework-specific auth/subprotocols are required.
- Give the user **`openUrl`** as the clickable entry. It authenticates in their
  browser and opens `url`; the mapped hostname can also initiate browser login.
  The device owner and the people this conversation is shared with can open it
  (`access: "thread"`); a mapping made outside a thread is owner-only. The link
  carries an 8-hour lease (`expiresAt`); create the port again for a fresh one.
  Never copy account tokens or one-use launch tickets into reports or CLI
  commands. CLI output contains neither.
- A disconnected Supervisor, disabled preview gateway or old runtime/Relay must
  be fixed explicitly. Do not invent a domain or bypass authentication. See
  `pockymoe preview --help`; an older installation may need updating.
