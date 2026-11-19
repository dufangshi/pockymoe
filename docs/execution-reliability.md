# Execution state, message delivery, and Supervisor ownership

A second Supervisor that opens the same database (for example a test Supervisor inheriting the production `POCKYMOE_DATABASE_PATH`) must never run startup recovery on it, or it rewrites live turns as interrupted while the real Supervisor keeps running them. The UI then mixed live output with an obsolete database snapshot, and a prompt endpoint acknowledged a request before its background admission failed.

## Ownership and state authority

`Database::open` takes a nonblocking OS exclusive lock before migrations or startup recovery. The lock lives as long as the database, resolves symlink aliases, and releases automatically when its owning process exits. The companion lock file is intentionally retained: unlinking it would allow another process to lock a different inode. This coordinates cooperating Rust Supervisors; older versions must be upgraded to participate. Playwright also clears inherited device settings and explicitly supplies both primary and legacy test database/workspace variables.

All supported providers use the common `AgentRuntime::execution_state` contract. The ACP runtime delegates interpretation to the harness adapter, using its existing ACP-owned connection and outstanding prompt request. Standard ACP defines output updates, prompt completion with a stop reason, and cancellation acknowledgement, but not a portable query that reattaches to an outstanding prompt on an unrelated connection. The generic adapter therefore reports running only for its tracked request on a live connection, idle for a connected session without a tracked request, and unknown when the connection cannot be verified. Native read-only extensions can refine this adapter contract without opening a second writer. Transcript timestamps are never treated as proof that a process is running.

Protocol references: [ACP prompt lifecycle](https://agentclientprotocol.com/protocol/v1/prompt-turn), [ACP session setup](https://agentclientprotocol.com/protocol/v1/session-setup).

The Supervisor checks active executions on reads and every two seconds. Observations are serialized with turn admission/settlement and matched against the current turn ID. A running backend repairs stale local status. A missing completion after connection loss or an ordinary Supervisor crash becomes `recovering` (displayed as “Confirming status”), with no completion timestamp. Input remains queued until reconnection confirms a usable idle session. Explicit reconnect never resends the uncertain original prompt. It retains the interrupted history and can deliver the user's queued instructions. An adapter unable to establish idle state keeps the task unresolved rather than guessing.

Runtime event streams are scoped to a thread/turn invocation and closed after completion. Late callbacks cannot mutate subsequent turns. Completion is persisted before broadcasting, and persistence failures produce a visible error instead of exposing unsaved output. The UI uses the neutral label “Interrupted”; a terminal status alone does not prove that a user pressed Stop.

Every frame written to an ACP adapter's stdin, including the wait for the stdin lock that sessions on the same process share, must finish within 30 seconds. Control requests then wait up to 180 seconds for their response; `session/prompt` waits without limit. A write that misses its deadline or fails may have left half a frame on the pipe, so the Supervisor closes that connection, fails its pending requests and stops the process; the next turn starts a fresh one. A writer that only timed out waiting for the lock wrote nothing and just fails.

ACP terminal commands keep the newest output up to their `outputByteLimit` (at most 1 MiB) in a ring buffer and report `truncated` once anything was dropped; a cut window starts at a UTF-8 character boundary. A command's exit status is published only after its stdout and stderr reach EOF, for at most 2 seconds after the exit, so a client that waits for the exit and then reads the output gets the last lines. Output from descendants that keep the pipes open longer still reaches the buffer while the terminal exists.

## Durable input

The HTTP prompt endpoint stores the complete input (including images and model/effort overrides) before acknowledging it. Pending rows are also used for first-turn admission, so runtime preflight failure cannot silently erase accepted input. The row is consumed in the same transaction that creates the turn and its user message. Per-thread dispatch serialization prevents concurrent consumers.

`clientRequestId` is a durable idempotency key scoped to the thread. A receipt stores a SHA-256 digest rather than duplicating large image payloads; retrying the same request confirms its original acceptance, including after restart or consumption. Reusing the key with another payload is rejected. Undispatched input survives restart; inputs attached to an uncertain execution wait rather than being blindly replayed. Errors after acceptance leave an explicit delivery error and either the pending input or its failed turn history.

This provides durable acceptance and deduplicated dispatch, not exactly-once external side effects. If a backend connection disappears after a tool changed a file or made a network request, its effects cannot be inferred solely from the transport. Reconnection/maintenance continuation tells the agent to inspect existing state before repeating work.

## Manual restart and access

`GET /api/management/supervisor` includes `startedAt`, monotonic `uptimeSeconds`, `processId`, and `canRestart`. Settings displays elapsed uptime and a manual restart action.

`POST /api/management/supervisor/restart` uses the existing independent maintenance worker. It retains the current executable and version, makes no registry request, and performs no package installation. It records the currently running turns, pauses them, persists output, restarts the process, verifies health (and relay reconnection in relay mode), then resumes only the marked tasks in their existing sessions. Existing queue entries and permission settings survive. Update and restart share concurrency guards. User Stop still wins over maintenance recovery.

Through the relay these endpoints are device scoped. Only effective device-owner access is allowed. Thread, workspace, and whole-device sharing grants cannot invoke management operations even when they allow thread control or workspace writes. The UI hides controls on owner-only denial; the relay enforces the boundary for direct requests and encrypted forwarding as well.

## Verification

- Rust workspace tests cover OS locking across processes and symlinks; live state repair and idempotent queued delivery for Codex, Claude, OpenCode and ACP; acceptance before dispatch/restart; uncertain ACP process exit; stale event rejection; persistence failure visibility; and owner-only management.
- Desktop Chromium `e2e/session-state-recovery.spec.ts` sends from the composer, corrupts an isolated status snapshot, verifies repair, queues input, reloads, and verifies exactly one subsequent user turn. The test launcher is deliberately given a hostile inherited primary database setting.
- Component tests cover uptime, restart confirmation, route-bound requests, shared-access restrictions and truthful timeline labels.
- `scripts/test-supervisor-restart-live.mjs` runs only in the isolated Linux machine. It starts the real Supervisor and independent restart worker with deterministic harnesses for all four providers, blocks external registry access, verifies PID change without version change, the original session IDs, queued delivery once, and no restart of an idle thread. This is separate from real ACP protocol fixture tests.
