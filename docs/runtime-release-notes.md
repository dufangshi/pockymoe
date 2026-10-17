A device you share with people who can control its agents (a share or grant
with control, or permission to create threads) can no longer reach your other
devices through cross-device agent access. Before, an agent those people drove
could turn on that device's own peer access and read or command your other
devices that had access on. The relay now refuses such requests with
`peer_source_shared`, and devices check the same flag. Read-only shares do not
count, and your other devices can still send work to the shared device.

On the Devices page such a device shows a small unlinked-chain icon; hover or tap
it for the reason. Results that a shared device would send back with
`--notify-on-complete` are refused as well, so read its status or transcript
instead.

Validated with the relay, Supervisor and protocol suites, new tests for which
shares count, the relay refusal, the device-side check and the outbox, and the
Web tests. The relay is redeployed. Windows Device Manager is unchanged.
Installing this release is a separate device update; publication does not
restart an existing Supervisor.
