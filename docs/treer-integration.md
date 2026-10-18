# Treer integration

Pockymoe uses one Supervisor **hub** per device and one thin `pockymoe attach`
process per top-level Treer Agent. The hub owns the threads, history, harnesses
and Web UI. Attach supplies its Treer identity over an authenticated loopback
WebSocket and registers `treer.agent-interface/v1` with the **hub's** HTTP port.
Each Agent gets a fresh interface instance ID on process start. The hub dispatches
Agent requests by Treer's `x-treer-agent-id` and `x-treer-interface-instance`
headers, rather than opening a separate listener for every thread.

Child threads stay inside their top-level thread's scope; they are not separate
Treer Agents. The hub is a Treer Managed App, so the Host owns its process and
relaunches it after an update. Attach remains alive across hub restarts and
rebinds the same thread.

## Setup

Open any installer or terminal Agent on the enrolled Treer machine and run:

```sh
curl -fsSL 'https://raw.githubusercontent.com/dufangshi/pockymoe/b4add8e7c34db490f868c9f8b7800e9256ffa51c/integrations/treer/treer-setup.sh' -o treer-setup.sh && sh treer-setup.sh --version 0.13.11
```

The URL is pinned to the 0.13.11 release commit `b4add8e7`; a newer script gets a new pinned URL. The public URL points to one
self-contained POSIX shell script; no other setup scripts are downloaded.
Its bundled Python sources and profile template are extracted into a private
temporary directory and removed when setup exits. Runtime and Web downloads use
the pinned release; the hub also downloads current managed ACP adapters when needed.

Setup needs Python 3.11 or later and supports Linux x64/ARM64 and Apple Silicon.
Treer must be on PATH and the Agent must have its normal `TREER_*` workload
environment. Setup waits for `treer whoami` discovery with backoff up to thirty
seconds between retries. It verifies the same explicit AIS v1 capability/feature
contract as attach, including `agent.interface-provenance.v1` and
`agent.prompt-delivery.v1`, before creating anything.
Install and authenticate the desired base harness separately; see
[device setup](device-setup.md) and [managed adapters](managed-acp-adapters.md).
The hub creates a thread only when that profile's base agent CLI (`codex`,
`claude` or `grok`) is on its PATH; otherwise Launch fails with "Install the base
agent first". Setup ends by naming any base agent it cannot find.
Treer and the hub must share the same OS account and loopback network namespace.

`--version` is required and accepts only an exact stable release, optionally
prefixed with `v`; `latest` and moving references are rejected. If
`~/.local/bin/pockymoe version` matches, setup reuses it. An older installed
version is upgraded through the running setup-created hub's management API,
as described below. A newer installed version is refused; setup never downgrades
or replaces an existing installation directly.

Setup creates the hub Managed App named `pockymoe` and the **Pockymoe Codex**,
**Pockymoe Claude**, and **Pockymoe Grok** launch profiles. In Treer, choose
**Launch**, select a profile, and set the working directory to your project.
The App and workspace profiles survive removal of the installer Agent.
Rerunning reuses the existing App and its port and updates profiles by stable ID;
it also completes partially successful runs without creating duplicates. Existing
duplicate names, a hub on another machine, or different hub settings stop setup
for inspection. The App's saved command is retained because Treer provides no
App update command.

After configuring the App and profiles, setup waits up to 60 seconds for hub
health on `127.0.0.1:<hub-port>`. It reads the local-mode hub's harness inventory
and checks the Codex and Claude ACP adapters whenever their base CLI is installed
in the hub. Missing, external, or unverified adapters are updated through the
hub's management API to the current Pockymoe-managed installation. Each update
has an eight-minute setup wait limit and must report a successful ACP connection
verification. Progress appears about every twenty seconds while waiting. A
managed adapter whose last adapter update was verified is skipped on reruns;
the hub's verification job record is in memory, so a hub restart requires a
fresh adapter update and verification. Missing base CLIs are noted and skipped.
Grok uses native ACP and needs no adapter installation.

Setup prints each checked adapter's version, selected path, and verification
result. Any adapter failure exits nonzero after preserving the App and profiles,
so a rerun can finish setup. Repair through **Pockymoe Settings → Harnesses → ACP
adapter → Install**, or rerun setup. If the setup wait limit expires, the hub job
may still be running; check its status in Settings before retrying.

Without `--hub-port`, a new hub uses the first free IPv4 loopback port from 8787
upward; an existing hub keeps its port. Pass `--hub-port N` for an explicit port.
`--data-dir` defaults to `~/.local/share/remote-codex/treer` and must match on
reruns. The App and profiles use stable executable and Web paths through
`~/.local/bin/pockymoe` and `native/current/web`, never `releases/<version>`.
`TREER_BIN` optionally selects the Treer executable. Setup prints progress and a
Launch summary, suppresses raw Treer responses, and exits nonzero on failure.

### Updating

Inside a Treer installer or terminal Agent, rerun the same setup script with the
desired exact release:

```sh
sh treer-setup.sh --version "$POCKYMOE_PINNED_VERSION"
```

For an older installation, setup requires the existing App named `pockymoe` on
this machine, with the setup-created stable launch command, data directory, and
host-managed restart configuration. The hub must be running and reachable on
its loopback port. If the App is stopped, start it in Treer and rerun; setup does
not install over it. A different App configuration, mismatched running/installed
versions, or an update already in progress stops setup for inspection. Keep the
same `--data-dir` and, if specified, `--hub-port` as the original setup.

The current Supervisor API updates to GitHub latest and accepts no target-version
argument. Setup calls **Check updates** first and refuses to request an update
unless `latestVersion` exactly equals `--version`. It then calls **Update** through
the loopback management API. The hub verifies and installs the release, switches
`current`, and exits; Treer relaunches it using the saved stable path. Setup polls
through restart disconnects for up to ten minutes, with progress about every
twenty seconds, until management reports the requested `runningVersion` and
startup verification has finished. It then converges profiles and checks ACP
adapters. A changed update target or reported failure stops setup; inspect
**Pockymoe Settings → Supervisor**. A timeout does not cancel the hub's update:
check the App and update status before rerunning. See [update recovery](supervisor-update-recovery.md).

Running attach Agents are not restarted by setup. They reconnect automatically
when the hub returns and continue using their existing attach process. They pick
up the updated attach binary on their next Treer restart. The hub handles pausing
and recovering affected coding turns through its normal update contract.

### Advanced: separate install and configuration

The editable sources live in [`integrations/treer/`](../integrations/treer/).
For a fresh installation, run the installer, then configure from inside a
Treer Agent (skip installation when the requested version is already installed):

```sh
python3 integrations/treer/install.py --version 0.13.2
python3 integrations/treer/configure.py --create-hub --hub-port 8787 --dry-run
python3 integrations/treer/configure.py --create-hub --hub-port 8787
```

`install.py` starts no service. It verifies the native executable and Web ZIP
against the release's `SHA256SUMS`, checks the executable's version, then activates
the normal `~/.local/share/remote-codex/native/releases/` layout with `current`,
`current.json`, and `~/.local/bin/pockymoe`. It refuses an active installation,
occupied CLI paths, or an existing version directory. An interrupted installation
can leave a retained release or lock; inspect those before retrying. Existing
devices use [Settings update recovery](supervisor-update-recovery.md).
Downloads and checksums are trusted through GitHub HTTPS; the checksum file is
not an independent signature or offline trust root.

`configure.py` uses the same idempotent configuration and contract checks.
`--dry-run` reads existing Apps/profiles and prints planned writes. Without
`--create-hub`, only profiles are configured; its default hub port is 8787.
`--cwd` sets a Treer machine-relative profile working directory, and
`--pockymoe` selects an absolute stable executable path. Launch can override cwd.

For development tests only, `POCKYMOE_RELEASE_BASE_URL` replaces the release
asset root. For example, a loopback mirror at `http://127.0.0.1:9000` serves
`/v0.13.2/SHA256SUMS`, `/v0.13.2/remote-codex-<platform>`, and
`/v0.13.2/remote-codex-web.zip`. HTTPS mirrors and loopback HTTP are accepted;
URL credentials, query strings and fragments are rejected. SHA256SUMS and runtime
version verification still apply. Do not set this override for normal installs.

After editing `setup.sh`, `install.py`, `configure.py`, or `profiles.json`, run
`python3 integrations/treer/bundle_setup.py` to rebuild `treer-setup.sh`.
`python3 integrations/treer/bundle_setup.py --check` and the packaging unit tests
fail if the committed script is stale. CI runs these checks for packaging changes.

[`profiles.json`](../integrations/treer/profiles.json) is the integration's own
template format, rendered by `configure.py` through Treer's CLI. It defines a hub
Managed App and Codex, Claude and Grok launch profiles. Grok uses
`--provider acp --agent grok`, matching the runtime's ACP catalog. The hub command
uses `/usr/bin/env` to retain these settings in Treer's saved command:

- `POCKYMOE_MODE=local`, `HOST=127.0.0.1`, and the selected `PORT`;
- `POCKYMOE_HOST_MODE=hosted`, `POCKYMOE_HOSTED_BY=treer`;
- `POCKYMOE_SUPERVISOR_RESTART_STRATEGY=host-managed`;
- an absolute `POCKYMOE_DATABASE_PATH` and `POCKYMOE_WEB_DIST_DIR` through `native/current/web`.

Keep launch commands on `~/.local/bin/pockymoe` or the native `current` path.
Never save `releases/<version>/...` in a profile or startup command. The hub
updates its current installation and exits for Treer to relaunch it; a saved
versioned path would launch the old version. The App's Web bundle must follow
`current/web` for the same reason.

## Hub App

The Managed App remains named `pockymoe`. It is the shared conversation hub for
all Pockymoe Agents on that machine. Opening its App UI shows a small read-only
page with the machine hostname, running version and number of durably bound
Agents (including disconnected Agents). Use a Pockymoe Agent to open
conversations. Stopping this App takes every Pockymoe Agent on the machine
offline; they reconnect when it starts again.

App UI tunnel requests carry `X-Treer-Identity-Token` without Agent interface
markers. Only loopback requests with that header can select this page. The
header grants no conversation access: `/api/status`, `/v1/status` and `/healthz`
return hub metadata; all other API paths, thread pages, WebSockets and writes
receive a read-only JSON denial. Responses never include thread titles, IDs or
credentials. The token is neither interpreted nor echoed because this surface
provides only non-privileged hub metadata. Agent interface requests continue to
require their normal provenance and live binding. Standalone hubs retain their
existing routes and reject Treer-marked requests.

While any Pockymoe attach Agent runs, it attempts to start an accidentally
stopped hub after at least thirty seconds of unavailability. It uses its own
Treer identity to inspect `treer agent show <self>` and `treer app list`, then
requests `treer agent start <app-id>` only when the matching App reports
`stopped` or `exited`. Lookups and start requests are limited to one attempt per
two minutes, including failures. Running, pending, unavailable or ambiguous
Apps are left alone; Policy denials and subprocess diagnostics are suppressed.
Attach never stops or restarts an App. With no running attach Agent, Treer's
normal stopped intent remains in effect.

Setup converges both new and existing launch profiles with `--hub-app pockymoe`.
Attach also accepts an explicit App ID and retains this option when saving its
`attach --thread` startup command. Older profiles without the flag discover the
setup-created `pockymoe` App by the loopback hub port. Recovery requires a unique
matching App on the attach Agent's machine, with the same hub port and saved
`POCKYMOE_HOST_MODE=hosted` / `POCKYMOE_HOSTED_BY=treer` settings. Existing running
attach processes gain recovery after their next normal Treer restart.

## Attach command

Treer launches attach as one Agent, with the project as its current directory:

```sh
pockymoe attach --provider codex --title "Project assistant" \
  --hub http://127.0.0.1:8787 --hub-token-file /path/to/data/treer-attach.token
```

For an existing thread, use `pockymoe attach --thread THREAD_ID` with the same
hub flags. `--thread` conflicts with creation options. Creation accepts
`--provider`, `--agent`, `--model`, `--reasoning-effort`, and `--title`; it sends
the absolute current working directory to the hub. Models are passed through
unchanged. `TREER_AGENT_ID` is required. The optional
`TREER_WORKLOAD_CREDENTIAL`, `TREER_AGENT_SERVER_URL`, `TREER_WORKSPACE_ID`, and
`TREER_SERVER_ID` values are forwarded in the hello without logging them.

The hub URL defaults to the ordinary CLI connection discovery (`--url` /
`POCKYMOE_URL`, then the local `cli.json` URL); if absent, attach uses the runtime's
local loopback port, normally 8787. The token file defaults to
`treer-attach.token` beside the discovered `cli.json` / configured Supervisor
database. An explicit `--cli-config` changes that directory. Attach uses this
separate file, never the normal thread CLI token. Use explicit hub flags when
Treer launches in a different environment or directory from the hub.

At startup, attach probes `treer interface contract` once. It requires an
explicit AIS v1 protocol, capability list, and the features
`agent.interface-provenance.v1` and `agent.prompt-delivery.v1`. Missing fields
mean unknown support and fail closed. The probe's AIS feature/capability lists
stay in memory; only their counts are printed. The hub's declared capabilities
must be supported by that Controller.

Attach sends `AttachHello` on `/api/treer/attach`, with the private token in the
WebSocket upgrade's Bearer header, and waits for `AttachReply`. It prints a waiting
line immediately and a reminder every thirty seconds; first-launch runtime and
ACP adapter setup can take several minutes. The socket stays open during setup
and answers pings. After creation,
it saves `treer agent startup set --cwd ... -- <stable pockymoe> attach --thread ...`
**before** registering the interface. The saved command includes the explicit
hub URL and token-file path, never token contents or Treer credentials. If only
a versioned executable is available, attach warns and refuses to save it.
The startup working directory is relative to the machine root reported by
`treer whoami`; attach rejects a working directory outside that root before
creating a thread. The hub still receives the absolute working directory.
A newly launched Agent may appear in the Controller's discovery snapshot only
after a delay; attach keeps retrying (backoff up to thirty seconds) instead of
exiting, because Treer does not restart an ordinary Agent that exits.
A pre-existing thread binding registers without rewriting startup.

Attach keeps the socket open, answers pings, and reconnects with exponential
backoff from one to thirty seconds. Reconnects send the bound thread ID and
register the interface again with the same process instance ID. Token contents
are reread on reconnect. A missing token file is retried while the hub starts.
A missing base agent CLI (`base_agent_missing`) or a provider not enabled on the
hub (`provider_unavailable`) keeps attach running: it prints what to install and
retries every 30 seconds, connecting on its own once the problem is fixed, because
an exited Agent would only show Treer's `process_not_running`. Any other rejected
binding, authentication failure, invalid configuration or unknown contract exits
nonzero. There is no successful exit path: Treer disables Agent
recovery after exit code 0. Short status lines remain available in Treer's PTY
fallback. Known setup failures identify a disabled provider, missing base agent,
missing thread, or a thread already attached elsewhere using fixed messages.
Unknown internal errors retain a generic binding refusal; raw runtime errors
and subprocess output are never forwarded.
Hub rejection diagnostics include the hub's error code and message, with known
credentials redacted, control characters stripped and code/message lengths capped
at 96/512 characters (plus a truncation marker). Treer subprocess output stays
suppressed, and attach disables tracing even if `RUST_LOG=trace` to protect the hello payload.

## Thread and Agent registry

In Treer hosted mode (`POCKYMOE_HOST_MODE=hosted` and
`POCKYMOE_HOSTED_BY=treer`), every visible durable thread, including a delegate,
owns one Treer Agent. Stable thread and Agent IDs bind for life; changing a name
never changes that binding. This registry supersedes the older top-level-only
binding and identity model described elsewhere in this guide. Standalone
Pockymoe installs no host hook and retains its existing behavior.

The runtime exposes one optional, generic thread lifecycle hook. The Supervisor
installs its Treer implementation once at startup. CLI delegates create Agents
with the parent thread's live identity; scoped browser creation uses the page's
Agent; forks use the source thread's Agent. Local pages, automations, imports,
and migration without a live caller use the hub App's own `TREER_*` identity.
A first-launch attach already owns an Agent and binds that identity directly.
Every bound thread receives only its own Agent credentials, including children.

Registration runs `treer agent admin create --machine SERVER --kind command
--name LABEL --cwd RELATIVE -- pockymoe attach --thread ID --hub URL
--hub-token-file PATH`, using a stable executable. Treer's default placement
inherits the creator's folder. Top-level labels sanitize the thread title;
children use `<parent Agent name>.<delegate name or sanitized title>`. Names
contain no thread UUID. Only `agent_name_conflict` retries with `-2`, `-3`, and
so on, up to 100 candidates. Cwd must remain within the reported machine root.
Creation waits up to 30 seconds for attach to bind. A CLI operation has a
15-second deadline. Registration failure deletes the new Agent and rolls back
the new thread; failed Agent cleanup persists for later reconciliation.

Closing a delegate stops its Agent and retains history and its reserved name.
A prompt to a closed thread starts the Agent and waits for rebind before any
harness delivery. Deletion removes the Agent before deleting thread history;
direct-child deletion keeps its ownership, descendant, active-work and queue
checks. Rename updates Treer first and cascades a matching parent name prefix
through the recorded descendants. Unrelated Agents with similar names are
unaffected. Rename failures attempt to restore already-renamed Agents.

Reconciliation runs at startup, on attach connection and disconnection, and
about every 60 seconds with the hub App identity. An Agent renamed in Treer
updates its thread title; children display the title without their parent's
prefix. A confirmed `agent_not_found` archives the thread. Archived threads are
hidden from lists, search, lineage views and Treer scope, while their rows and
history remain in SQLite. A stopped or failed Agent retains its binding and
history; it is not treated as deleted. CLI outages and policy failures never
archive a thread.

Unbound visible threads from older installations, including unbound delegates,
and interrupted registrations migrate through the same registration path.
A lost create response is recovered by checking the Agent's saved startup for
this hub's `attach --thread ID`, preserving the original stable Agent ID rather
than adopting an Agent by its name. Closed legacy delegates migrate and then
stop their new Agents. Archived and explicitly hidden internal threads never
migrate. Treer browser
scope includes only each thread's own binding or active registration; a root
binding no longer exposes an unbound child. Controller access is limited to its
own bound thread. Credentials and subprocess diagnostics are never persisted or
returned to callers.

## Lifecycle mapping

| Treer action | Pockymoe meaning |
| --- | --- |
| Agent stop | Attach is terminated; the socket closes. The hub interrupts the bound turn and releases its harness, retaining history. |
| Agent start / restore | The saved `attach --thread` command rebinds the same thread. Harness recovery is demand driven. |
| Agent restart | Stop followed by start with a fresh process instance ID. |
| Agent delete in Treer | Registry reconciliation confirms deletion, archives the thread and retains its database history. Explicit Pockymoe deletion removes the Agent first, then its thread history. |
| Hub update / restart | Attach waits, reconnects and re-registers. It does not create another thread. |

Attach cannot reliably send a final notification within Treer's short stop
grace period. The hub uses socket lifetime for disconnection, including crashes.
A hub App stop also closes all attach sockets; the Agents keep retrying until
the App starts again.

## Security model and limits

The attach token file is a private, mode-0600 local bootstrap secret. The hub
must enforce constant-time comparison and reject unknown Agent/thread/instance
bindings. The attach client accepts only loopback HTTP hub URLs without URL
credentials, query, fragment or a path. It does not transmit the Agent's
credentials to a remote URL or use the general Supervisor API token.

Treer credentials are held only in attach/hub process memory and the managed
workload environment. They are not written into startup arguments, profile
JSON, thread history, logs or integration state. The hub delivers each Agent's
credentials only to the harness of the thread bound to that Agent; a child gets
its own Agent's credentials when that Agent attaches. The token-file **path** is
saved; its contents are not.

Treer's injected source/Agent/instance markers are provenance, not secrets.
They support browser and routing checks but do not authenticate an OS process.
A process running as the same OS user can inspect environments, read private
files, or forge markers. This integration is not an isolation boundary against
that user. Unknown or disconnected requesting bindings fail closed.

A Treer machine belongs to one workspace. Treer currently authorizes browser
humans at workspace level: any workspace member can open every Agent's UI.
Consequently a live Agent UI tunnel may access every top-level thread on this
hub with a persisted Treer binding (live or detached), and all their descendants.
It exposes nothing a member cannot already open through Treer. Unbound hub
threads remain invisible, including in collections and WebSocket events.
`pockymoe_treer::thread_in_scope` is the single thread-policy decision, with a
marked hook to intersect future Treer `policy/check` results and per-node
permission bits. Controller AIS access remains limited to its own Agent family.
Management, global settings writes, devices, upstreams and harness administration stay
denied, except the hub's own Supervisor status, check, update and restart. An unrestricted App/workbench still requires verified human identity and
visibility filtering; local HTTP authentication alone does not supply that.

## Unsupported scope

- A shared hub across isolated containers or devices without shared loopback.
- Windows installation/profile wrappers in this Unix packaging; Windows needs
  a stable owner-controlled launcher that follows `native/current.json`.
- Deleting a thread from Treer alone: the registry archives the thread of a
  deleted Agent and keeps its history instead of deleting it.
- Cross-device split views and fine-grained human/App authorization.
- Release rollback, automated profile upgrades, or replacing a running
  Supervisor through the installer.

## Treer Apps

A bound thread uses workspace Apps as its own Agent, as Treer's own Agents do:
`curl -fsS http://NAME.treer/` reaches the App through the thread's workload
proxy, which adds a short-lived identity token for that Agent. The hosted guide
lists this with the Issues entry points (`/`, `/v1/issues?assignee=me&unresolved=1`
and `/v1/issues/N`).

The Treer Issues App (`apps/issues`) needs no changes. A thread can read the
directory, create and assign issues, comment, claim and close them, and every
activity entry names that thread's Agent. Issues' change notifications are sent
with the actor's token: a human actor's notice wakes the assignee, an Agent's
stays passive mail. Issues' reminders and digests come from the App's runtime
Agent; with the default `POCKYMOE_TREER_WAKE_APPS=issues` they wake an idle
assignee. Issues sends reminders only when `treer agent show` reports the Agent
`idle` and not busy, which the hub supplies through `state.observe`.

## Hub internals

The hub enables the integration only with `POCKYMOE_HOST_MODE=hosted` and
`POCKYMOE_HOSTED_BY=treer`; startup rejects other combinations. Hosted mode
defaults the update restart strategy to `host-managed`; an explicit
`POCKYMOE_SUPERVISOR_RESTART_STRATEGY` overrides it. Pure logic lives in
`crates/treer`; routes and wiring in `crates/supervisor/src/treer.rs`. The
attach wire types are in [`crates/protocol/src/treer.rs`](../crates/protocol/src/treer.rs).

**Attach endpoint.** `GET /api/treer/attach` accepts a loopback WebSocket with
`Authorization: Bearer` set to the hex 256-bit token in `treer-attach.token`
beside the hub database (Unix mode 0600; a symlink, non-file or wider mode is
refused). Requests with `Origin` or `Sec-Fetch-*` are refused. One live Agent may
bind each thread. A repeated `create`
from a known Agent rebinds its saved thread instead of creating another.
Bindings are stored in their own table; core thread tables are unchanged.

**Disconnect.** When the attach socket closes (Treer stop, crash or restart) the
instance's access ends at once. The hub pauses that thread's queue, interrupts
the running turn and releases its harness, keeping the thread, its history and
queued inputs. Persisted bindings wait for the next attach after a hub restart.

**Credentials.** The Treer identity from the hello is installed only into the
bound thread's harness and ACP terminal environment. It is held in memory, never
persisted or logged, and never inherited by another thread. Harnesses, terminals
and the fake-runtime fixture never inherit the hub's own `TREER_*` identity or
its workload proxy (`HTTP_PROXY` and related variables equal to the hub's
`TREER_HTTP_PROXY`); any other configured proxy is kept. Attach also sends its
own `TREER_HTTP_PROXY` (`httpProxy`, accepted only as `http://127.0.0.1:PORT`),
and the bound thread uses it as `http_proxy`, `https_proxy`, `HTTP_PROXY`,
`HTTPS_PROXY` and `TREER_HTTP_PROXY`, with loopback in `no_proxy`. Treer signs
`*.treer` requests through that proxy as the thread's Agent, so an unbound
thread reaches no App with the hub's identity.

**Agent Interface (`treer.agent-interface/v1`).** The hub advertises
`prompt.submit`, `prompt.queue`, `prompt.queue.read`, `prompt.notify`,
`transcript.read`, `state.observe` and `abort`. Controller requests must come
over loopback TCP with Host `agent-interface`, source `controller-api`, the live
Agent and instance IDs, and no browser headers. `operation_id` maps to a durable
pending-request ID: a retry with the same input returns the original receipt, a
different input conflicts. Receipts and completion notifications survive hub
restarts. Submission lookup and queue cancellation decode percent-encoded path
IDs sent by the Controller. A completion notification is sent once per submission
with the bound Agent's live identity through the `treer` CLI on the hub's `PATH`, using the
idempotency key `prompt-submission/<id>/terminal`; it retries until delivered and
never reruns the task. Transcript `page`/`cursor` are turn offsets (default limit
1, maximum 1000); items map through the typed thread schema. Status is `blocked`
while the thread has a pending approval or question.

**Browser UI tunnel.** A `proxy-ui` request needs a live binding and the hub's
loopback Host. `/` redirects relatively to the requesting Agent's bound thread.
Collections list only bound top-level threads and descendants; detached bindings
remain visible. The workbench supports tabs, split views, favorites, thread
settings, rename, delete and new conversations across that set. Files are
read-only. Fork/import, sharing, terminal, device/global settings and harness
administration controls are hidden. Denied requests with
`thread_scope_forbidden` have a clear fallback message.

Settings stay available with three tabs. **Session** holds the current thread's
own settings, the same ones its composer opens. **Preferences** keeps what is
stored in this browser: language, theme and theme packs, color mode, text size,
phone dock and hand, timeline folding and status summaries. It omits model
pricing, the message-shortcut setting (saved to a relay account) and the tour.
**Device** holds only hub maintenance: the running and latest version, **Check
updates**, **Update** and **Restart Supervisor**, without the local passcode or
device templates. These use the same management API as Treer setup
(`GET /api/management/supervisor`, `POST …/check|update|restart`). Updates go
to GitHub latest only; the hub then exits and Treer relaunches it, and the page
waits for it to return. Any user who can open a Pockymoe Agent here can already
prompt an agent on this machine, which can run the same setup command. The other
tabs administer the whole device and are not offered. The tunnel serves the
built-in theme packs under `/themes/` read-only.

The thread map stays available with this hub as its one root. Its workspace and
thread reads are the scoped collections above, and `GET /api/thread-activity`
keeps only threads the tunnel scope allows, so sizes and hourly activity never
include other threads on the machine.

Inside a Treer frame the workbench rail (or the phone top bar) has **Open in a new
browser tab**, which opens the same page full size. It needs a Treer whose
frame sandbox allows popups (`allow-popups allow-popups-to-escape-sandbox`); in
an older frame the button explains Treer's own tab menu entry instead. A
top-level page shows no button.

Scoped runtime config responses include camelCase `hostScope` with kind
`treerAgent`, `boundThreadId`, stable non-secret `hubId` and allowed operations;
ordinary requests omit it. Browser navigation, favorites, presentation and seen
markers use hub + tunnel mount keys, without importing old shared-origin
shortcuts. Definitive scope-denied/404 references are silently removed; transient
or offline failures do not discard shortcuts.

**Browser thread/Agent synchronization.** Creating a top-level thread also runs
`treer agent admin create --machine <machine> --kind command --name <label>
--cwd <relative-cwd> -- <stable-pockymoe> attach --thread <id> --hub <loopback-url>
--hub-token-file <path>` using the requesting Agent's live in-memory identity.
`treer whoami` supplies the machine root; the workspace cwd must be within it.
Omitting `--folder` inherits the requesting Agent's Treer tree folder. Labels
come from the title with invalid characters removed and a thread-ID suffix for
sibling uniqueness. A failed Agent creation rolls back the new thread. The new
thread pauses until its own Agent attaches; it never receives the caller's Treer
credentials. The Web opens the result in the same page.

Renaming a bound top-level thread runs `treer agent admin rename` before changing
its title. Deleting one runs `treer agent admin delete` before deleting its thread
and descendants; failed Treer operations preserve the thread. Both use the
requesting Agent's live identity, including when targeting another bound Agent.
Deleting the requesting Agent's own thread is allowed after normal confirmation
and may close the page. Child thread changes do not create/delete Treer Agents.
Deleting an Agent from Treer itself currently retains its thread because the hub
cannot distinguish deletion from stop. Provenance headers remain browser defenses,
not authentication against a hostile process of the same OS user.

## Messages

In Treer-hosted mode (`POCKYMOE_HOST_MODE=hosted` together with
`POCKYMOE_HOSTED_BY=treer`), the hub installs one generic runtime messaging hook.
Without it, the existing internal send, queue, steer and inbox paths retain
their original behavior. Controller submissions already accepted through AIS
use the local path directly, avoiding a transport loop.

A thread sends with **its own** live Agent credentials. Both sender and recipient
must have stable Agent bindings; an unbound thread is refused. Passive inbox
mail runs `treer message send --to <agent-id>` with the body on stdin and an
idempotency key derived from the durable Pockymoe message ID. The body starts
with a small, explicitly untrusted kind/subject display header. Durable mappings
preserve Pockymoe/Core message IDs and translate `--in-reply-to` to Core context
IDs. Queue and steer run `treer agent prompt`, and direct requests choose steer
for running recipients or queue otherwise. Receipts remain `queued`, `steered`
or `held`. Prompting a closed recipient starts its Agent first.

Request IDs deduplicate sends locally. A mail retry reuses its Core idempotency
key, including after a restart. Treer's prompt CLI has no caller-supplied
idempotency flag: the hub commits a dispatch marker before invoking it. If the
process fails or the hub restarts before recording a confirmed receipt, retries
return `held` and never dispatch that prompt again. Inspect the recipient queue
and history to resolve this uncertain outcome.

Every live binding has an Agent-scoped receive worker using
`treer message receive --wait 30000 --limit 50`. One transaction stores each
fresh message in the existing inbox, its Core ID mapping, any eligible queued
wake, and its pending ack operation. Only after that commit does the worker call
`treer message ack`. Pending acks survive restarts, retry with the same operation
ID, and disappear only after a confirmed acknowledgement. Duplicate Core IDs
never create another inbox record or turn. Detaching cancels the subprocess;
reattaching resumes pending acknowledgements before receiving more mail.

The inbox shows attested Treer sender kind/ID/path and Core context IDs, and maps
known senders back to local thread IDs. Human senders or messages with an
external-source envelope also queue a prompt with quoted third-party data,
message IDs and a `treer message get <id>` hint. This decision uses only the Core
envelope, never body text, labels or completion-looking JSON. Automatic prompts
allow three wakes per sender kind/ID and recipient per minute; additional mail
stays unread in the inbox. Mail whose attested sender is an Agent equal to the
runtime Agent of an allowed Managed App also wakes. `POCKYMOE_TREER_WAKE_APPS`
lists the App names (comma-separated, default `issues`; empty disables). The hub
resolves the names with `treer app list` as the hub App, at most every 10 seconds
after mail from an unknown Agent and otherwise each minute; a failed lookup keeps
the previous answer. That wake prompt names the App and its `http://NAME.treer/`
manual. Other Agent, machine and service mail stays passive, and the existing
unread reminder applies. Replies map their context IDs to
`inReplyTo` and satisfy matching existing inbox waits without starting a turn.

`--notify-on-complete` maps to Treer's opt-in `--notify`. A completion is correlated
only when its attested Agent sender and submission ID match an outbound request
that opted in. It is stored as passive `result` mail with `inReplyTo` pointing to
that request. Completion-looking text from another Agent cannot claim the
subscription and remains ordinary passive mail.

Same-hub `--attach` snapshots files (and archives directories) using the existing
file-copy helper. Retries reuse those copies, and the message names their local
paths. Across machines, attachments are refused with a `treer machine put` hint:
Core Messages are text-only. Transfer files first and mail the destination paths.

The hosted `pockymoe skill` and managed-thread topic guides append the Treer
addendum: create children with `pockymoe thread create` to record lineage and use
Treer policy; `treer agent admin create` and profile launches are unrelated
top-level creation. Standalone skill/guide text is unchanged. The bridge requires
the thread/Agent registry to supply a live identity for every child; until that
registry is installed, unbound children fail closed.
