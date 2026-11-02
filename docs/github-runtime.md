# Native installation and GitHub runtime releases

GitHub Releases in `dufangshi/pockymoe` are the authoritative runtime versions.
Pockymoe no longer publishes runtime/npm packages. npm remains a possible
installation mechanism for third-party Agent/ACP dependencies, not for this runtime.

## Install and migrate

Copy the device's setup command from the Devices page. SH (macOS/Linux) or PowerShell
(Windows x64) resolves `releases/latest/download/runtime-version.txt`, downloads the
matching versioned Rust executable, verifies SHA256 and its `version`, then runs native
`setup`. Native setup downloads and verifies the same release's Web bundle. Linux
requires glibc 2.28 or newer; supported targets are Linux x64/ARM64, Apple Silicon,
and Windows x64. Downloads report progress; failures identify the failed stage.
Without relay arguments the same scripts run `setup --local`
([local mode](local-mode.md)), which keeps `~/.remote-codex/local-supervisor.json`
and its own service next to any relay device.

The native installer preserves `~/.remote-codex/relay-supervisor.json`, its database,
credentials and transport identity. An unrelated occupied port or mismatched device
configuration is rejected before installation; it never replaces another device's
configuration. Reusing an enrollment code requires the saved enrollment receipt.
Legacy database URLs only recover to the original SQLite file when its database and
transport identity both exist; ambiguous or relative paths require repair.

Devices still on npm (0.12.74 or older) only check the npm registry. Version 0.12.77
is also published as the final `remote-codex` npm package (`npm-final-release.yml`,
`scripts/prepare-npm-bridge.py`): its retired launcher runs the GitHub executable of
the same version. After the device installs it through Settings → Update, Settings
shows Update once more; that schedules the native worker, which installs the GitHub
release and moves the service to it. Windows Device Manager devices from before 0.2.0 keep their legacy bootstrap until
explicit migration; Manager 0.2.0 uses native setup and the same management APIs. No later npm
releases are published.

Alternatively, rerun the same setup command once for an existing npm device. The verified native
runtime stages itself, installs a small compatibility bridge beside the old writable
npm launcher, and invokes the existing management Update API. That API journals and
pauses active turns before the independent native worker restarts the service. The
old helper is backed up. Read-only/unrecognized legacy helpers fail before shutdown;
those require explicit service recovery instead of silently stopping the device.
The old npm package is not removed automatically. On Unix the native CLI is available
at `~/.local/bin/pockymoe` and `~/.local/bin/remote-codex` when those paths are free; restart your shell or use its
absolute path if an older npm CLI still wins PATH. Managed agents always receive the
running native executable's directory on PATH.

Native setup uses systemd user services or launchd where available. Other Linux
hosts use a detached session; Windows uses an independent WMI-created process.
Detached installations require a process owner to restart them after reboot.
Windows Device Manager 0.2.0 supplies tray/sign-in startup and resolves
`native/current.json` on each launch. Its Check/Update controls use native management
APIs. See [Windows Device Manager](windows-device-manager.md) for installation,
legacy migration and its independent release boundary.

## Update contract

Web Settings uses the same device-scoped management API. Check reads the stable
GitHub version; Update copies a Rust worker outside the running Supervisor, stages
and verifies the native/Web release, drains active turns, verifies ownership of the
old PID/service, switches the versioned installation, and verifies HTTP health plus
Relay reconnection. Durable recovery markers resume only turns interrupted by this
maintenance operation. Queued user input and user-stopped threads retain their behavior.

State lives under `~/.local/share/remote-codex/native/`; per-device protected update
journals/logs live beside its SQLite database under `updates/`. No sudo or npm prefix
mutation is used after migration. Download/verification failure leaves the old runtime
intact. Once the new binary has been launched, it is retained on failure because it
may have migrated SQLite; automatically starting an older binary could corrupt recovery.

Node is checked/prepared only when a user installs an Agent or ACP adapter that needs
it. The private Agent Node runtime is checksum-verified from nodejs.org, and does not
modify the user's global Node/npm. ACP adapters install into a fresh versioned prefix
and activate only after their executable is verified, avoiding npm ENOTEMPTY leftovers.

## Publish

Record the runtime commit, stable version and published shared UI full SHA. Update the
root/Cargo version with `node scripts/set-version.mjs VERSION`, update Cargo.lock, run
relevant checks, commit and push. Do not bump historical npm manifests or Device Manager.

```sh
gh workflow run runtime-release.yml --ref main \
  -f channel=latest -f thread_ui_sha=FULL_PUBLISHED_UI_SHA
gh run watch RUN_ID --interval 30 --exit-status
```

The release workflow tests the workspace as its release gate, builds all four native
platforms and the pinned Web concurrently, then assembles all assets. It rejects a
candidate missing already-published runtime ancestry. Every version is immutable;
existing assets may only be verified, never overwritten with another build. `dry-run`
assembles artifacts without publishing. Verify the resulting version, all four native
assets, Web ZIP, `setup.sh`/`setup.ps1`, version file and SHA256SUMS. Then make sure `thread-ui.sha` on `main`
holds the same full shared UI SHA. The public Web UI and its `setup.sh`/`setup.ps1`
are a static GitHub Pages site that `pages-deploy.yml` builds from `main` whenever the
Web, the setup scripts or `thread-ui.sha` change. The setup scripts install the latest
published release, so a script change that needs a new runtime option fails on the
public site until that release is published. Each release also carries the
`setup.sh`/`setup.ps1` it was built with, so the README's
`releases/latest/download/setup.sh` always matches the latest runtime and needs
no Web origin. `relay-deploy.yml` deploys only the
API-only relay; dispatch it from `main` when the relay changed.

The runtime release is marked GitHub latest. Independent Device Manager/tool releases
must use `--latest=false` so they cannot replace the runtime's latest download alias.
If GitHub cannot be reached, Check/Update reports the error and preserves the installed
runtime; npm and unverified mirrors are never silent fallbacks.

Windows detached launchers keep PowerShell preflight errors strict, then allow
native stderr during execution and propagate the native exit code. Windows
PowerShell 5.1 otherwise treats ordinary tracing output as a terminating error.
Maintenance stops the already-drained headless Supervisor by its validated PID
with `taskkill /F`; a console-close request cannot stop a windowless process.

## Portable release addresses

The runtime release contains no built-in production Relay or Web address.
`setup.sh` and `setup.ps1` fetch assets from GitHub; without connection arguments
an install is local, and relay mode requires an explicit Relay address and setup
code/device token. The Rust runtime reads connection settings at runtime.

`runtime-release.yml` builds Web assets with `POCKYMOE_RUNTIME_RELEASE=1` and an
empty `VITE_POCKYMOE_RELAY_ORIGIN`. The Vite release flag also overrides local
`.env` values, so the portable bundle uses its own origin instead of inheriting
a hosted deployment. A self-hosted separate-origin Web build may explicitly set
`VITE_POCKYMOE_RELAY_ORIGIN=https://relay.example.com`; this is deployment
configuration, not a default in published runtime assets.

Our Pages deployment deliberately keeps its own configured origin. Repository
secrets used by the deployment workflows are:

| Secret | Purpose | Example |
| --- | --- | --- |
| `POCKYMOE_RELAY_ORIGIN` | Existing Relay HTTPS origin, also fed into the Pages Web build | `https://relay.example.com` |
| `POCKYMOE_WEB_DOMAIN` | Pages CNAME, asset carry-forward host, and Relay redirect/preflight target | `app.example.com` |
| `POCKYMOE_LEGACY_RELAY_ORIGIN` | Former Relay origin whose isolation the deployment checks | `https://legacy-relay.example.com` |

Relay deployment also requires the following repository secrets (available to
its `relay-rust-canary` job):

| Secret | Purpose |
| --- | --- |
| `POCKYMOE_RELAY_SSH_FINGERPRINT` | Expected SSH server `SHA256:…` fingerprint for all SSH/SCP actions |
| `POCKYMOE_RELAY_HOSTNAME` | Expected remote `hostname` checked before modifying the service |
| `REMOTE_CODEX_RELAY_DEPLOY_HOST` | SSH transport host |
| `REMOTE_CODEX_RELAY_DEPLOY_USER` | SSH login user |
| `REMOTE_CODEX_RELAY_DEPLOY_PASSWORD` | SSH authentication password |
| `REMOTE_CODEX_RELAY_DEPLOY_PORT` | SSH transport port |

`--mask --relay-host` validates these as well as the three origins before the
first network boundary check; missing values stop deployment. The fingerprint
and host identity are not stored in workflow source. Raw and derived fingerprint
values and hostnames are masked. Existing secret names keep their compatibility
spelling. Pages requires only the three address secrets, not the six SSH secrets.

The deploy archive includes checksum-verified proxy configuration/audit helpers.
Before modifying the service, deployment privately captures the active NPM
`nginx -T` dump and audits the exact Relay virtual host and its loaded includes.
The audit requires that every Relay upstream appends `$remote_addr` through
`$proxy_add_x_forwarded_for`, or overwrites X-Forwarded-For with `$remote_addr`.
It rejects forwarding a client-supplied header by itself and accounts for Nginx's
all-or-none `proxy_set_header` inheritance. Dynamic/unrecognized upstream or
real-IP policies stop deployment rather than being assumed safe.

A public health read with forged forwarding headers and DNS inspection detects
Cloudflare when its edge headers or current published IP ranges are observed.
If detected, Nginx must restore `$remote_addr` from `CF-Connecting-IP` and trust
only the current official Cloudflare ranges (complete IPv4/IPv6 coverage).
Otherwise the audit stops before service replacement: trusting only loopback
would leave a CDN edge as every person's client IP. Deployment does **not**
automatically trust CDN headers/ranges in the Relay or rewrite Nginx. Official
ranges come from Cloudflare's HTTPS endpoints during preflight. A missing or
unverifiable NPM dump, failed public probe, unidentified `Via` intermediary or
unavailable trust reference also stops the workflow.

After that audit, `configure-relay-proxy.py` makes a private pre-change
`relay.env` backup (0700 parent, 0600 file) and appends
`POCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1` **only when both the canonical and legacy
keys are absent**. Existing values, including empty/invalid ones, remain untouched;
the auth preflight validates them and refuses unsafe/missing loopback trust.
Repeated deployment steps do not duplicate the setting or overwrite the original
backup. Later database backup preserves this pre-change environment snapshot;
rollback restores it before restarting the original binary. No manual server
environment edit is needed to adopt the missing default.

**Verification boundary:** tracked docs establish Cloudflare for the preview
wildcard and show its append-style Nginx location. The generated NPM Relay
virtual-host configuration is not tracked. A health response does not expose
Relay's resolved client IP, so the forged-header probe detects CDN presence but
does **not** independently confirm end-to-end attribution. The audit combines
active configuration with the tested rightmost-untrusted resolver. Absence of
Cloudflare headers/DNS does not exclude another hidden CDN. Such a CDN needs an
explicit reviewed real-IP policy; it must not be treated as individual visitors.
The actual deployed chain is verified only when this preflight runs on that host;
local/CI fixture tests do not establish the live infrastructure state.

See [mobile sign-in](mobile-host.md) for client IP resolution, anonymous/provider
limits, consent and remaining scheme risks.

Origins must have no trailing slash, path, query, credentials or port; the Web
hostname is lowercase. Set all three secrets to the corresponding current
hosts before using `pages-deploy.yml` or `relay-deploy.yml`. The preflight refuses
missing or malformed settings (both workflows require all three). The Relay
workflow passes its secrets into the SSH steps and checks the same systemd service, environment origins, redirect and
legacy boundary as before. No server configuration is rewritten by this change.
Repository secrets are available to the Pages build and the Relay job in the
`relay-rust-canary` environment; separate environment copies are not required.
An environment secret with the same name overrides the repository secret and
must be set consistently or removed. The `github-pages` deployment job does not
need separate copies. Do not use repository variables for these three addresses:
variables are not masked in public Actions logs. Each build/deploy preflight
registers `::add-mask::` for the secret values **and derived hostnames and Web/WS
origins** before they reach curl, SSH, systemd output or Web build diagnostics.
Existing logs from older runs are not retroactively redacted.
`POCKYMOE_DEV_ALLOWED_HOSTS` is an optional comma-separated environment variable
for local Vite development hosts; no deployment host is hard-coded in Vite.

Every PR runs `python3 scripts/check-production-address.py --tracked`, including
changes limited to docs. The guard compares candidate domains against a SHA-256
hash and never includes the forbidden spelling in its implementation or errors.
It checks ASCII, UTF-16, escaped dots/Unicode, and decompressed ZIP members.
The only tracked-source exception is the exact pre-existing Windows Device
Manager manifest, frozen by file hash because that product has an independent
release contract. Artifact scans have **no** exception. The runtime release
package job scans every platform binary, both setup scripts, checksums/version
files and the decompressed `remote-codex-web.zip` before uploading release assets.
Run it locally with `python3 scripts/check-production-address.py release-assets`.

The Windows Device Manager remains unchanged: its manifest supplies the initial
Relay URL for missing/invalid settings and a separate fixed destination for both
Open Web buttons. Removing the default without a replacement onboarding flow
would leave fresh/recovered installs without a usable Relay; clearing the portal
URI would also break Open Web. Existing saved Relay settings are independent of
that initial default. This exception must be handled in that product's release.

This removes the address from the current runtime release inputs, not from Git
history, existing tags/source archives, old release assets, old CI logs/artifacts,
installed copies or the configured hosted Pages output. Automatic source archives
also retain the independent Windows manifest, including for new tags. Those remain unchanged;
no history rewrite or retroactive release replacement is performed.
