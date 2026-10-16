Rerunning the local-mode setup command with other options, such as `--lan`,
now applies them. Before, a Pockymoe running without a user service manager
(some containers, Windows) stopped setup with "already running ... Update it
from Settings", and the new options only took effect after a restart. Setup now
restarts it, also under systemd and launchd, and waits instead of restarting
while an agent is working on a turn.

Settings → Update and Restart on a local-mode install now restart its own
service. Version 0.13.7 started the new release outside the service, so the next
login could start the previous release again. Updating a 0.13.7 local install
still runs 0.13.7's updater once; the new release then points the service at
itself, and the next update, restart or reboot moves Pockymoe back into its
service.

`pockymoe passcode off` now confirms that the passcode was removed. Relay
devices, their services and databases are unchanged. Windows Device Manager is
unchanged. Installing this release is a separate device update; publication does
not restart an existing Supervisor.
