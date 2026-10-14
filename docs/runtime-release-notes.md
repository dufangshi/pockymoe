Pockymoe inside Treer now syncs threads both ways. In a Pockymoe page opened
from a Treer Agent you can create, rename and delete top-level threads: creating
one also creates its Treer Agent in the same folder, renaming renames the Agent,
and deleting removes the Agent and the thread. Every Treer-bound thread on the
same hub (and its child threads) can be opened, switched in tabs and shown in
split view from any of those pages; hub threads without a Treer Agent stay
invisible, and management, settings and device pages remain unavailable there.
Browser navigation state is kept per hub, and references that can no longer be
opened are dropped quietly instead of showing an error.

`pockymoe attach` now reports what it is waiting for (hub token, thread setup)
and shows the hub's reason when it refuses a binding, such as a missing base
agent. `treer-setup.sh` checks the Codex and Claude ACP adapters after setup and
replaces missing, external or unverified ones with a verified Pockymoe-managed
adapter, and rerunning it with a newer `--version` upgrades an existing hub
through its own update path.

Validated with the runtime, Supervisor, CLI, setup and Web unit suites, the
static Web build, and live runs against a local Treer control plane with
authentication enabled (thread create, rename, delete, cross-thread navigation,
gate denials and attach diagnostics, using the fake runtime). The adapter check
and the hub upgrade path were tested against a simulated hub, not yet against
real Codex or Claude adapters. A thread whose Agent is deleted from the Treer
side is kept on the hub. Windows Device Manager is unchanged.
