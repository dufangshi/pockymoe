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

The full driver requires the parent-provided integrated H/A commit. Initial main
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

Only one browser project and one spec are selected:

```bash
node /home/ubuntu/dev/remoteCodex/reference/treer/web/node_modules/@playwright/test/cli.js \
  test --config scripts/treer-lab/playwright.config.mjs \
  --project=desktop-chromium --list
# The --full driver runs the same command without --list after preparing fixtures.
```

Install that Playwright version's Chromium if it is unavailable. Traces, video,
storage-state files and credential-bearing failure output are disabled.

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
`full-evidence.json`. JSON stays in ignored runtime state. The full driver and
browser assertions must be validated against the final integrated contracts;
capability/notification, UI-prefix and reconnect behavior remain open until then.
