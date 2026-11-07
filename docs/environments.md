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
storage and budgets, creates or recovers the deterministic Dokploy Compose
identity, deploys it, and verifies the actual container. Stop fences the old
generation, clears queued deployments and waits for active deployments before
running a fixed `compose -p <owned-app> -f docker-compose.yml stop` command
through the Dokploy deployment runner. It checks that no owned container is
running. Rebuild stops the old generation and deploys a new worker using the
same project and state paths. Destroy first runs a fixed Compose
`down --remove-orphans` command and confirms container removal before calling
`compose.delete` with `deleteVolumes: false`. It retains project data, state, bridge
directories and the destroyed resource record. Destroying a resource does not
implicitly remove thread bindings; a bound destroyed resource remains
unavailable until the owner explicitly changes the binding.

A Supervisor restart changes previously active states to `interrupted`, clears
execution targets and advances generation. It does not resume a harness or
replay commands. An explicit stop or rebuild reconciles retained resources.
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
