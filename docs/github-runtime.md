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
assets, Web ZIP, version file and SHA256SUMS. Then make sure `thread-ui.sha` on `main`
holds the same full shared UI SHA. The public Web UI and its `setup.sh`/`setup.ps1`
are a static GitHub Pages site that `pages-deploy.yml` builds from `main` whenever the
Web, the setup scripts or `thread-ui.sha` change. The setup scripts install the latest
published release, so a script change that needs a new runtime option fails on the
public site until that release is published. `relay-deploy.yml` deploys only the
API-only relay; dispatch it from `main` when the relay changed.

The runtime release is marked GitHub latest. Independent Device Manager/tool releases
must use `--latest=false` so they cannot replace the runtime's latest download alias.
If GitHub cannot be reached, Check/Update reports the error and preserves the installed
runtime; npm and unverified mirrors are never silent fallbacks.
