# Connection and encryption

The lock icon on device rows and thread pages shows the state of the connection between this browser and the device.

## Encrypted {#encrypted}

Messages, files, terminals and other private content are encrypted end to end between this browser and the device. The relay only sees routing information such as which device, data sizes and timing.

## Not verified yet {#not-verified}

The browser has not confirmed the device's identity yet. Reconnecting once is usually enough; if it stays like this, check that the device is online and runs a current Pockymoe.

## Legacy transport {#legacy}

The device runs an older Pockymoe, so content through the relay is not encrypted. Update it from the device's settings or rerun its setup command on the device.

## Identity changed {#identity-changed}

The device showed a different identity than the one saved before, and this browser stopped the connection.

1. Run `pockymoe relay-fingerprint` on the device.
2. Compare it character by character with the SHA-256 fingerprint the Web shows.
3. Trust the new identity only if they match. If they do not, do not trust it; check the device and the relay first.

## Password required {#password-required}

A protected device only accepts requests signed by this browser. When a password prompt appears, enter your account password to continue. See [why the password is asked for](/help/account#password-prompts).

## Device offline {#offline}

The device is not connected to the relay. Check that it is on, that Pockymoe is running and that it has network access. The Web reconnects on its own when the device comes back.
