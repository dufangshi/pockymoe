Owners can now isolate a device or a workspace from agents outside it.

- **Devices → ⋯ → Agent isolation…**: agents on other devices can no longer list,
  read or message this device's threads, list its workspaces, read its files or
  create threads on it.
- **Workspaces → ⋯ → Isolate workspace**: agents outside the workspace, on this
  device or another one, can no longer list, read, message, wait for or close its
  threads, create threads in it, set automations on them, run commands there or
  read its files. Isolated workspaces show an **Isolated** badge.

Isolation is inbound only, stored and enforced on the device, and changed only by
the owner. Your own use of the Web UI and sharing with other people are not
affected. It covers the `pockymoe` CLI, cross-device requests and Treer prompts
and messages from agents outside; it is not an operating-system sandbox. See
[agent isolation](https://github.com/dufangshi/pockymoe/blob/main/docs/agent-isolation.md).

Validated with the Supervisor, runtime and CLI suites, new isolation tests for
local, cross-device, automation and Treer paths, and the Web tests. Relay and
Windows Device Manager are unchanged. Installing this release is a separate
device update; publication does not restart an existing Supervisor.
