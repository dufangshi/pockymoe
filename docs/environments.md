# Dokploy execution environments

An execution environment runs the Pockymoe worker in a Linux Docker container
managed by Dokploy. The host Supervisor keeps thread identity, approvals,
database, API-broker credentials and the execution gateway. Optional owner-approved
native/file credential binds are described below. The environment is not a
registered device. Lifecycle operations use the Dokploy REST API, validated
against Dokploy v0.30.6. The trusted control plane uses Docker CLI for storage
proofs, inspection, verified stopping of fenced writers and worker exec stdio.
Agent work uses the Supervisor worker transport; the worker has no Docker access.
An unavailable environment never requests host execution.

## Connections and storage

A connection selects an existing Dokploy environment, an explicitly versioned
Linux image with compatible glibc, Git and the required harness dependencies, a
local Unix Docker socket and explicit `allowedRoots`. Remote storage without a
replication adapter is rejected. Rootless Docker and daemon UID remapping need
a storage ownership adapter and are rejected. The Docker socket and Dokploy key are never
mounted or passed to the worker. Connection responses expose `configured`, not
the key. Omitting `apiKey` when updating preserves the saved key. Changing
storage, image or limits requires destroying the connection's retained
environments first; the name and key can be updated independently.

The [harness image recipe](../containers/environment/README.md) supplies a
pinned Ubuntu 24.04 x86_64 image with Git, gh, Python, Node and native Codex/Claude
ACP dependencies. Build and validate it on a dedicated test host before use.

The Supervisor derives project paths from the trusted thread record. Creating
an environment records its owner but does not bind a thread. The HTTP owner
must validate idle/native-session transitions before calling `bind_thread`.
Multiple authorized child threads can inherit the same environment through
`inherit_thread`; they share its mounted storage and limits.

Only the selected checkout is mounted. A linked worktree additionally mounts
the necessary private gitdir, common gitdir and recursive local object
alternates. Each path is canonicalized and must be under an allowed root;
symlink and metadata pointers cannot extend that grant. Sources are revalidated
at every start: a changed canonical checkout path requires recreating the
environment. Project/Git grants cannot include the private control-plane
sidecar, the Supervisor home itself or its authentication/configuration roots.
Mounting a common
gitdir grants its shared Git metadata, including shared references, but does
not mount sibling checkouts. Submodule working copies and HTTP alternates
require an additional storage adapter and are rejected.

The Supervisor also creates one empty managed-worktree root next to the
workspace: `<workspace-name>.worktrees/<environment-owner-thread-id>`. Only
that exact directory is mounted, not its parent or sibling thread directories.
Inherited threads use that same environment root. Git creation happens through
the worker, not during control-plane directory preparation.

Docker bind sources belong to the daemon's namespace. When the Supervisor
itself runs in a container, the provider resolves the most specific inspected
bind or named-volume mount to its daemon source. Paths in the Supervisor image
layer are rejected. Random file challenges prove that each directory source is
the same storage the Supervisor sees; the current executable is checked by
SHA-256 before its read-only worker bind. The probe itself uses the non-root
checkout UID:GID, so private directory permissions are exercised without
adding capabilities. Same-spelling empty directories on a
different host do not pass this check. The CLI probe cannot represent source
paths containing commas and rejects them.

## Isolation and limits

The worker runs as the checkout's non-root UID:GID, with an init/reaper,
private PID/IPC namespaces, a read-only root filesystem, all capabilities
dropped and `no-new-privileges`. The container has no host devices, published
ports, Docker socket, host home or Supervisor database/configuration. Writable
project mounts retain their original absolute paths. Private state is mounted
at `/pockymoe/state`; `HOME` and `CODEX_HOME` refer to that state.

Every generation has a separate private `/pockymoe/bridge` directory for the
Supervisor's restricted Unix gateway sockets. The worker mounts this directory
read-only, so it can connect but cannot replace socket paths. No long-lived
credential is placed there. Network mode stays `none`. `allowedEgressHosts` is a
default-empty list of exact DNS names consumed by the Supervisor's separately
authorized egress broker; saving that list does not enable Docker networking.

The generated Compose file uses long bind syntax with `create_host_path:
false`, and escapes Compose variable interpolation in paths. Dokploy's
`isolatedDeployment` is disabled and the worker's `serviceNetworks` explicitly
detaches the Dokploy network, so Dokploy does not inject a conflicting network.

CPU, memory, swap, PIDs and open-file limits are mandatory. Running-count, CPU
and memory budgets apply across owned active or uncertain resources, in
addition to configured host CPU/memory reserves. Limits are inspected on the
actual container, alongside mounts, ownership labels and security settings,
before the state becomes `running`. `observedLimits` records that result.
Ordinary bind storage has no disk hard quota; `diskQuotaSupported` is `false`.

## Lifecycle and recovery

`EnvironmentManager::new(database_path)` owns the private sidecar
`<database_path>.environments`. Directories use mode `0700`, records use `0600`,
and a process lock prevents a second manager from overwriting the same store.
Resource records and connection credentials are replaced atomically. A failed
persistence operation disables that manager's execution authority until
Supervisor recovery. Public
errors omit raw Dokploy response bodies and redact the configured key.

Creating an environment produces a stopped resource record. Start verifies
storage and budgets, creates or recovers the Dokploy Compose resource by its
exact deterministic name, deploys it, and verifies the actual container. Dokploy
appends a suffix to `appName`; its returned value is pinned in the private record
and used unchanged for lifecycle commands. Recovery queries `environment.one`
for the exact name, rejects ambiguous/incomplete listings, and verifies the
resource's full name, environment, source type and server using `compose.one`.
An uncertain create without a visible matching resource blocks another create.
Stop fences the old generation, clears queued deployments and waits for active deployments before
running a fixed `compose -p <owned-app> -f docker-compose.yml stop` command
through the Dokploy deployment runner. It checks that no owned container is
running. Rebuild stops the old generation and deploys a new worker using the
same project and state paths. Destroy first runs a fixed Compose
`down --remove-orphans` command and confirms container removal before calling
`compose.delete` with `deleteVolumes: false`. It retains project data, state, bridge
directories and the destroyed resource record. Destroying a resource does not
implicitly remove thread bindings; a bound destroyed resource remains
unavailable until the owner explicitly changes the binding.

A Supervisor restart changes previously active states to `interrupted` and
advances generation. Before clearing the public execution target, the same
atomic write retains the old full container ID, generation and daemon identity
in a private pending-quiescence record. Startup/background reconciliation and
unsafe running-container observations stop this exact old writer through the
trusted local Docker driver. The driver checks the daemon, full ID and immutable
environment/generation labels before stopping, then inspects again to confirm
the container is stopped or absent. This operation retains the Dokploy record,
data, capacity reservation and original interrupted reason. Failure leaves the
target durable for retry and blocks Start/Rebuild until quiescence is confirmed.
Before requesting deployment, a separate durable marker records that its
outcome is unconfirmed. If the Supervisor exits before persisting the full
container ID, reconciliation verifies the pinned Dokploy resource, clears its
queued jobs and runs the owned Stop path. It confirms actual stopped containers
and daemon identity before clearing the marker, retaining the interrupted
reason and budget. An empty Docker listing alone cannot confirm this window:
a queued deployment may still create a writer later.
Legacy interrupted records that already lost their full container ID require an
explicit Dokploy-owned Stop; they cannot infer or adopt a writer by name/label.
The Supervisor does not resume a harness or replay commands. An explicit start
or rebuild can deploy a new generation after confirmed quiescence and cleanup.
Errors retain uncertain resource reservations until stop/destroy confirms the
actual outcome. Dokploy deletion can succeed before its asynchronous cleanup;
remaining containers are reported as a failure rather than a successful
destroy. External administrators can still change Docker resources after an
inspection; the execution transport must validate generation and container
identity on use.

`reconcile(id)` and `reconcile_all()` inspect running resources without resuming
work. Exit, OOM, missing or replaced containers, weakened limits or isolation,
and an unreachable Docker daemon change the resource to `interrupted`, advance
generation and clear the execution target and observed limits. An already
interrupted resource stays fenced until an explicit lifecycle action. The
integration calls these methods before reporting live state or authorizing
execution; the synchronous getters return the last observed state.

The integration owns `Arc<EnvironmentManager>` on the Supervisor, registers the
environment HTTP routes, enforces thread authorization and idle/native-session
changes, and revokes worker capabilities before lifecycle transitions. The
provider does not register HTTP routes or start the worker gateway itself.

## Thread entry and execution compatibility

The default workflow is agent-first. `pockymoe environment connections` discovers
non-secret connection policy for the authenticated thread. `environment request
--spec FILE --request-id ID` submits metadata and returns immediately; `requests`,
`show ID` and `cancel ID` inspect/revoke its receipt. The spec includes resources,
exact HTTPS hosts, optional credential source/target/RO-RW/config pointers and an
untrusted handoff. Thread, workspace, selected worktree, harness, model and effort
come from Supervisor records. There is no thread override or agent approve CLI.
Unknown/secret-value fields are rejected. Proposal validation reads filesystem
metadata, not credential contents, and does not create a deployed environment.

The owner reviews the prepared proposal in the thread's Isolation window or
Settings → Environments and approves/rejects its immutable SHA-256 digest. The
review states in words where the thread will run, its paths, resources, model,
per-harness authentication (API broker or native file), every credential
mapping with its source → target, file/directory and RO/RW, exact HTTPS hosts,
the migration and the agent's handoff; the full digest and IDs are under
**Technical details**. While a request awaits approval, is migrating or has
failed, the thread's tools entry reads "Isolated environments · approval
needed / migrating / migration failed"; it is read when the thread's status or
environment changes, not polled. It
covers normalized paths, their original filesystem identities and connection
policy. A changed spec requires a new request. Tool/yolo approval, gateway/CLI
capabilities and sharing grants cannot approve. A valid owner-signed browser or
the administrator session configured at **server startup** is required. Unsigned
loopback and a self-bootstrapped local passcode are not owner proof. The same
check protects legacy direct connection/lifecycle/binding and isolated-thread
creation controls; they cannot bypass this ceremony from a Host agent.

Requests persist as `pending`, `waitingIdle`, `provisioning`, `ready`, `failed`,
`rejected` or `cancelled` in the private sidecar. Approval is idempotent. It waits
for the current turn and admitted tasks; it never holds the proposing command
open waiting for its own admission. After an exclusive transition revalidates
settings/worktree and closed descendants, it persists a Host fence, ends the old
harness and background descendants, closes PTYs and revokes CLI/previews. It then
clears `providerSessionId`, provisions/binds Dokploy and initializes a fresh native
session. The original thread ID, journal, queue, approvals, model and effort stay
on the original Supervisor. The new session receives bounded public journal
text, original/latest owner instructions and the agent's untrusted handoff. This
is a text projection, not a new LLM summary or copied native state. A deterministic
continuation receipt prevents replay after a crash, including after consumption.

Host Linux ACP, callback commands, controlled commands and PTYs use a local
subreaper guardian with a private metadata-only exit receipt. Transport EOF on
Supervisor death ends/reaps its native/tool descendants before recording success.
Migration accepts that durable proof, never a missing PID alone. Startup recovering
turns without a receipt from a legacy binary cannot migrate automatically. A
killed/uncertain guardian blocks migration for explicit process recovery; unfinished
commands are marked interrupted and never replayed. This guardian is not an OS
isolation environment or a second registered device.

Failures retain the execution fence and actionable state. The owner can retry
the same grant after stopping/rebuilding retained computation, or explicitly
**Restore Host execution**: end the old process domain, destroy this request's
computation, retain project/state and clear its binding/fence. It does not replay
the old command. Connection/source changes can be recovered this way before
submitting a new proposal. Completed migrations offer the same explicit owner
recovery. The old binding selector cannot remove an approved isolation fence;
use this action to destroy its retained computation before restoring Host.
There is no automatic Host fallback. Approved waiting
requests can be cancelled before the fence. Execution settings, worktree and
manual binding changes are blocked during approved migration.

Owner direct management remains available. A new thread can select
`environmentConnectionId` on `POST /api/threads/start` (also the workspace route)
to create/bind a stopped resource; start it before prompting. Existing direct
switches explicitly confirm `resetSession: true`. Inherited child bindings cannot
switch to Host.

Binding changes are mutually exclusive with admitted commands, hooks and HTTP
file request bodies. A busy request returns HTTP 409; retry after the operation
finishes. Before acknowledging a successful switch, the Supervisor ends old
PTYs and revokes the old host CLI identity. Close every open descendant before
switching a parent. Historical children without the same environment as their
ancestor cannot resume or probe the host; create a new child from the isolated
parent instead.

The adapter supports Codex and Claude API upstream profiles, approved native
Linux file logins and custom
ACP commands present in the configured image. Readiness comes from that
image's actual ACP transport, rather than host installation. The image must
contain compatible glibc, the harness/adapter and its tools. Project configuration
and skills remain in original project storage; global host homes and skills are
not projected. Project files, including an existing `.env`, are part of the
explicit storage grant and are readable by the agent. Do not place host-only
secrets in an approved project and expect a broker to hide those files.

API mode uses the host's selected upstream profile and exact thread model.
The broker streams HTTP Responses/Messages, keeps long-lived upstream keys on
the host, and replaces credentials echoed in response chunks. Worker keys are
opaque one-hour thread/environment/generation capabilities. Reconnecting or a
new admitted load renews them; stop, rebuild and binding changes revoke them.
The worker cannot choose another model, redirect an upstream request, forward
arbitrary host headers or open the general local HTTP service. Subscription
OAuth/auth refresh is not proxied by the API broker. Selecting an API profile is
an explicit user configuration, never an automatic login or billing conversion.
Native file mode below uses the native CLI and separately approved HTTPS egress.

For Claude Messages, the broker preserves the SDK's date-shaped
`anthropic-version` and bounded `anthropic-beta` feature selectors. Missing
version uses `2023-06-01`. It rejects duplicate versions, malformed beta names,
more than 32 beta names or more than 2048 input beta-header bytes. Authentication
still comes only from the selected host profile. These headers follow the
[Claude version](https://platform.claude.com/docs/en/api/versioning) and
[beta header](https://platform.claude.com/docs/en/api/beta-headers) protocols
(checked 2026-11-07); they do not enable additional endpoints or imply every
beta feature is supported by the installed native harness.

Isolated Claude requires `claude-agent-acp` 0.101.0. Before creating or restoring
a session, the thin ACP adapter verifies its reported identity/version and pins
its selected auth route in both SDK environment options and programmatic
settings. API mode pins the loopback broker/capability and clears subscription
environment overrides. Native mode pins the approved CLAUDE_CONFIG_DIR and public
Anthropic route, clears API/helper/ambient OAuth env precedence, and lets the
native CLI read and refresh its file login. Both retain project rules, tools and
settings sources. An unverified version or incompatible metadata fails with an
`unsupported_environment_claude_*` error; there is no environment-only fallback.
The requested model must be accepted by the adapter's advertised model picker;
a rejected model selection also fails explicitly before a prompt is sent.
Explicit full access selects Claude's advertised `bypassPermissions` mode. If
project settings or the adapter disable that mode, the request fails explicitly
instead of silently entering another permission mode. Guarded policies retain
their existing permission mapping.
The image's package/ELF installation checks do not establish native protocol
compatibility. Validate its actual ACP/native CLI with a dedicated API fixture
and enforced worker limits before enabling that image.

Capabilities use only the selected harness's API profile and protocol: Codex
Responses or Claude Messages. Custom ACP commands use the explicitly configured
Codex Responses profile and cannot select a Claude credential.

Git status/diff/stage/commit and managed worktrees execute through worker Git.
Host SSH agents, `.ssh`, global Git helpers and credentialed/global MCP are not
projected. Local Git operations work; optional approved gh file login installs a
container-local HTTPS credential helper. Other remote Git/MCP authentication needs
its own precise file/config mount and approved HTTPS destinations, otherwise it
fails without network or credentials. SSH forwarding remains unsupported. Networkless project/image stdio MCP naturally executes inside
the container. Host plugins, native thread forks, host-home role templates,
legacy delegate `--worktree`, cross-device file transfers and workspace downloads
have explicit unsupported errors. A child can use the managed-worktree CLI
within the environment's dedicated root instead. Unrecognized environment CLI
operations are refused by an allowlist.

Native subagents may execute in the container and report live ACP metadata.
Native transcript inspection, scheduled-history recovery, host usage hydration
and full context inspection are unavailable for isolated threads until they
have an environment state adapter. They never scan host harness homes to fill
missing environment history. Supervisor conversation history remains available.
Environment capability snapshots hide native fork and fork-at actions even if
the ACP adapter advertises them. This does not change host session capabilities
or live ACP token and context-window usage reporting.

A private worker loopback gateway implements thread self, messages, child
creation and managed-worktree access without a device registration, machine
CLI config, database or Docker socket. Workspace file HTTP requests include
`threadId`; an ambiguous isolated workspace request is rejected. Selecting a
managed worktree updates the file backend and next harness cwd. Preview mappings
store environment + generation + port and forward through worker TCP, retaining
existing Relay sharing, leases, revocation and HTTP/WS/SSE transport. Rebuilding
invalidates an old mapping's execution target; create a current mapping again.
Changing a thread binding revokes its old preview leases and waits for their
active streams to close before acknowledging the change. Arbitrary linked host
file previews are unavailable for isolated threads. Attachment images are read
and validated inside the worker; the UI serves the original instead of creating
a host thumbnail cache. Use scoped file reads for individual workspace files;
the existing file/archive download endpoint is explicitly unsupported.
Treer/externally hosted harness integration has no execution adapter in this
version. Creating or binding an isolated environment with that integration is
rejected; enabling it later also blocks execution of retained isolated bindings.

Egress has no container network route. A separate authenticated CONNECT bridge
allows only configured exact DNS names on port 443, resolves and pins public IPs,
and rejects private, loopback and metadata addresses. Git and package tools that
honor `HTTPS_PROXY` can use it; no SSH, UDP, general HTTP proxy or unrestricted
host network is supplied. The same container remains a shared-kernel Docker
security boundary, not a VM or an untrusted multi-tenant host boundary. E2B is a
future remote provider and would require an explicit storage/transport adapter;
local absolute paths do not become remote mounts.


## Optional direct credential grants

All non-broker credentials are absent by default. Agent proposals may request up
to 16 existing regular files/directories with exact absolute source/target paths,
RO/RW, purpose (`generic`, `codex`, `claude`, `github`) and allowlisted config-path
pointers. Targets are under `/credentials`; the only state exceptions are native
read-only auth-file slots. Whole HOME, private control-plane/DB/identity storage,
system/special files, nested symlinks or hardlinks, Docker/SSH sockets, worker/bridge/identity paths and overlapping
targets are rejected. Symlink/non-normal source paths and source replacement
require new review. Directory contents are part of the explicit grant, including
nested files. Directory metadata review is bounded to 100,000 entries; narrow a larger grant.
The only nested-link exception is Codex's generated `tmp/arg0[-linux]/codex-arg0*/`
helper aliases (`apply_patch`, `applypatch`, `codex-linux-sandbox`,
`codex-execve-wrapper`) to an absolute `codex` executable. Review never follows
those links; they resolve inside the container namespace. Other links, sockets
and hardlinks remain unsupported. This permits reapproval/rebuild after native
Codex has created its own dispatch helpers.
Metadata-only dev/inode/type checks prove the daemon source and the
running bind match Supervisor storage without reading/hashing credential bodies or
writing a challenge into an RO directory. Source identity is checked on admission
and reconciliation; replacement fences/quiesces the environment.

A direct bind exposes the approved long-term credential to environment processes;
it does **not** keep that credential Host-only. No credential is copied to state.
RW grants reserve overlapping sources exclusively across Pockymoe environments
until computation destruction, even while stopped/failed. Native sessions have
one supported active process per tool/config in an environment; in-process native
subagents remain supported. Arbitrary external Host programs do not honor our lock.

`nativeFile` Codex requires a real ChatGPT `auth.json` file store and an approved
`CODEX_HOME` directory, pinned Codex 0.167.0/codex-acp 2.5.0 and a compatible model.
It forces ChatGPT/file/openai startup and removes API-only overrides. Conflicting
custom providers/profiles in native config are rejected instead of changing
billing. `nativeFile` Claude requires Linux `.credentials.json` under the approved
`CLAUDE_CONFIG_DIR` and claude-agent-acp 0.101.0 / SDK 0.3.318. The native CLI owns
its OAuth refresh. OS keyring/auto-with-only-keyring and macOS Keychain credentials
are not exported or treated as file authentication.

A native directory must be writable for native state and atomic auth replacement;
its config/history are explicitly included. Alternatively a single RO auth file can
bind to `/pockymoe/state/codex/auth.json` or
`/pockymoe/state/claude/.credentials.json` with its tool home pointing to that state
parent. This does not authorize the source parent and cannot persist OAuth refresh.
A cached Codex account response does not prove refresh succeeded: the pinned native
CLI can log a proactive refresh error and still return the cached account.
A Host atomic replacement of a single file changes its inode and invalidates the
grant; a direct directory bind sees native atomic replacements inside it.

`github` projects a file-backed `hosts.yml` directory via `GH_CONFIG_DIR`. The image
supplies gh and the worker installs a non-secret HTTPS Git helper invoking that
gh. Ambient GH_TOKEN/GITHUB_TOKEN cannot override it. Keyring-only hosts.yml and
SSH protocol fail explicitly. The pinned gh 2.45 needs `config.yml` schema
`version: 1` on a read-only directory. Migrate the original configuration on Host
first, or submit a new request approving that directory read-write; the worker
rejects the older read-only schema and never copies credentials to bypass it.
Generic grants support tool config-path variables
and CA file pointers (SSL_CERT_FILE, NODE_EXTRA_CA_CERTS, CURL_CA_BUNDLE), not arbitrary
Host environment forwarding, executable paths, API token values or identities.

Native Codex requires exact `auth.openai.com` and `chatgpt.com`; Claude requires
`api.anthropic.com` and `platform.claude.com`. gh/Git and optional features need their
own explicit HTTPS hosts. The approved request's list replaces connection default
egress for this environment. TCP/UDP/SSH, private/metadata/management IPs and non-443
CONNECT remain denied. An expired capability must be renewed by a fresh admitted
session; a native auth refresh is distinct from gateway capability renewal.

Authentication source contracts checked 2026-11-07: [Codex file/keyring/auth and
refresh](https://learn.chatgpt.com/docs/auth), [Claude Linux file/Keychain and env
precedence](https://code.claude.com/docs/en/authentication), [gh auth storage](https://cli.github.com/manual/gh_auth_login)
and [gh config/token environment](https://cli.github.com/manual/gh_help_environment).
The gh read-only migration contract comes from its
[versioned multiple-account migration documentation](https://github.com/cli/cli/blob/v2.45.0/docs/multiple-accounts.md#immutable-config-users).
