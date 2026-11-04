# CI scope

Ordinary pull requests run `PR checks` (`.github/workflows/ci.yml`). Checks are
selected from the PR's changed paths, and a newer push cancels the older PR run.
The final merged SHA on `main` also produces the aggregate used by release and
deployment gates; direct pushes are checked, but should be replaced with PRs
before branch protection is enabled.

| Changed area | Automatic checks |
| --- | --- |
| Rust, Cargo configuration, migrations | Linux formatting and `cargo check --workspace --locked` |
| Protocol crate, shared package, Cargo manifests/lockfile | Protocol wire tests and Rust/TS thread discriminant contract tests |
| Supervisor Web, shared/plugin packages, `thread-ui.sha` | Web typecheck and Vitest; build the external thread UI dependency required by Web |
| Native bootstrap, legacy npm launcher or Node scripts | Native SH/legacy launcher, updater, and historical publishing-script unit tests; no product installation or release build |
| Treer integration packaging | Bundled setup freshness, shell syntax and offline Python installer/configuration tests |
| GitHub Actions workflows | actionlint; ShellCheck warnings/errors (not style or informational suggestions) |
| Documentation, agent instructions, browser-only E2E files, Windows Device Manager | No automatic product build; agents validate relevant changes locally |

Documentation-only and version-only changes retain the address guard, classifier
regressions and aggregate, without product builds.

Root Node manifests/lockfiles and TypeScript configuration select their affected
package checks. Web changes do not compile Rust. npm-only changes need neither
pnpm installation nor Web compilation.

`PR checks complete` aggregates selected job results, including failures and
cancellations, and is the only main-repository check intended to be required.
Version-only diffs parse the allowed root package/workspace/owned lockfile
version fields; any dependency, removal or other field change selects its normal
tier. Unknown inputs select Rust, Web and script checks conservatively.
Shared UI validation consumes the exact full SHA in `thread-ui.sha` and verifies
it belongs to the published shared UI main history, matching deployment. Relevant local regression
tests remain part of implementing changes; passing compilation is not a substitute
for a test of a changed behavior.

## Full validation: explicit request only

`Full compatibility (manual)` retains the previous compatibility workflow at
`.github/workflows/platform-compatibility.yml`. It accepts `workflow_dispatch` with `validation=full`, or an explicit `full-ci`
PR label through the PR aggregate. There is no schedule or automatic full run.
It runs every retained suite without
path filtering, even when invoked on a documentation-only commit:

- Rust workspace tests on Linux, macOS, and Windows; Linux formatting and Clippy.
- Windows Device Manager build, verification, and downloadable artifact.
- Web typecheck, tests, and production build.
- Rust release binary build and native bootstrap verification.

An agent may dispatch it or add `full-ci` only when the user explicitly asks for full validation.
Ordinary instructions to fix, test, commit, push, or merge do not request it.
Use a pushed ref, record the resulting run ID and `headSha`, and watch that run:

```bash
gh workflow run platform-compatibility.yml --ref <pushed-branch-or-tag>
gh run watch <run-id> --interval 30 --exit-status
```

For example, after the user requests a full check of main:

```bash
gh workflow run platform-compatibility.yml --ref main
```

The compatibility workflow does not publish packages, create a GitHub Release, or deploy
services. It does not include the separate browser E2E suite; select browser tests
using the focused-e2e skill, with a full browser run only upon explicit request.
The release and relay deployment workflows remain manual and keep their release
gates. Do not invoke a release dry-run as a routine check of a CI-only change.

`Web Pages Deploy` (`.github/workflows/pages-deploy.yml`) is the one automatic
deployment: a push to `main` that changes the Web, its packages, the setup scripts
or `thread-ui.sha` builds the static Web UI with the shared UI at that pinned SHA,
checks it with `scripts/check-static-web.mjs` and publishes it to GitHub Pages. It
can also be dispatched manually from `main`. `relay-deploy.yml` builds and deploys
only the relay.

The isolated chat-image pipeline has its own config and starts a real API-only
relay, static Web build and fake-harness Supervisor. Run it with assigned ports
and one browser project, after building the Rust binary and shared UI:

```sh
E2E_RELAY_PORT=19260 E2E_WEB_PORT=19261 E2E_EXPECT_DIAGNOSIS=1 \
  pnpm exec playwright test --config e2e/chat-image-pipeline.config.ts --project=desktop-chromium
```

`E2E_SUPERVISOR_BINARY` selects a legacy runtime; `E2E_ELECTRON_EXECUTABLE`
selects an independently packaged desktop client with an isolated profile.
`E2E_STATIC_WEB_DIST` optionally reuses a static build configured for that relay
port. The test sends a generated PNG through the real composer, activates a
worker update, checks native media without a controller, switches threads
without reloading, moves/restores the real
attachment to verify diagnosis and Retry, and damages the thumbnail cache to
verify original fallback. Image traces and video stay off for private local inputs;
visual captures refuse private inputs and remain in ignored `.scratch/`.

Runtime publication uses the manual `runtime-release.yml` workflow: `validation=auto` by default: tests for affected packages and their workspace
consumers, four native platforms and the published Web pin, complete asset assembly
and scanning, then immutable GitHub publication. The diff is computed from the
latest trusted runtime release SHA to the final SHA, including changes before the
version bump. Database, migration, authentication and update paths select their
affected packages like every other change; none forces a workspace run.
`validation=full` explicitly selects the full workspace. `channel=dry-run` builds
and verifies the complete artifacts without publishing. The Windows worker smoke
compiles a tiny harness with rustc using the exact production launcher source;
it does not relink the full Supervisor test binary in release mode. Ancestor,
four-platform, hash/version, bootstrap, address and immutability gates remain
mandatory. No npm publication job remains.
