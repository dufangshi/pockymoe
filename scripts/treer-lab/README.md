# Local Treer integration lab

This lab runs authenticated Treer Proxy and PostgreSQL plus one enrolled Linux
container, with a native Treer Web dev server. The container has Node 22, Treer
Host/Controller/CLI and a Pockymoe binary. A Treer-managed Agent creates the
Pockymoe hub as a Managed App; all Pockymoe turns use the fake runtime.

## Start and check

Run from the Pockymoe worktree root. Requirements: Docker socket access, Rust,
Node 22+, Corepack, and the existing Treer checkout. No sudo is used.

```bash
getent group docker
docker info --format '{{.ServerVersion}}'

# Supply an already-built, matching Pockymoe Web distribution.
# This was the initial-main distribution used for this machine's baseline:
TREER_LAB_POCKYMOE_WEB_DIST=/home/ubuntu/dev/remoteCodex/apps/supervisor-web/dist \
  scripts/treer-lab/up.sh
node scripts/treer-lab/e2e.mjs
```

`up.sh` compiles the four Treer binaries and Pockymoe, prepares an isolated Docker
build context, installs only Treer Web's locked dependencies, builds the device
image, creates a real admin session, invites/registers a synthetic lab user,
creates an organization/workspace and enrolls the device. The baseline driver
checks authentication, enrollment, terminal execution, a managed CLI caller and
the fake hub Managed App. It leaves the hub App running for the integration phase.

Default ports: Proxy `127.0.0.1:18787`, Web `127.0.0.1:15173`, hub
`127.0.0.1:18880` **inside the device only**. `TREER_LAB_PROXY_PORT` and
`TREER_LAB_WEB_PORT` can override the first two; use the same values for every
subsequent command. `TREER_LAB_SOURCE` overrides the Treer checkout path.

The existing session on this machine has a Docker socket ACL. On machines with
new group membership and an installed `sg`, prepend `TREER_LAB_USE_SG=1` to lab
commands; this runs Docker as `sg docker -c ...` without a re-login.

Build preparation works without Docker daemon access:

```bash
TREER_LAB_POCKYMOE_WEB_DIST=/path/to/matching/web/dist \
  scripts/treer-lab/up.sh --prepare-only
scripts/treer-lab/up.sh --skip-prepare
```

## Stop, resume and reset

```bash
scripts/treer-lab/down.sh
scripts/treer-lab/up.sh --skip-prepare
node scripts/treer-lab/e2e.mjs

# Deletes this worktree's lab containers and three named volumes only.
scripts/treer-lab/reset.sh --confirm
```

The Compose project name includes a hash of this worktree path. `down.sh`
preserves database, enrollment, threads and saved workload intent. The device has
a stable hostname because Treer pins an installation to its hostname. Reset
retains build artifacts and evidence; a subsequent up creates fresh identities.
The scripts stop only recorded native processes whose PID, start time and
command match this lab. An occupied Web port fails rather than reusing a service.

After a cold down/up, a new random Proxy admin password is delivered over stdin.
The admin issues a password-reset token for the existing lab user, and the lab
immediately exchanges it for a new random password/session. Treer rate-limits
password reset issuance; a rapid series of cold cycles may require waiting for
that upstream limit or explicitly resetting this lab.

## Full hub/attach check

The full driver runs against the integrated H/A commit. Initial main
has no `pockymoe attach`; `--full` fails explicitly instead of pretending that a
terminal Agent verifies AIS. Build Pockymoe from the integrated commit in this
worktree, or supply a binary from that exact integrated checkout with
`TREER_LAB_POCKYMOE_BIN`. Supply the matching Web distribution as well.

After replacing the lab image, stop/start the lab so restored workloads use the
new stable binary path:

```bash
scripts/treer-lab/down.sh
TREER_LAB_POCKYMOE_WEB_DIST=/path/to/integrated/web/dist \
  scripts/treer-lab/up.sh
node scripts/treer-lab/e2e.mjs --full
```

The saved launch profile runs:

```text
/usr/local/bin/pockymoe attach --provider codex \
  --hub http://127.0.0.1:18880 \
  --hub-token-file /workspace/hub/treer-attach.token
```

`TREER_LAB_ATTACH_ARGS_JSON` overrides that non-secret argv list if the integrated
CLI contract changes. Never include credentials in it. Hub and attach use the
same stable `/usr/local/bin/pockymoe` binary. The hub wrapper sets
`POCKYMOE_MODE=local`, `POCKYMOE_HOST_MODE=hosted`, `POCKYMOE_HOSTED_BY=treer`,
`POCKYMOE_E2E_FAKE_RUNTIME=1`, an isolated database/workspace and the host-managed
restart strategy.

The full driver checks two Agents sharing the hub port with distinct interfaces;
default queued CLI prompts and completion notification; two transcript pages;
idle/working/busy status; a slow fake turn stopped before completion; harness
release and interruption; same-thread/history rebind; and hub App replacement
without replacing the attach process. Its focused browser test uses a **real
Treer user cookie**, checks anonymous tunnel rejection, loads the bound thread
page, and rejects another bound thread's API through the first Agent entry.

To rebuild and replace only the enrolled lab device while preserving the running
Proxy, PostgreSQL and session broker:

```bash
corepack pnpm --filter @pockymoe/supervisor-web build
node scripts/treer-lab/lab.mjs refresh-device
node scripts/treer-lab/e2e.mjs --full
```

`refresh-device` compiles/stages the current checkout, rebuilds the shared lab
image, replaces only the device container and starts its existing enrolled
service. Its `--skip-prepare` option uses the already staged artifacts.

Only one browser project and one spec are selected:

```bash
node /home/ubuntu/dev/remoteCodex/reference/treer/web/node_modules/@playwright/test/cli.js \
  test tunnel.spec.mjs --config scripts/treer-lab/playwright.config.mjs \
  --project=desktop-chromium --list
# The --full driver runs the same command without --list after preparing fixtures.
```

Install that Playwright version's Chromium if it is unavailable. Traces, video,
storage-state files and credential-bearing failure output are disabled.

### Bidirectional thread/Agent synchronization

For the candidate that exposes all bound hub threads through each authenticated
Agent page, use the focused sync driver instead of `e2e.mjs --full` (that older
driver asserts the original single-thread browser policy):

```bash
node scripts/treer-lab/bidirectional-e2e.mjs
```

Start with the candidate binary/Web already installed in the fake hub and two
bound Agents recorded in `.local/treer-lab/browser.json`. The driver verifies
authentication and scoped runtime config, creates its own managed CLI worker,
and selects only `bidirectional.spec.mjs` with `desktop-chromium`. It checks UI
creation/prompt/rename, Treer Launch, tab/split navigation, hidden unbound threads,
favorites and stale storage, deletion of another Agent and the page's own Agent,
management/AIS isolation, and attach waiting/error diagnostics in Treer's PTY.
The PTY check briefly creates a separate fake hub on device port 18881 with only
Codex enabled, verifies the disabled-Claude error, then deletes that App and Agent.
The driver stops its worker afterward and leaves the lab
running. It does not replace the device container or change the setup-created App.
Evidence goes to `.scratch/treer-bidirectional/`; set
`TREER_LAB_SYNC_EVIDENCE_DIR` to select another ignored evidence directory.

## Treer Issues App

`issues-e2e.mjs` runs Treer's unmodified Issues App (`apps/issues` from the Treer
checkout) against two new bound threads on the fake hub. It installs the candidate
binary, the strict CLI fixture (now also allowing `curl`) and the Issues source,
restarts the fake hub App, and creates the `issues` App if absent with
`ISSUES_REMINDER_FIRST=1m`. It checks that each thread's `/v1/me` is its own Agent
rather than the hub App, that an Agent-assigned issue reaches the assignee's inbox
without a turn, that the App's reminder wakes the idle assignee with the App hint,
and that the assignee's comment and status change are attributed to its Agent.
It deletes its two Agents afterward and leaves the Issues App running. The run takes
about three minutes because of the reminder interval.

```bash
TREER_LAB_POCKYMOE_BIN=/path/to/candidate/target/debug/pockymoe \
  node scripts/treer-lab/issues-e2e.mjs
```

`TREER_LAB_ISSUES_SOURCE` selects another Issues checkout and
`TREER_LAB_ISSUES_EVIDENCE_DIR` the ignored evidence directory (default
`.scratch/treer-issues/`).

## Strict registry CLI fixture

The fake runtime has no OS harness process. To exercise the real CLI with the
thread credential and Treer identity ACP would receive, explicitly set
`POCKYMOE_E2E_FAKE_CLI_DRIVER=/opt/lab/thread-driver.sh` on the **fake hub only**
and stage `thread-driver.sh` and `thread-driver.mjs` in `/opt/lab/`. A prompted
thread starts a fixture worker with its own `launch_env()`; credentials stay in
memory. Jobs and replies use `/workspace/l2-thread-jobs/THREAD_ID/`. Publish a
`UUID.request.json` atomically with `{ "cli": "pockymoe", "args": [...] }`
(`treer` is also supported); inspect `UUID.reply.json` and its exit code.
Never put credentials in jobs. The worker exits when its hub process exits.
`POCKYMOE_E2E_FAKE_CLI_JOB_DIR` and `POCKYMOE_E2E_FAKE_CLI_BIN` select isolated
host fixtures for standalone tests.

`strict.spec.mjs` checks a bound parent's child grouping and own child tunnel,
anonymous rejection, and the metadata-only hub App. Prepare a private
`fixture.json` under `.scratch/treer-strict-l2/` with `parentThread`,
`parentAgent`, `parentName`, `childThread` and `childAgent` (stable IDs only),
or set `TREER_LAB_STRICT_EVIDENCE_DIR` to its directory. Run only this spec:

```bash
node /home/ubuntu/dev/remoteCodex/reference/treer/web/node_modules/@playwright/test/cli.js \
  test strict.spec.mjs --config scripts/treer-lab/playwright.config.mjs \
  --project=desktop-chromium
```

## One-command setup against a real release

`setup-e2e.mjs` exercises the public setup flow through two plain command Agents
created by the authenticated lab user. It uses the unmodified bundled script and
genuine v0.13.2 assets, verifies the native installation and Managed App, compares
profile IDs and App PID across the rerun, launches the created Codex profile, and
checks existing lab workloads before the launch. It leaves both hubs running.
The device image includes curl and Python 3.11+; existing containers built before
Python was added need the prerequisite installed before this test.

Prepare the host mirror once, retaining the authoritative checksum file:

```bash
mkdir -p .local/treer-lab/setup-mirror/v0.13.2
gh release download v0.13.2 -R dufangshi/pockymoe \
  -p SHA256SUMS -p remote-codex-linux-x64-gnu -p remote-codex-web.zip \
  -p runtime-version.txt -D .local/treer-lab/setup-mirror/v0.13.2
node scripts/treer-lab/setup-e2e.mjs
```

The driver verifies host checksums, starts a temporary HTTP mirror container on
the internal lab network with no published ports, and forwards its release root
to device loopback port 19000. `POCKYMOE_RELEASE_BASE_URL` is set only in the two
setup command Agents. Both HTTP helpers are removed on completion. No runtime
build is substituted for the release, and no Treer credentials are copied into
the command. An existing installation must be v0.13.2; setup refuses replacement.

When attach binds successfully, the driver selects only `setup-tunnel.spec.mjs`
and `desktop-chromium` using the existing Playwright config. The spec checks real
authentication, the relative redirect, final bound-thread URL and rendered prompt.
A missing Codex harness is recorded separately; an Agent that exits before binding
fails the interface check and blocks the browser and prompt checks. Evidence is
written to `.local/treer-lab/setup-evidence.json`, separate from the fake-runtime
full-driver evidence. Task reports and archived test evidence belong in `.scratch/`.

## Isolation and credentials

Runtime/build state is under `.local/treer-lab/`, already gitignored, with a
private parent directory. The lab normalizes upstream `compose.yaml` before
starting it: it drops App/NATS/updater sidecars, upstream wildcard ports and
Proxy environment, then permits only a loopback Proxy publication. PostgreSQL
and the device share an internal Docker network. Proxy also joins a second
bridge because Docker suppresses published ports on internal-only networks.
No Docker socket is mounted in a lab container.

The device forwards `127.0.0.1:8787` to `proxy:8787` with `socat`. Enrollment
uses that loopback origin and explicitly installed native binaries, preserving
the production cleartext installer guards. No Treer source/config changes are
needed. The device uses Treer's documented plain network mode inside Docker,
without privileged containers or host networking.

Generated admin/user passwords, invitation/reset/enrollment tokens and session
cookies are never written to lab JSON, command argv, Docker environment metadata
or evidence. A mode-0600 Unix socket exposes the session to the local test process
from an in-memory broker; `down.sh` stops it. Treer's own PostgreSQL and native
enrollment stores retain their normal product state in private lab volumes.
Managed workload credentials come from Treer Host into the worker/attach process
environment, and are never copied into a job file. Do not dump container
environments, Controller config, session sockets or database tables into logs.

## Optional native fallback

On this Ubuntu 26.04 machine, a diagnostic native run works without Docker. It
downloads PostgreSQL 18/libpq packages with `apt-get download`, extracts them
under the lab directory without installing anything, initializes a local test
database and starts only loopback listeners. It creates an authenticated Treer
user/workspace, enrolls a native Host, verifies terminal and managed CLI behavior,
and starts a fake Pockymoe Managed App. It terminates its own process groups on
completion. This does **not** verify the required Docker-device boundary.

```bash
scripts/treer-lab/native-smoke.sh
# If the Docker lab is already running, use a different Proxy port:
TREER_LAB_PROXY_PORT=28787 scripts/treer-lab/native-smoke.sh
```

It also needs free loopback ports 15432 (PostgreSQL), 18790 (Controller) and 18880
(native fake hub). PostgreSQL uses local test trust authentication; Treer Proxy
authentication remains enabled. It refuses occupied ports.

## Check evidence

Initial product source: Pockymoe `4bec710db44302cafaac63094636850a537958f0`,
Treer `1a512715a935b49f262ad957ddf344bc5a8944eb`. No Treer code changes or pushes.

Checks performed on 2026-10-14 UTC:

| Command/check | Result |
| --- | --- |
| `cargo build --locked -p treer-proxy -p treer-agent-server -p treer-agent-host -p treer-cli --target-dir .local/treer-lab/treer-target` in the Treer checkout (absolute target path) | Passed |
| `cargo build --locked -p pockymoe` | Passed |
| `corepack pnpm install --ignore-workspace --frozen-lockfile` in Treer Web | Passed |
| Shell `bash -n` and Node `node --check` for all lab scripts | Passed |
| Normalized upstream Compose config | Three services; device/Postgres internal; only Proxy loopback publication |
| Docker image build and authenticated bootstrap/enrollment | Passed |
| `node scripts/treer-lab/e2e.mjs` | Five baseline checks passed |
| `TREER_LAB_PROXY_PORT=28787 scripts/treer-lab/native-smoke.sh` | Seven native fallback checks passed |
| `scripts/treer-lab/reset.sh --confirm` | Removed only this lab's containers/volumes; fresh up/enrollment passed |
| `scripts/treer-lab/down.sh`, `scripts/treer-lab/up.sh --skip-prepare`, then baseline driver | Passed with the same persisted workspace/enrollment |
| Focused Chromium command above with `--list` | One spec, one `desktop-chromium` test discovered; **not executed** |
| Matching Playwright Chromium installation and headless launch preflight | Passed; browser prerequisites are ready |
| `node scripts/treer-lab/e2e.mjs --full` | Expected preflight failure: initial-main binary lacks `attach`; full checks remain pending integrated H/A code |

The lab writes non-secret build provenance in `build.json`, baseline evidence in
`evidence.json`, native evidence in `native-evidence.json`, and full results in
`full-evidence.json`. JSON stays in ignored runtime state. The full integrated results below supersede the initial-main full preflight. Browser
assertions use real authenticated Proxy requests and the bound thread UI.


## Full integrated evidence (2026-10-14 UTC)

Branch `agent/b-lab` fast-forwarded to `integrate/treer-b` at `87dc6ab4`, including
hub notification backoff `bf78c24d`. Two small attach fixes were committed
separately: `676b0e4a` makes startup cwd relative to the authenticated machine
root, and `bf2a8461` waits for the new Agent to appear in discovery. The tested
binary is built from `bf2a8461164e9d245949d044ce781dcf2ef67bce`; SHA-256
`3ef780554d57c93f07c626cae265334aa4a23a6f343590356525644a436ae3e3`.
Web assets were built from this worktree; the shared UI checkout matches
`thread-ui.sha` (`dbed5ff1a5dc7fab68a8161af6f4a8953f8f731b`). Treer remains at
`1a512715a935b49f262ad957ddf344bc5a8944eb`, with no local changes.

Exact preparation/build commands on this machine:

```bash
git merge integrate/treer-b
ln -s /home/ubuntu/dev/remoteCodex/pockymoe-thread-ui pockymoe-thread-ui
corepack pnpm install --offline --frozen-lockfile
corepack pnpm --filter @pockymoe/supervisor-web build
scripts/treer-lab/down.sh
scripts/treer-lab/up.sh

# After the focused attach fixes, only the lab device needed replacement:
scripts/treer-lab/up.sh --prepare-only
node scripts/treer-lab/lab.mjs refresh-device --skip-prepare
node scripts/treer-lab/e2e.mjs --full
```

The final full driver exited zero at `2026-10-14T01:39:21.825Z`:

| Driver check | Result |
| --- | --- |
| Proxy and authenticated lab admin/user | PASS |
| Docker host device enrolled and online | PASS |
| Plain terminal Agent accepts input and returns output | PASS |
| Managed Agent caller and Controller contract | PASS |
| Fake Pockymoe hub is a Treer Managed App created by a managed Agent | PASS |
| Two attach Agents register distinct interfaces on the hub port | PASS, all seven capabilities |
| CLI prompt produces replies, idle status and two transcript pages | PASS, exact fake replies, page/cursor offsets |
| Agent-to-Agent queued prompt sends a completion notification | PASS, explicit `--notify`, submission ID in Core Message |
| Chromium thread UI and cross-thread API gate | PASS, authenticated UI, anonymous rejection and cross-thread denial |
| Stop/start keeps the same Agent, bound thread and history | PASS, working/busy before stop, interrupted turn, harness released, same-thread/history rebind |
| Hub App restart reconnects existing attach processes | PASS, changed App PID, unchanged attach PID, retained transcript |

Playwright: `tunnel.spec.mjs`, `desktop-chromium`, **1 passed (1.9s)** in the full
run. The focused spec also asserts the `307` relative `Location:
threads/<bound id>` and final URL under the same Agent tunnel prefix; this strengthened spec passed
separately in **1.4s**. The attach
CLI crate tests passed: **20 unit + 4 attach lifecycle + 1 bind-status = 25**, via
`cargo test --locked -p pockymoe`. `cargo fmt --all -- --check`, the Pockymoe
build, Web build and script syntax checks passed.

Failures encountered and resolved:

| Failure | Owner and resolution |
| --- | --- |
| Attach connects but cannot save startup (`invalid_machine_path`, cwd must be relative to machine root) | Attach product bug; `676b0e4a`, focused path boundary test and lifecycle assertion |
| Fresh attach exits because its Agent is not yet in discovery | Attach startup race; `bf2a8461`, bounded discovery wait and transient-failure lifecycle fixture |
| Device offline immediately after warm container replacement | Lab preparation; restore the installed enrolled service and await online |
| Stop interrupts but `thread.isLoaded` stays true | Lab assertion; ordinary thread DTO does not describe current harness membership. Assert actual loaded-harness presence before stop and absence after, plus interrupted turn and cleared active turn |

The driver also corrected pre-run assumptions: notification needs `--notify`;
transcript page/cursor is a turn offset, with `--limit 1` preserving a stable first
page for history comparison. No product checks were weakened. No hub or Treer
source changes were necessary, and attach protocol types are unchanged.

The lab is left running with the final two attach Agents idle and registered on
port 18880; three leftover attempt Agents were stopped. The container binary
SHA-256 matches the tested build, and Proxy health passes. Scope is this Linux Docker device with authenticated
Treer, fake Codex runtime and one Chromium project. Real inference, other
providers/platforms/browsers, TLS deployment and explicit pending-queue pause
across detach are not covered by this full driver. Notification retry/backoff
failure paths remain covered by hub crate tests, rather than an injected CLI
failure in this lab run.
