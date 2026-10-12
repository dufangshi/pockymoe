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
administrator at `https://relay.example.com/relay-admin`.

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
[quick start](../README.md#quick-start).

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

Repeat step 1 with the new version and restart the service. Devices update
themselves from Settings; the relay serves setup commands for its own version.

### Accounts without an account key and unbound devices

Accounts, factors, devices, shares and threads are kept. An account that has no
account key yet signs in once with **First sign-in since the security upgrade**
on the sign-in page, the only time its password reaches the relay, and then
within 10 minutes **Secure your account**: a password of at least 12 characters
and the recovery code. Until then the relay allows only the session, sign-out and
device requests; sessions from before the upgrade are restricted the same way.
Creating the vault deletes the relay's old password hash. Accounts that use
Google or GitHub sign in as usual and choose an unlock password. The relay
administrator account signs in at `/relay-admin` with its configured password and
has no account key.

A device that is not bound keeps working without protection (**Not protected ·
Relay trusted**) until its owner chooses **Protect this device** and runs the
shown `pockymoe bind` command on the device, or reruns its setup command. The
device must run a runtime version that supports binding. Step-by-step:
[migration](account-key-trust.md#migration).

The released `remote-codex-web.zip` offers the upgrade choice. When every account
has been secured, build the Web UI from source with
`VITE_POCKYMOE_LEGACY_UPGRADE=0` set (for example the
[static build](#building-the-static-web-ui)) to remove it. Accounts that still have
no vault, including those reset by you before they were secured, then cannot sign
in with a password.

## Password resets

The relay stores each user's account key only inside a vault encrypted with the
user's password and recovery code. Nobody with access to the server, including
you, can recover it.

Resetting a user's password in the admin panel sets a temporary password of at
least 12 characters. It works once, still requires the user's second factor if
they have one, and stops the old password from signing in. The user chooses
**Use an admin reset password** on the sign-in page, then enters their recovery
code and a new password. Without the recovery code they can only start over with
a new account key and bind each device again locally (`pockymoe bind --reset`,
then **Protect this device**). For an account that was not yet secured, the reset
sets an ordinary password that the user enters with **First sign-in since the
security upgrade** to secure the account.

## Backups

Back up the whole data directory. It holds `relay-store.sqlite` and
`session-secret`. The secret also encrypts stored two-factor and device
credentials: if it is lost or replaced, those can no longer be decrypted. Use a
consistent SQLite backup rather than copying a database that is being written.

The database also holds the encrypted account vaults, each with its previous
revision. If it is lost, users must start over and bind every device again. A
restored backup can contain older vaults, which browsers that have already seen a
newer vault refuse ("The relay served an older account vault").
More detail: [relay security operations](relay-security-operations.zh.md).

## Separate Web origin

By default the relay serves the Web UI and its API from
`POCKYMOE_PUBLIC_BASE_URL`. A relay that serves the Web UI can also change it,
and that code handles your password and account key. To remove this power, serve
the Web UI as static files from another origin and run the relay in API-only mode:

```sh
POCKYMOE_PUBLIC_BASE_URL=https://relay.example.com   # API, OAuth callbacks, devices
POCKYMOE_WEB_ORIGIN=https://app.example.com          # static Web UI
```

`POCKYMOE_WEB_ORIGIN` must be an HTTPS origin (HTTP only for `localhost`) without
a path; it defaults to the origin of `POCKYMOE_PUBLIC_BASE_URL`. The relay then:

- uses the Web origin for WebAuthn: its host is the RP ID of sign-in passkeys.
  Passkeys are bound to their RP ID, so if users registered passkeys on the
  current host, keep the Web UI on that host and move the API to a new one;
- accepts only the Web origin in the `Origin` check for state-changing requests
  and WebSocket upgrades;
- answers CORS on `/relay/*` for exactly the Web origin, with credentials and
  `Vary: Origin`. Preflights allow the methods and headers the Web sends
  (`content-type`, `accept`, `range`, `authorization`, `if-none-match`,
  `x-remote-codex-auth-realm`, `x-rcd-*`), and responses expose the headers it
  reads. Other origins get no CORS headers and a 403 preflight;
- sends the browser back to the Web origin after OAuth sign-in; OAuth callbacks
  stay on `POCKYMOE_PUBLIC_BASE_URL`;
- when the two origins differ, serves no HTML or scripts: it keeps `/relay/*`,
  `/healthz`, `/supervisor/tunnel` and the preview data path
  `/supervisor/port-stream/*`, redirects other `GET` and `HEAD` requests (including
  `/setup.sh`) to the same path on the Web origin and answers 404 to other methods.
  It needs no `POCKYMOE_RELAY_WEB_DIST_DIR`.

Relay cookies remain host-only on the relay host, `Secure`, `HttpOnly` and
`SameSite=Lax`. Browsers send them with the Web's requests only when both
origins are on the same site (registrable domain), such as `app.example.com` and
`relay.example.com`. Different ports on `localhost` work for development.

Browser keys and the account data each browser has verified live in the Web
origin's storage. Moving the Web UI to another origin makes every browser sign in
again as a new browser.

### Building the static Web UI

In a source checkout prepared as in [development](development.md), with the
shared UI at `pockymoe-thread-ui/`:

```sh
VITE_POCKYMOE_RELAY_ORIGIN=https://relay.example.com \
  pnpm --filter @pockymoe/supervisor-web build:static
```

The build selects relay mode at compile time, so it needs no inline script. Add
`VITE_POCKYMOE_LEGACY_UPGRADE=0` to leave out the **First sign-in since the
security upgrade** choice. Besides the app, `apps/supervisor-web/dist/` contains:

- `_headers`: a CSP whose `connect-src` allows only the Web origin, the relay
  origin and its `wss:` host and whose `script-src` adds only
  `'wasm-unsafe-eval'` (password derivation runs Argon2id as WebAssembly), plus
  `Service-Worker-Allowed: /` for `/assets/*` and `no-cache` for `index.html`;
- `_redirects`: the SPA fallback `/* /index.html 200`;
- `setup.sh` and `setup.ps1`, because device setup commands download them from
  the Web origin;
- `STATIC_HOSTING.txt`, a summary of these requirements.

Hosts such as Cloudflare Pages apply `_headers` and `_redirects` directly; on
other hosts, translate them into response-header and rewrite rules. Serve the
directory over HTTPS on the Web origin. Without `VITE_POCKYMOE_RELAY_ORIGIN`
the build calls its own origin.

### Existing devices

A device connects to `/supervisor/tunnel` on the relay URL saved at setup. If the
Web UI takes over the host that devices were installed with, keep that path on
that host routed to the relay (for example with a proxy or CDN rule), or those
devices go offline. Setup commands copied from the Devices page use the relay
origin and pass the Web origin with `--web-origin`.
