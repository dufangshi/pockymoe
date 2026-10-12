# Rename: Remote Codex → Pockymoe

The product, CLI, crates, workspace packages, UI text and docs are named
Pockymoe. Identifiers that older installations, browsers, relays, native apps
or users' harness configurations still read keep their old names, and the
runtime accepts the old command and environment names.

## Name forms

| Before | After |
| --- | --- |
| Remote Codex (prose, UI, docs) | Pockymoe |
| `remote-codex` command and Cargo package | `pockymoe` |
| `remote-codex-{protocol,runtime,supervisor,relay}` crates | `pockymoe-…` (`pockymoe_…` in Rust paths) |
| `@remote-codex/*` workspace packages | `@pockymoe/*` |
| `REMOTE_CODEX_*` environment variables | `POCKYMOE_*` |
| GitHub repositories `dufangshi/remoteCodex`, `dufangshi/remote-codex-thread-ui-rust` | `dufangshi/pockymoe`, `dufangshi/pockymoe-thread-ui-rust` |
| `remote-codex-thread-ui` checkout directory | `pockymoe-thread-ui` |
| `.remote-codex/agents/*.md` role files | `.pockymoe/agents/*.md` |
| `[remoteCodex …]` prompt markers, i18n keys | `[Pockymoe …]`, `pockymoe…` |

## Compatibility

Accepted under both names:

- **Commands.** Native activation links both `pockymoe` and `remote-codex` in `~/.local/bin`. Managed agents get both names in their `cli-bin` directory.
- **Environment.** At startup the CLI adopts every `REMOTE_CODEX_*` variable as `POCKYMOE_*`; a value already set under the new name wins. It then drops the old keys.
  - The exception is DSH credential references `REMOTE_CODEX_DSH_<PROFILE>`, which stay as they are.
  - Agent and hook processes receive `THREAD_ID`, `URL`, `TOKEN` and `COMMAND_ID` under both prefixes.
- **Device config.** Native setup reads `relay-supervisor.json` under either key prefix and writes the `REMOTE_CODEX_*` keys. Windows Device Managers and pre-rename runtimes read only those.
- **Role files.** `.pockymoe/agents` is read first, then `.remote-codex/agents`.
- **Legacy npm installs.** Management still recognizes a global `remote-codex` npm package.

Kept unchanged, with the reason:

| Identifier | Why |
| --- | --- |
| GitHub release asset names (`remote-codex-<platform>`, `remote-codex-win32-x64-msvc-cli.exe`, `remote-codex-web.zip`) | Installed native updaters (0.12.75+) and `setup.sh`/`setup.ps1` download them by name |
| Installed executable `native/<version>/remote-codex`, `~/.local/share/remote-codex/native` | Service units run `native/current/remote-codex`; the updater switches `current` in place |
| The retired npm launcher in `npm/` and its tests and live drivers | Legacy devices run published copies of it, by its old names, until native setup migrates them; native setup bridges `remote-codex.mjs` |
| HPKE info and exporter labels, HKDF info strings, `__remote_codex_legacy_sha256__` | E2E encryption with older devices; stored TOTP and setup tokens; legacy password logins |
| `__remote_codex_runtime_migrations` table | Renaming re-runs migrations |
| Session, MFA, trusted-browser and OAuth cookies; the preview proxy's `remote_codex_relay_` filter | Renaming signs everyone out and drops MFA trust |
| `X-Remote-Codex-Auth-Realm`, `x-rcd-*` headers, `rcd_` tokens | Wire protocol between mixed versions; Device Managers validate `rcd_` |
| localStorage, sessionStorage, IndexedDB `remote-codex-transport-v1`, window/service-worker events, `Symbol.for('remote-codex.…')` | User preferences, sign-in state, trust-on-first-use pins, cross-bundle stores |
| `remoteCodexNative`, `messageHandlers.remoteCodex`; `name` in `/api/version` | Native app bridges and identification until the apps ship an update |
| Plugin IDs `remote-codex.*`; manifest field `remoteCodex` | Persisted enablement; third-party manifests |
| Artifact fence and type `remote-codex-artifact` / `remote-codex.artifact`; tool `remote_codex_render_molecule` | Present in existing transcripts |
| Codex provider `remote_codex`; Grok `remote-codex/<model>` aliases and the `remote-codex` model; DSH provider `remote-codex`, bridge `remote-codex-bridge`, `REMOTE_CODEX_DSH_*` references | Written into users' harness configuration; renaming would orphan entries and break session resume |
| `~/.remote-codex/` (relay config, transport identity, SQLite stores) and `~/.local/share/remote-codex/` | Shared with the Device Manager and with every installed runtime |
| `remote-codex-supervisor.service`, `com.remote-codex.supervisor`, `com.remotecodex.update.*`, tmux `remote-codex-relay-supervisor` | Installed units, labels and sessions that installed updaters manage |
| Device Manager internals | Assembly and executable names, mutex, control pipe, Run entry, `%LOCALAPPDATA%\RemoteCodex`, runtime-state keys, the npm package it installs |
| Relay host paths, units and env file; `REMOTE_CODEX_RELAY_DEPLOY_*` secrets | Live infrastructure, renamed only in a coordinated rollout |
| `/var/lib/remote-codex-relay` in `Dockerfile.relay` | Data volume of deployed images |
| `remote.lnz-study.com`, `remote-codex.lnz-study.com` | Deployed domains; renamed separately |
| Old repository URLs in installed runtimes, the retired npm launcher and historical docs | GitHub redirects them; see "Repository rename" below |

## Regenerating the rename

The rename commits are produced by `node scripts/rename-pockymoe.mjs <root>`. The script:

- uses a keep list for the identifiers above;
- skips release notes, incident reports and operations logs;
- skips the Device Manager and the retired npm launcher.

To rebase onto a newer `main`:

1. Drop the "codemod output" commit.
2. Re-run the script on both repositories.
3. Regenerate `pnpm-lock.yaml` and `Cargo.lock`, and run `cargo fmt --all`.
4. Replay the later commits, which hold the hand-written compatibility code.
5. In the thread UI, rebuild the committed `dist/` bundles.

For other branches after the rename lands:

- Run the same script on the branch, then merge `main`.
- In local checkouts, rename `remote-codex-thread-ui` to `pockymoe-thread-ui` and run `pnpm install`.

## Landing and release

1. Merge the thread UI and runtime branches together. CI on `main` clones the thread UI `main`.
2. Publish a runtime release with `runtime-release.yml` and the merged thread UI SHA.
   - Assets keep their names, so native devices update through Web Settings as usual.
3. Publish the same version as the final `remote-codex` npm package with `npm-final-release.yml`.
   - npm devices install it through their existing Update, then press Update again to move to the native runtime.
4. Set `thread-ui.sha` on `main` to the same thread UI SHA, which deploys the public Web to GitHub Pages, and dispatch `relay-deploy.yml` from `main` for the relay.

## Repository rename

The repositories were renamed on 2026-10-10, after the renamed runtime was released and deployed:

- `dufangshi/remoteCodex` → `dufangshi/pockymoe`
- `dufangshi/remote-codex-thread-ui-rust` → `dufangshi/pockymoe-thread-ui-rust`

GitHub redirects git, web, API and release-download URLs from the old names, so installed updaters, `setup.sh`/`setup.ps1` copies and the retired npm launcher keep working. Never create a new repository under an old name: that would break the redirect.

The compiled-in repository URL (`distribution/releases.rs`), the setup scripts, the workflows, the relay Dockerfile and the notification claim use the new names. Release notes, incident reports and the frozen npm packages keep the old URLs.

Release asset names can switch to `pockymoe-*` only after a release whose updater accepts both names has been deployed to existing devices.

## Follow-ups

- **Windows Device Manager.** Release it with the renamed UI and icon.
- **Infrastructure and data directories.** Migrate the Device Manager internals, data directories, relay host paths and units, and deploy secrets in coordinated releases.
- **Native apps.** Update the iOS, Android and macOS apps in their own repository before renaming their bridges.
- **Trademarks.**
  - "Pocky" is a registered trademark of Ezaki Glico; check the name before promoting it.
  - The icon carries no company marks, but its centre character is a fan-made DeepSeek persona.
