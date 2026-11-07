# Dokploy execution environments

An execution environment runs the Pockymoe worker in a Linux Docker container
managed by Dokploy. The host Supervisor keeps thread identity, approvals,
database, credentials and the execution gateway. The environment is not a
registered device. Lifecycle operations use the Dokploy REST API, validated
against Dokploy v0.30.6; Docker CLI access is limited to trusted control-plane
storage proofs and container inspection. Agent work uses the Supervisor worker
transport. An unavailable environment never requests host execution.

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

Settings → Environments configures Dokploy connections, exact HTTPS egress
hosts, per-container limits and a Supervisor budget. Keys are write-only.
A new thread can select a connection using `environmentConnectionId` on
`POST /api/threads/start` (also the workspace start route). This creates and binds
a stopped environment before recording the unloaded thread. Start the
environment before its first prompt. Existing threads use the thread environment
control and explicitly confirm `resetSession: true` before switching; conversation
history is retained. Inherited child bindings cannot switch to host execution.

The first adapter supports Codex and Claude API upstream profiles and custom
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
OAuth/auth refresh and model-provider WebSocket protocols are not supported;
selecting an API profile is an explicit user configuration, never an automatic
login or billing conversion.

Git status/diff/stage/commit and managed worktrees execute through worker Git.
Host SSH agents, `.ssh`, `gh` tokens, Git helpers and credentialed/global MCP are
not mounted or copied. Local Git operations work; authenticated remote Git and
HTTP MCP need separate credential adapters and are rejected or fail with the
network disabled. Networkless project/image stdio MCP naturally executes inside
the container. Host plugins, native thread forks, host-home role templates,
legacy delegate `--worktree`, cross-device file transfers and download archives
have explicit unsupported errors. A child can use the managed-worktree CLI
within the environment's dedicated root instead. Unrecognized environment CLI
operations are refused by an allowlist.

Native subagents may execute in the container and report live ACP metadata.
Native transcript inspection, scheduled-history recovery, host usage hydration
and full context inspection are unavailable for isolated threads until they
have an environment state adapter. They never scan host harness homes to fill
missing environment history. Supervisor conversation history remains available.

A private worker loopback gateway implements thread self, messages, child
creation and managed-worktree access without a device registration, machine
CLI config, database or Docker socket. Workspace file HTTP requests include
`threadId`; an ambiguous isolated workspace request is rejected. Selecting a
managed worktree updates the file backend and next harness cwd. Preview mappings
store environment + generation + port and forward through worker TCP, retaining
existing Relay sharing, leases, revocation and HTTP/WS/SSE transport. Rebuilding
invalidates an old mapping's execution target; create a current mapping again.

Egress has no container network route. A separate authenticated CONNECT bridge
allows only configured exact DNS names on port 443, resolves and pins public IPs,
and rejects private, loopback and metadata addresses. Git and package tools that
honor `HTTPS_PROXY` can use it; no SSH, UDP, general HTTP proxy or unrestricted
host network is supplied. The same container remains a shared-kernel Docker
security boundary, not a VM or an untrusted multi-tenant host boundary. E2B is a
future remote provider and would require an explicit storage/transport adapter;
local absolute paths do not become remote mounts.
