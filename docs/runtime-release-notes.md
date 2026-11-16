Fixes a process leak that could use up a device's memory.

- **Leaked worker processes**: every command, Git read or agent that Pockymoe
  stopped (rather than let finish) left one idle `environment-worker host-run`
  process behind, and they were never cleaned up. A busy device collected
  thousands of them, tens of gigabytes in total. A stopped worker now exits at
  once.
- **Updating clears the old ones**: the update restarts Pockymoe, and the
  processes already left behind exit with it. Nothing has to be killed by hand.
