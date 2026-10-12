# CI scope

Ordinary pull requests run `PR checks` (`.github/workflows/ci.yml`). Checks are
selected from the PR's changed paths, and a newer push cancels the older PR run.
There is no duplicate push workflow after merging to `main`.

| Changed area | Automatic checks |
| --- | --- |
| Rust, Cargo configuration, migrations | Linux formatting and `cargo check --workspace --locked` |
| Supervisor Web, shared/plugin packages | Web typecheck and Vitest; build the external thread UI dependency required by Web |
| Native bootstrap, legacy npm launcher or Node scripts | Native SH/legacy launcher, updater, and historical publishing-script unit tests; no product installation or release build |
| GitHub Actions workflows | actionlint; ShellCheck warnings/errors (not style or informational suggestions) |
| Documentation, agent instructions, browser-only E2E files, Windows Device Manager | No automatic product build; agents validate relevant changes locally |

Root Node manifests/lockfiles and TypeScript configuration select their affected
package checks. Web changes do not compile Rust. npm-only changes need neither
pnpm installation nor Web compilation.

`PR checks complete` aggregates selected job results, including failures and
cancellations, and can be used as a stable required status check. Direct pushes
do not run PR checks; use a PR for automatic validation. Relevant local regression
tests remain part of implementing changes; passing compilation is not a substitute
for a test of a changed behavior.

## Full validation: explicit request only

`Full compatibility (manual)` retains the previous compatibility workflow at
`.github/workflows/platform-compatibility.yml`. It has only `workflow_dispatch`:
no PR, push, schedule, or automatic caller. It runs every retained suite without
path filtering, even when invoked on a documentation-only commit:

- Rust workspace tests on Linux, macOS, and Windows; Linux formatting and Clippy.
- Windows Device Manager build, verification, and downloadable artifact.
- Web typecheck, tests, and production build.
- Rust release binary build and native bootstrap verification.

An agent may dispatch it only when the user explicitly asks for full validation.
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

Runtime publication uses the manual `runtime-release.yml` workflow: workspace release gate, four native platforms and pinned Web build, complete asset assembly, then immutable GitHub publication. No npm publication job remains.
