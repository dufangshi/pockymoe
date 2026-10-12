# Agent notes

Pockymoe is a self-hosted control plane for coding agents. The Supervisor, relay
and CLI are Rust under `crates/`; the Web UI is `apps/supervisor-web`. The shared
thread UI comes from the separate `dufangshi/pockymoe-thread-ui-rust` repository,
checked out at `pockymoe-thread-ui/`. Documentation index: [docs/README.md](docs/README.md).

## Code

- Keep runtime and HTTP in Rust under `crates/`; do not add a Node or TypeScript supervisor.
- ACP is the harness path. Put command or capability differences in a thin adapter under `crates/runtime/src/acp/` (`catalog.rs`, `capabilities.rs`).
- JSON field names are camelCase.
- Some `remote-codex`, `REMOTE_CODEX_` and `remoteCodex` identifiers must keep their old names. Read [docs/rename-pockymoe.md](docs/rename-pockymoe.md) before renaming one.
- Do not copy Android, iOS or Windows app sources into this tree.

## Checks and commits

- Run checks in proportion to the change: the affected crates or test names, formatting and compilation. Do not run `cargo test --workspace`, full browser suites, platform matrices or release dry-runs unless the user asks. See [docs/ci.md](docs/ci.md).
- For Web E2E, follow the [focused-e2e skill](.agents/skills/focused-e2e/SKILL.md): chosen specs and one explicit browser project.
- After a change passes its checks, commit the relevant files in each affected repository. Keep unrelated work out of the commit.

## Releases and deployment

- Follow the [release-runtime skill](.agents/skills/release-runtime/SKILL.md) for runtime releases and installed-version problems. GitHub Releases are authoritative. A release is immutable and contains every platform; do not publish npm packages.
- The Windows Device Manager is released independently. Bump or release it only when its own UI, installer, tray/startup or self-update behavior changes, never for a runtime fix, and mark its releases `--latest=false`.
- The public Web UI is a GitHub Pages site that `pages-deploy.yml` builds on pushes to `main` with the shared UI at the SHA in `thread-ui.sha`; the relay is API-only.
- After a Web or shared UI change, push the shared UI and update `thread-ui.sha` on `main`; after a relay change, dispatch `relay-deploy.yml` from `main`.
- Update a running Supervisor through its management Check/Update API (Settings), not by replacing its executable or killing it.

## Documentation and notes

- `docs/` holds only maintained documentation. Follow the rules in [docs/README.md](docs/README.md), and update docs in the same commit as the behavior they describe.
- Write plans, task notes, investigation logs and test evidence in `.scratch/`, which git ignores, never in `docs/`.
