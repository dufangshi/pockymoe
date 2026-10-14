# Treer integration

Pockymoe uses one Supervisor **hub** per device and one thin `pockymoe attach`
process per top-level Treer Agent. The hub owns the threads, history, harnesses
and Web UI. Attach supplies its Treer identity over an authenticated loopback
WebSocket and registers `treer.agent-interface/v1` with the **hub's** HTTP port.
Each Agent gets a fresh interface instance ID on process start. The hub dispatches
Agent requests by Treer's `x-treer-agent-id` and `x-treer-interface-instance`
headers, rather than opening a separate listener for every thread.

Child threads stay inside their parent Agent's scope; they are not separate
Treer Agents. The hub is a Treer Managed App, so the Host owns its process and
relaunches it after an update. Attach remains alive across hub restarts and
rebinds the same thread.

## Setup

Open any installer or terminal Agent on the enrolled Treer machine and run:

```sh
curl -fsSL 'https://raw.githubusercontent.com/dufangshi/pockymoe/<PINNED_COMMIT_AFTER_MERGE>/integrations/treer/treer-setup.sh' -o treer-setup.sh && sh treer-setup.sh --version 0.13.2
```

**Pinned URL placeholder:** replace `<PINNED_COMMIT_AFTER_MERGE>` with the final
merged commit SHA before sharing this command. The public URL points to one
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
`~/.local/bin/pockymoe version` matches, setup reuses it. A different installed
version stops setup: update through **Pockymoe Settings → Supervisor → Update**,
then rerun. Setup never replaces a running installation.

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
WebSocket upgrade's Bearer header, and waits for `AttachReply`. After creation,
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
A rejected binding, authentication failure, invalid configuration or unknown
contract exits nonzero. There is no successful exit path: Treer disables Agent
recovery after exit code 0. Short status lines remain available in Treer's PTY
fallback. Raw hub error messages and Treer subprocess output are suppressed,
and attach disables tracing even if `RUST_LOG=trace` to protect the hello payload.

## Lifecycle mapping

| Treer action | Pockymoe meaning |
| --- | --- |
| Agent stop | Attach is terminated; the socket closes. The hub interrupts the bound turn and releases its harness, retaining history. |
| Agent start / restore | The saved `attach --thread` command rebinds the same thread. Harness recovery is demand driven. |
| Agent restart | Stop followed by start with a fresh process instance ID. |
| Agent delete | Identity/credentials are revoked in Treer. Thread history is retained; deletion reconciliation is not part of attach. |
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
JSON, thread history, logs or integration state. The hub may deliver them only
to the bound top-level thread's harness, never automatically to child threads.
The token-file **path** is saved; its contents are not.

Treer's injected source/Agent/instance markers are provenance, not secrets.
They support browser and routing checks but do not authenticate an OS process.
A process running as the same OS user can inspect environments, read private
files, or forge markers. This integration is not an isolation boundary against
that user. Agent UI requests must remain confined to their bound thread and
its children; user-supplied thread IDs must never widen that scope. Unknown
bindings fail closed.

Treer's Agent UI entry does not supply a human identity with per-Agent policy
checks. A full workspace App UI therefore needs separate verified human
identity and visibility filtering. Until that exists, an App route must not
expose the unrestricted workbench. This limit is independent of hosted mode:
local HTTP authentication alone does not provide Agent scoping.

## Unsupported scope

- A shared hub across isolated containers or devices without shared loopback.
- Windows installation/profile wrappers in this Unix packaging; Windows needs
  a stable owner-controlled launcher that follows `native/current.json`.
- Creating Treer Agents automatically from new hub UI threads, and Agent-delete
  reconciliation/archive policy.
- Core Message bridging, child Agent registration, cross-device split views,
  and fine-grained human/App authorization.
- Release rollback, automated profile upgrades, or replacing a running
  Supervisor through the installer.

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
bind each top-level thread; child threads cannot be bound. A repeated `create`
from a known Agent rebinds its saved thread instead of creating another.
Bindings are stored in their own table; core thread tables are unchanged.

**Disconnect.** When the attach socket closes (Treer stop, crash or restart) the
instance's access ends at once. The hub pauses that thread's queue, interrupts
the running turn and releases its harness, keeping the thread, its history and
queued inputs. Persisted bindings wait for the next attach after a hub restart.

**Credentials.** The Treer identity from the hello is installed only into the
bound top-level thread's harness and ACP terminal environment. It is held in
memory, never persisted or logged, and never inherited by child threads.

**Agent Interface (`treer.agent-interface/v1`).** The hub advertises
`prompt.submit`, `prompt.queue`, `prompt.queue.read`, `prompt.notify`,
`transcript.read`, `state.observe` and `abort`. Controller requests must come
over loopback TCP with Host `agent-interface`, source `controller-api`, the live
Agent and instance IDs, and no browser headers. `operation_id` maps to a durable
pending-request ID: a retry with the same input returns the original receipt, a
different input conflicts. Receipts and completion notifications survive hub
restarts. A completion notification is sent once per submission with the bound
Agent's live identity through the `treer` CLI on the hub's `PATH`, using the
idempotency key `prompt-submission/<id>/terminal`; it retries until delivered and
never reruns the task. Transcript `page`/`cursor` are turn offsets (default limit
1, maximum 1000); items map through the typed thread schema. Status is `blocked`
while the thread has a pending approval or question.

**Browser UI tunnel.** A `proxy-ui` request needs a live binding and the hub's
loopback Host. `/` redirects relatively to the bound thread. Access covers the
bound thread, its descendants, and the minimum global reads the page needs
(plugins, runtime config, models, capabilities, assets). Collections are
filtered to permitted threads and WebSocket events to those threads. Management,
device metrics, subscription usage and other threads are denied. These provenance
headers are browser defenses supplied by Treer, not authentication against a
hostile process of the same OS user.
