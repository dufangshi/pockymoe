# Agent isolation

By default every agent of an account can reach every thread: on the same
device through the `pockymoe` CLI, on other devices through
[cross-device peers](cross-device-peer.zh.md) and, in Treer, through Treer
prompts and messages. The owner can close a device or a workspace to agents
outside it. Isolation is about agents only. It does not change what you can do
in the Web UI, or what people you share threads and devices with can do.

## Settings

| Where | Effect |
| --- | --- |
| **Devices** page, a device's **⋯ → Agent isolation…** | Agents on other devices cannot list, read or message this device's threads, list its workspaces, read its files or create threads on it. |
| **Workspaces** page, a workspace's **⋯ → Isolate workspace** | Agents outside the workspace, on this device or another one, cannot list, read, message, wait for, close or create threads in it, set automations on them, run commands there or read its files. |

Isolation is inbound only: threads of an isolated device or workspace can still
reach unisolated ones. An isolated workspace shows an **Isolated** badge; a
device row shows one once the Devices page has talked to the device.

Isolation is stored on the device (KV key `agent:isolation`) and enforced there;
the relay can neither see nor change it. Only the owner can change it:
`GET`/`PATCH /api/config/agent-isolation` (`{"device": bool}`) and
`PUT /api/config/agent-isolation/workspaces/{id}` (`{"isolated": bool}`) are
under `/api/config`, which grantees and other devices cannot reach.
`WorkspaceDto.agentIsolated` reports a workspace's state.

A device whose [peer access](thread-interaction.md#other-devices) is off is
already closed to other devices, so it reads as isolated. Turning device
isolation off also turns peer access on; turning it on keeps peer access, so the
device's own agents can still reach other devices.

## Devices shared with other people

Isolation closes a device to agents elsewhere. The opposite direction has a
fixed rule: a device that someone else can control cannot reach your other
devices. It applies while the device has a live share or access grant that lets
another person control threads or create them; read-only sharing does not count.
People you share with control agents that run as the Supervisor's OS user, so
they could turn the device's own peer access on; the relay and your other
devices therefore refuse its requests (`403 peer_source_shared`) whatever that
switch says. Your other devices can still reach the shared device, subject to
its peer access and isolation. The Devices page marks such a device with a small
icon; revoke the control (or make it read-only) to lift the rule. The relay
decides whether a device is shared; see the
[cross-device contract](cross-device-peer.zh.md) §3 and §5.

## Who counts as inside

The device decides from what it can verify:

- A thread's own CLI credential (`POCKYMOE_TOKEN`) identifies that thread; it is
  inside the workspace the thread belongs to, worktrees included.
- Requests from other devices are outside every workspace and are refused
  entirely while the device is isolated. Only `info` still answers, with
  `isolated: true`.
- In Treer, a prompt or message whose sender is one of this hub's threads counts
  as that thread; any other agent is outside. Prompts and messages without an
  agent sender come from the person using Treer and are not restricted. A
  refused Treer message is acknowledged and dropped; a refused prompt answers
  `agent_isolated`. Treer reads an agent's status and transcript without
  naming the caller, so those stay under Treer's own permissions.
- The machine's own CLI credential (the connection file next to the database)
  is the owner, like the Web UI.

Refusals are `403 forbidden` with a message naming the isolated workspace or
device. Listings (`thread list`, `device workspaces`) leave out what the caller
cannot reach.

## Limits

Isolation governs how agents use Pockymoe; it is not a sandbox. Agents run as
the same operating-system user as the Supervisor, so an agent that ignores the
CLI could read workspace files, the database or the machine credential directly,
and in local mode without a [passcode](local-mode.md#phones-and-other-computers)
it could call the Web API on the loopback address. Use separate OS users or
devices for a hard boundary. The bundled agent skill tells agents to report an
isolation refusal to the user instead of working around it.

Arrangements made before isolation keep working: automations that watch or act
in a now-isolated thread, `--notify-on-complete` subscriptions and `wake`
registrations. Review a thread's automations in its settings if they should
stop.
