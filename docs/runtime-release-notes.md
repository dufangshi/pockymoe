Starting or resuming a thread no longer fails with "Too many open files" on a
busy device.

- **Open-file limit**: Pockymoe now raises its own limit at startup from the
  usual 1,024 (256 on macOS) to 65,536, or to the system's hard limit when that
  is lower. Agents and terminals it starts get the same limit.
- The new limit applies after the update restarts the Supervisor. Agents that
  were already running keep the old limit until they are loaded again.
