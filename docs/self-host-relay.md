# Self-hosting a relay

[简体中文](self-host-relay.zh-CN.md)

A relay is the website you sign in to. It connects your browser to your devices
and serves the device setup commands. It only forwards traffic: conversations,
files and terminals are end-to-end encrypted between your browser and each
device. A device bound to your account key also refuses commands that your
browsers did not sign; see [account key trust](account-key-trust.md).

You need:

- a Linux server (x64 or ARM64, glibc 2.28 or newer) reachable from the internet;
- a domain name pointing at it, for example `relay.example.com`;
- HTTPS in front of the relay. Sign-in cookies are `Secure`, so browsers only
  accept them over HTTPS (or on `localhost`).

## 1. Download the relay

The relay is the same `pockymoe` executable as the device runtime. It runs from
a single directory:

```sh
mkdir -p ~/pockymoe-relay && cd ~/pockymoe-relay
repo=https://github.com/dufangshi/pockymoe/releases
version=$(curl -fsSL $repo/latest/download/runtime-version.txt)
asset=remote-codex-linux-x64-gnu   # or remote-codex-linux-arm64-gnu
curl -fLO $repo/download/v$version/$asset
curl -fLO $repo/download/v$version/remote-codex-web.zip
curl -fLO $repo/download/v$version/SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS
install -m 0755 $asset pockymoe
rm -rf web && mkdir web && (cd web && unzip -q ../remote-codex-web.zip)
```

## 2. Configure

Create `~/pockymoe-relay/relay.env` and keep it private (`chmod 600 relay.env`):

```sh
# The administrator account, created on first start.
POCKYMOE_ADMIN_USERNAME=admin
POCKYMOE_ADMIN_PASSWORD=choose-a-long-password
POCKYMOE_ADMIN_EMAIL=you@example.com

# The exact public HTTPS address of this relay.
POCKYMOE_PUBLIC_BASE_URL=https://relay.example.com

POCKYMOE_RELAY_HOST=127.0.0.1
POCKYMOE_RELAY_PORT=8788
POCKYMOE_RELAY_DATA_DIR=/home/you/pockymoe-relay/data
POCKYMOE_RELAY_WEB_DIST_DIR=/home/you/pockymoe-relay/web
```

Optional settings:

| Variable | Effect |
| --- | --- |
| `POCKYMOE_GOOGLE_OAUTH_CLIENT_ID`, `POCKYMOE_GOOGLE_OAUTH_CLIENT_SECRET` | Show **Continue with Google**. Create an OAuth client in Google Cloud with the redirect URI `https://relay.example.com/relay/auth/oauth/google/callback`. |
| `POCKYMOE_GITHUB_OAUTH_CLIENT_ID`, `POCKYMOE_GITHUB_OAUTH_CLIENT_SECRET` | Sign in with GitHub; callback `…/relay/auth/oauth/github/callback`. |
| `POCKYMOE_RELAY_REGISTRATION_ENABLED=false` | Close public sign-up. It is open by default. |
| `POCKYMOE_RELAY_REGISTRATION_PASSWORD` | Require an invite code to sign up. |

Registration, approval and sign-in providers can also be changed later by the
administrator at `https://relay.example.com/relay-admin`, the
[admin console](relay-admin.md). With approval required, a
sign-up waits there; rejecting it keeps the request in the review history and
deletes the encrypted account data it carried.

## 3. Run it as a service

```ini
# ~/.config/systemd/user/pockymoe-relay.service
[Unit]
Description=Pockymoe relay
After=network-online.target

[Service]
WorkingDirectory=%h/pockymoe-relay
EnvironmentFile=%h/pockymoe-relay/relay.env
ExecStart=%h/pockymoe-relay/pockymoe relay
Restart=always

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now pockymoe-relay
loginctl enable-linger "$USER"   # keep it running after you log out
curl -fsS http://127.0.0.1:8788/healthz
```

## 4. Add HTTPS

Any reverse proxy with WebSocket support works. With [Caddy](https://caddyserver.com),
which obtains certificates automatically:

```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:8788
}
```

Open `https://relay.example.com`, create your account, and continue with the
[quick start](../README.md#quick-start). This relay serves the Web UI itself; to
keep the Web code out of the relay's hands, host it separately
([separate Web origin](#separate-web-origin)).

## Docker

In a checkout of this repository, `Dockerfile.relay` builds the relay and its Web UI from source:

```sh
docker build -f Dockerfile.relay -t pockymoe-relay .
docker run -d --name pockymoe-relay --restart unless-stopped \
  -p 127.0.0.1:8788:8788 -v pockymoe-relay:/var/lib/remote-codex-relay \
  --env-file relay.env pockymoe-relay
```

In the container, leave out `POCKYMOE_RELAY_HOST`, `POCKYMOE_RELAY_DATA_DIR` and
`POCKYMOE_RELAY_WEB_DIST_DIR`; the image sets them.

## Updating

Repeat step 1 with the new version and restart the service. With a separate Web
origin, also rebuild and publish the static Web UI from the same version. Devices
update themselves from Settings; the setup scripts install the latest published
release.

### Accounts without an account key and unbound devices

Accounts, factors, devices, shares and threads are kept. An account that has no
account key yet fails its first ordinary sign-in and then chooses **Continue
with this password**, the only time its password reaches the relay. **Secure your
account** follows: a password of at least 12 characters and the recovery code.
Until then the relay allows only the session, sign-out and device requests;
sessions from before the upgrade are restricted the same way and are asked to
sign in again.
Creating the vault deletes the relay's old password hash. Accounts that use
Google or GitHub sign in as usual and choose an unlock password. The relay
administrator account signs in at `/relay-admin` with its configured password and
has no account key.

A device that is not bound keeps working without protection (**Not protected**,
trusting the relay) until its owner protects it: with **Protect all devices** on the
Devices page, or with **Protect this device** and the shown `pockymoe bind`
command run on the device, or by rerunning its setup command. The device must run
a runtime version that supports binding. Step-by-step:
[migration](account-key-trust.md#migration).

The released `remote-codex-web.zip` offers the upgrade. When every account
has been secured, build the Web UI from source with
`VITE_POCKYMOE_LEGACY_UPGRADE=0` set (for example the
[static build](#building-the-static-web-ui)) to remove it. An account that still
has no vault then needs a [password reset](#password-resets) to sign in and secure
itself.

## Password resets

The relay stores each user's account key only inside a vault encrypted with the
user's password and recovery code. Nobody with access to the server, including
you, can recover it.

Resetting a user's password in the admin panel sets a temporary password of at
least 12 characters. It works once, still requires the user's second factor if
they have one (even on a trusted browser), and stops the old password from
signing in. The user chooses **Forgot password?** → **Admin password** on the
sign-in page, then enters their recovery code and a new password. Without the
recovery code they can only start over with a new account key and bind each device
again (`pockymoe bind --reset` on the device, then **Protect this device**). For
an account that was not yet secured, the same sign-in leads straight to **Secure
your account**; this also works in Web builds without the security upgrade.

## Backups

Back up the whole data directory. It holds `relay-store.sqlite` and
`session-secret`. The secret also encrypts stored two-factor and device
credentials: if it is lost or replaced, those can no longer be decrypted. Use a
consistent SQLite backup rather than copying a database that is being written.

The database also holds the encrypted account vaults. If it is lost, users must
start over and bind every device again. A restored backup can contain older
vaults, which browsers that have already seen a newer vault refuse ("The relay
served an older account vault").
More detail: [relay security operations](relay-security-operations.zh.md).

## Separate Web origin

A relay runs in one of two modes:

- **Single origin**, the default: the relay serves the Web UI from
  `POCKYMOE_RELAY_WEB_DIST_DIR` and its API, both on `POCKYMOE_PUBLIC_BASE_URL`.
  This is the simplest setup, but the relay can then change the Web code, and that
  code handles your password and account key
  ([threat model](account-key-trust.md#threat-model)).
- **Separate Web origin**: the Web UI is a static site on its own origin and the
  relay serves only its API. The public Pockymoe service runs this way, with the
  Web UI on GitHub Pages at `https://app.lnz-study.com` and the relay at
  `https://remote.lnz-study.com`.

For the second mode, set both origins on the relay:

```sh
POCKYMOE_PUBLIC_BASE_URL=https://relay.example.com   # API, OAuth callbacks, devices
POCKYMOE_WEB_ORIGIN=https://app.example.com          # static Web UI
```

`POCKYMOE_WEB_ORIGIN` (also read under its old name `REMOTE_CODEX_WEB_ORIGIN`)
must be an HTTPS origin (HTTP only for `localhost`) without a path; it defaults to
the origin of `POCKYMOE_PUBLIC_BASE_URL`. The relay then:

- uses the Web origin for WebAuthn: its host is the RP ID of sign-in passkeys.
  Passkeys are bound to their RP ID, so passkeys registered on another host stop
  working;
- accepts only the Web origin in the `Origin` check for state-changing requests
  and WebSocket upgrades;
- answers CORS on `/relay/*` for exactly the Web origin, with credentials and
  `Vary: Origin`. Preflights allow the methods and headers the Web sends
  (`content-type`, `accept`, `range`, `authorization`, `if-none-match`,
  `x-remote-codex-auth-realm`, `x-rcd-*`), and responses expose the headers it
  reads. Other origins get no CORS headers and a 403 preflight;
- sends the browser back to the Web origin after OAuth sign-in; OAuth callbacks
  stay on `POCKYMOE_PUBLIC_BASE_URL`;
- when the two origins differ, serves no HTML or app scripts: it keeps `/relay/*`,
  `/healthz`, `/supervisor/tunnel` and the preview data path
  `/supervisor/port-stream/*`, redirects other `GET` and `HEAD` requests (including
  `/setup.sh`) to the same path on the Web origin and answers 404 to other methods.
  It needs no `POCKYMOE_RELAY_WEB_DIST_DIR`. The only script it still serves is a
  worker that retires the service worker of a Web UI the relay used to serve
  ([old Web origin](account-key-trust.md#old-web-origin)).

Relay cookies remain host-only on the relay host, `Secure`, `HttpOnly` and
`SameSite=Lax`. Browsers send them with the Web's requests only when both
origins are on the same site (registrable domain), such as `app.example.com` and
`relay.example.com`. Different ports on `localhost` work for development.

Device files reach the page through the Web origin's service worker, which
encrypts the requests the page makes itself: `fetch` calls and subresources such
as the images in Markdown files, chat photos and the Explorer's image preview.
Safari's worker cannot always read the browser key the page stored (WebKit bugs
183167 and 312279). When the worker finds no browser key, or the device answers
401, it hands a read (`GET`/`HEAD`) to the page that asked, which makes it with
its own key and returns the response (marked `x-rcd-page-fetch` so the worker
lets it pass). Only a page, never the worker, opens the unlock prompt for a
missing browser key.
A frame or tab that opens a relay URL is a navigation on the relay origin, which
that worker never sees and which the relay refuses to frame. The Web therefore
reads previews a browser would navigate to, such as PDFs, through the encrypted
API and shows them from a `blob:` URL (`frame-src 'self' blob:`); browsers without
a PDF viewer in frames, such as Chrome on Android, show **Open** and **Download**
buttons instead.

Browser keys and the account data each browser has verified live in the Web
origin's storage. Moving the Web UI to another origin makes every browser sign in
again as a new browser. To move an existing relay with users and devices, follow
[moving to a separate Web origin](account-key-trust.md#moving-to-a-separate-web-origin).

### Building the static Web UI

In a source checkout prepared as in [development](development.md), with the
shared UI at `pockymoe-thread-ui/`:

```sh
VITE_POCKYMOE_RELAY_ORIGIN=https://relay.example.com POCKYMOE_WEB_DOMAIN=app.example.com \
  pnpm --filter @pockymoe/supervisor-web build:static
node scripts/check-static-web.mjs   # with the same two variables set
```

`POCKYMOE_WEB_DOMAIN` is the Web host name and is required. The build selects
relay mode at compile time, so it needs no inline script. Add
`VITE_POCKYMOE_LEGACY_UPGRADE=0` to leave out the security upgrade (**Continue
with this password**). Besides the app, `apps/supervisor-web/dist/` contains:

- in `index.html`, the Content Security Policy as a `<meta http-equiv>` tag, with
  every directive of the header version except `frame-ancestors`, and a
  `no-referrer` referrer policy;
- `404.html`, a copy of `index.html` that serves as the SPA fallback on hosts such
  as GitHub Pages;
- `relayServiceWorker.js` at the site root, so its scope is `/` without a
  `Service-Worker-Allowed` header;
- `_headers`: the CSP, whose `connect-src` allows only the Web origin, the relay
  origin and its `wss:` host, whose `script-src` adds only `'wasm-unsafe-eval'`
  (password derivation runs Argon2id as WebAssembly) and which sets
  `frame-ancestors 'none'`; `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer` and `X-Frame-Options: DENY`; and `no-cache` for
  `index.html` and the service worker;
- `_redirects`: the SPA fallback `/* /index.html 200`;
- `CNAME`, the custom domain for GitHub Pages;
- `setup.sh` and `setup.ps1`, because device setup commands download them from
  the Web origin;
- `STATIC_HOSTING.txt`, a summary of these requirements.

Hosts such as Cloudflare Pages apply `_headers` and `_redirects` directly; on
other hosts, translate them into response-header and rewrite rules. GitHub Pages
applies neither: it uses `CNAME` and `404.html`, and only the meta tags and the
app's own frame check protect the page
([what that leaves out](account-key-trust.md#static-hosting)). Serve the
directory over HTTPS on the Web origin. Without `VITE_POCKYMOE_RELAY_ORIGIN` the
build calls its own origin.

This repository publishes the public Web UI with `.github/workflows/pages-deploy.yml`.
A fork can reuse it: set the repository variable `POCKYMOE_RELAY_ORIGIN`, change
`POCKYMOE_WEB_DOMAIN` in the workflow, pin the shared UI commit in `thread-ui.sha`,
and choose GitHub Actions as the Pages source. Each deployment keeps the earlier
builds' assets for 72 hours (`scripts/carry-web-assets.mjs`), so pages left open
across a deployment keep working.

### Existing devices

A device connects to `/supervisor/tunnel` on the relay URL saved at setup. If the
Web UI takes over the host that devices were installed with, keep that path on
that host routed to the relay (for example with a proxy or CDN rule), or those
devices go offline. Setup commands copied from the Devices page use the relay
origin and pass the Web origin with `--web-origin`.
