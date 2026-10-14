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

Use the packaging in [`integrations/treer/`](../integrations/treer/). It is an
external integration; no Pockymoe sources or Node supervisor are installed into
Treer. The installer needs Python 3.11 or later and supports Linux x64/ARM64 and
Apple Silicon. Install and authenticate the desired harness and ACP adapter
separately; see [device setup](device-setup.md) and [managed adapters](managed-acp-adapters.md).
Treer and the hub must share the same OS account and loopback network namespace.

Select an exact, trusted GitHub runtime release containing the attach command:

```sh
python3 integrations/treer/install.py --version "$POCKYMOE_PINNED_VERSION"
```

`--version` is required and accepts only a stable version, optionally prefixed
with `v`; `latest` and moving references are rejected. The installer downloads
the native executable and Web ZIP from that version's GitHub Release, verifies
both against its `SHA256SUMS`, and checks the executable's version before
activation. It uses the normal `~/.local/share/remote-codex/native/releases/`
layout, `current`, `current.json`, and `~/.local/bin/pockymoe`. Downloads and
checksum metadata are trusted through GitHub HTTPS; the checksum file is not
an independent signature or offline trust root.

The installer starts no service. It refuses an existing active native
installation or occupied CLI paths, and never overwrites a version directory.
An interrupted installation can leave a retained release or installation lock;
inspect those before removing them and retrying. Existing devices use Settings
→ Supervisor → Check updates / Update, as described in
[update recovery](supervisor-update-recovery.md).

Create the three attach profiles and the hub App on the enrolled Treer machine:

```sh
python3 integrations/treer/configure.py --create-hub --hub-port 8787 --dry-run
python3 integrations/treer/configure.py --create-hub --hub-port 8787
```

Run this once; each invocation creates new profiles. Without `--create-hub`,
only profiles are created. `--cwd` is the Treer machine-relative profile working
directory; override it when launching an Agent for a particular project.
`--data-dir` defaults to `~/.local/share/remote-codex/treer` and must match for
hub and attach. `--pockymoe` selects an absolute stable executable path.
`TREER_BIN` selects the Treer executable; otherwise both tools use `treer` on PATH.
If configuration fails after some creations succeed, inspect the Apps/profiles
and remove duplicates before retrying.

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
A rejected binding, authentication failure, invalid configuration or unknown
contract exits nonzero. There is no successful exit path: Treer disables Agent
recovery after exit code 0. Short status lines remain available in Treer's PTY
fallback. Hub rejection diagnostics include the hub's error code and message, with known
credentials redacted, control characters stripped and code/message lengths capped
at 96/512 characters (plus a truncation marker). Treer subprocess output stays
suppressed, and attach disables tracing even if `RUST_LOG=trace` to protect the hello payload.

## Lifecycle mapping

| Treer action | Pockymoe meaning |
| --- | --- |
| Agent stop | Attach is terminated; the socket closes. The hub interrupts the bound turn and releases its harness, retaining history. |
| Agent start / restore | The saved `attach --thread` command rebinds the same thread. Harness recovery is demand driven. |
| Agent restart | Stop followed by start with a fresh process instance ID. |
| Agent delete in Treer | Identity/credentials are revoked. Thread history is retained because attach cannot distinguish deletion from stop. Deleting a thread in the scoped Web instead deletes its Agent first, then its history. |
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
denied. An unrestricted App/workbench still requires verified human identity and
visibility filtering; local HTTP authentication alone does not supply that.

## Unsupported scope

- A shared hub across isolated containers or devices without shared loopback.
- Windows installation/profile wrappers in this Unix packaging; Windows needs
  a stable owner-controlled launcher that follows `native/current.json`.
- Treer-side Agent-delete reconciliation: deletion is indistinguishable from
  stop/disconnection today, so the hub retains the thread and its history.
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
loopback Host. `/` redirects relatively to the requesting Agent's bound thread.
Collections list only bound top-level threads and descendants; detached bindings
remain visible. The workbench supports tabs, split views, favorites, thread
settings, rename, delete and new conversations across that set. Files are
read-only. Fork/import, sharing, terminal, device/global settings and harness
administration controls are hidden. Denied requests with
`thread_scope_forbidden` have a clear fallback message.

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
