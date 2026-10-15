# Devices and protection

Each device row has two small icons: the lock shows whether the connection is encrypted, and the shield label shows the device's protection status. Open either one for the reason and what you can do.

## Protected {#protected}

The device is bound to your account key. Only browsers you signed in to, and people you shared with, can send it commands; the relay cannot impersonate you.

Nothing to do.

## Not protected {#not-protected}

The device has no account key yet and trusts the relay: whoever controls the relay can also send this device commands.

Two ways to protect it:

- **One-click protection**: choose **Protect all devices** on the Devices page, or open the device's **Not protected** label → **Protect this device**, then enter your password. Fine for most cases; it trusts the relay only at the moment of binding.
- **Local bind command** (most secure): **Protect this device** → **Show local bind command**, then run the command in a terminal on the device.

## Waiting for bind {#waiting-for-bind}

A local bind command was created, but the device has not run it yet.

1. Run the command in a terminal on the device. **Show local bind command** in the label shows it again.
2. Keep this browser open; it finishes when the device connects.

If you cannot reach the device, choose **Use one-click protection instead**.

## Waiting for device {#waiting-for-device}

Protection is queued, but the device is offline. It continues on its own when the device comes back online while this browser is open. The queue is kept for 7 days.

If the device stays offline, check that it is on and that Pockymoe is running.

## Update needed {#update-needed}

The device runs a Pockymoe version that cannot be bound from the Web.

1. Choose **Update and protect**. The device updates, restarts and then continues.
2. If it shows **Update failed**, update it from the device's settings or rerun its setup command on the device, then choose **Retry**.

You can also use the local bind command instead.

## Protecting {#protecting}

The device is being bound and receives its certificate. This normally takes a few seconds. Reload the page if nothing changes for a long time.

## Bound to another account {#other-account}

The device reports a binding to **a different** account key, so it cannot be bound to yours. Common causes:

- A bind command for someone else's account was run on the same computer and reached your device. This happens easily when one system user runs several Pockymoe instances.
- The device was bound to an old account key you replaced with **Start over**.

To fix it:

1. On the device, run `pockymoe bind --status` and note the 12-character account ID after `Bound to account`.
2. Open the device's **Bound to another account** label, or the Account page, to see **your account ID**.
3. If they differ, run `pockymoe bind --reset` on the device and restart Pockymoe there. When the computer runs several Pockymoe instances, add `--database <this device's database path>` to both commands.
4. Back on the Devices page, choose **Retry** or use the local bind command.

> If both account IDs match, do not reset. The device is fine and the relay may be giving wrong information; contact the relay administrator.

## Needs repair {#needs-repair}

The device is bound to your account but did not show a valid device certificate. This happens after its data was restored, or when the certificate was lost or removed.

Choose **Repair device binding** and run the repair command it shows on the device. Until then the device stays disconnected instead of falling back to trusting the relay.

## Identity changed {#identity-changed}

The device's identity key differs from the one this browser saved. The device may have been reinstalled or wiped, or something may be impersonating it.

- If you just reinstalled or moved the device: run `pockymoe relay-fingerprint` on it, compare it character by character with the SHA-256 fingerprint the Web shows, and only then confirm the new identity.
- If you did not touch the device: do not confirm; check the device and the relay first.

## Protection failed {#protection-failed}

The last attempt did not finish, for example because the network dropped or the device restarted. Choose **Retry**; if it keeps failing, use the local bind command.
