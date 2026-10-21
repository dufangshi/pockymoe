A delegate's report reaches an agent that is still working instead of waiting
for its turn to end, and people a conversation is shared with can open the port
previews its agent creates.

- **Wakes join the running turn**: when the agent threads an agent registered a
  wake for (`thread wait --wake`) all finish or one is blocked, their report is
  delivered as an interjection into the agent's running turn when its harness
  takes steers, for example while Claude waits on a native subagent. An idle
  agent, or one whose harness cannot steer, gets it as its next turn as before.
- **Wakes say why and where from**: the report's first line says whether the
  agent threads finished or one is blocked, and that it adds information to the
  delegated task rather than giving a new instruction. It no longer counts as
  something you said. Wakes still waiting show above the composer as **Waiting
  for wake** with each agent thread's status; a queued report reads **Delegate
  wake** with each agent thread's state instead of its raw header.
- **Shared previews**: a port preview an agent creates with `pockymoe preview
  create` records its conversation. Anyone that conversation (or one of its
  parents) is shared with, or whose share covers the whole device, can open the
  link the agent gives them; revoking the share ends their access at their next
  request. Previews you create yourself stay yours. Renewing a preview's access
  stays with you or the conversation's agent (`preview create` again prints a
  fresh link).
- **Install from GitHub**: the README's one-line install now downloads
  `setup.sh` from the latest GitHub release instead of the Pockymoe website.

Validated with the runtime coordination tests (a wake steered into a running
turn, a queued wake, a blocked wake's header), the relay preview tests (a
recipient of the creating conversation opens it, other accounts and revoked
shares are refused) and Supervisor port tests, the shared UI and Web tests,
end-to-end runs of a shared preview on bound and unbound devices, and the wake
rows in every theme, light and dark, on phone and desktop. Both the relay and
the device must be updated for shared previews. Installing this release is a
separate device update.
