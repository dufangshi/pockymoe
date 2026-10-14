Pockymoe can now run inside Treer. One Supervisor per device runs as a Treer
Managed App (the hub), and each top-level thread appears in Treer as an Agent
through the new `pockymoe attach` command. Treer can prompt, queue, steer,
interrupt and read a thread's transcript, observes working, idle and blocked
(pending approval) states, and receives completion notifications. Opening the
Agent in Treer shows the Pockymoe thread page, limited to that thread and its
child threads. Stopping the Agent interrupts its turn and releases the harness;
starting it again rebinds the same thread and history. Setup, pinned
installation and launch profiles are in `integrations/treer/` and
`docs/treer-integration.md`.

Supporting changes: the Web UI also works when served beneath a path prefix;
`POCKYMOE_HOST_MODE=hosted` turns off relay, account binding, device peer
access and port previews for a Supervisor embedded in another control plane;
`POCKYMOE_SUPERVISOR_RESTART_STRATEGY=host-managed` lets an owning process
manager relaunch the Supervisor after an update instead of the built-in
updater (the default is unchanged); and thread item kinds, statuses and event
types are typed and checked against the Web contract (`docs/thread-event-schema.md`).
This release also stabilizes connection recovery, keeps final replies clear of
the composer, lets thread viewer reports pass through relay sockets, tightens
resource readings, and fixes the color mode of single-mode theme packs.

Validated with the runtime, Supervisor, CLI and Web unit suites, the static Web
build, and a full authenticated run against a local Treer control plane with a
Docker host device (eleven checks, using the fake runtime, not real inference).
Real Claude, Codex and Grok harnesses were not exercised through Treer. The
3Dmol and draw.io previews do not yet load under a Web path prefix. The Treer
packaging installer supports Linux and macOS only. Windows Device Manager is
unchanged.
