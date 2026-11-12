Adding a device works from start to finish again, including on a computer that
was a device before.

- **A new device becomes usable by itself.** After the setup command runs, the
  Add device window shows "Waiting for the device to connect…" and then
  "Connected and protected." with **Open device**. Before, the device came online
  but refused every request ("This protected device is missing its account
  certificate") until a small chip on its row was found and pressed.
- **When the password is needed**, the device says so where you meet it: a
  **Finish protection** button on its card, in the Add device window and on its
  own page instead of "Unable to load workspaces".
- **Remove a device, then add the computer again.** Running a new device's setup
  command on a computer that was another device now registers it as the new one
  and keeps its threads and workspaces. Before, setup printed "Device is already
  running" and the new device stayed offline for good.
- **Running a device's setup command again fixes an offline device**: a device
  that runs but cannot reach the relay is restarted, and a device the relay
  refuses is told why (it was removed, or its token was replaced) instead of
  waiting two minutes.
- **A removed device leaves the Devices page at once.** Before, its card stayed
  as an offline device, sometimes for good, and removing it again failed with
  "Device not found".
- An offline device no longer shows "Needs repair"; that label is kept for a
  device that connects without a valid certificate.
- Setup refuses to turn a computer that is online as one device into another:
  remove the old device on the Devices page first.
