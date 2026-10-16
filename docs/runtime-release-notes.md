Inside Treer, every Pockymoe thread is now exactly one Treer Agent, and both
sides keep the same record. Creating a delegate with `pockymoe thread create`
goes through Treer's own Agent creation with the parent's identity, so Treer
Policy applies; the child appears in Treer as `parent.name` in the parent's
folder, has its own Treer identity, and Pockymoe still groups it under its
parent. Closing a delegate stops its Agent and keeps its name; prompting it
starts the Agent again. Deleting and renaming sync in both directions, and a
reconciliation pass picks up Agents deleted or renamed in Treer. Threads made
without a Treer caller (the hub's own page, automations) are registered with the
hub App's identity, and children left unregistered by earlier versions are
migrated on startup.

Mail between Pockymoe threads now travels as Treer Core Messages and queue/steer
as `treer agent prompt`, so Treer's message history matches Pockymoe's inbox.
Core Messages sent to a Pockymoe Agent arrive in its thread's inbox; a message
from a person also queues a prompt, while completion notifications and agent
mail stay passive. Opening the hub App in Treer shows a read-only status page
instead of an error, and if the hub App is stopped while Pockymoe Agents are
running, they start it again within a few minutes.

None of this changes standalone Pockymoe: the behavior is active only on a hub
started with `POCKYMOE_HOST_MODE=hosted` and `POCKYMOE_HOSTED_BY=treer`.
Validated with the full runtime, Supervisor, CLI and setup suites, a standalone
regression run, and a live run of nine scenarios against a local Treer control
plane with authentication enabled (fake runtime). Existing attach Agents gain
hub-App recovery after their next restart. Windows Device Manager is unchanged.
