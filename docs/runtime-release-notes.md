Account-key trust: a compromised relay can no longer command a protected device
or read its traffic. The account password unlocks, in the browser, an account
key that the relay stores only encrypted (Argon2id); the relay never receives
the password again after a one-time upgrade. Each browser gets its own
non-expiring certificate, so a browser stays signed in until it is removed in
Account → Security. Devices are bound to the account key once: new installs bind
during setup, existing devices with "Protect this device" (local `pockymoe bind`
command) or "Protect all devices" (one click, trusting the relay only at that
moment). A protected device refuses plaintext and unsigned relay requests.

Sharing is enforced by the device itself through grants the owner's browser
signs; the relay cannot create or widen a share. Recipients verify the owner
with a safety code or an invite link. Devices of one account authenticate each
other with account-signed certificates for cross-device messages and files.
Public links are encrypted with a key that stays in the link. Port previews on
protected devices need an owner-issued lease.

The Web is now a static site at https://app.lnz-study.com (GitHub Pages); the
relay at https://remote.lnz-study.com serves only the API and redirects old
pages. On first sign-in, existing accounts choose "First sign-in since the
security upgrade" and save a recovery code; passwords shorter than 12
characters must be changed, and Google/GitHub accounts set an unlock password.
Administrators can still issue a temporary password, but regaining device
access then needs the user's recovery code.

Validated with Rust protocol/relay/Supervisor/runtime/CLI suites, Web unit
tests, desktop and mobile Chromium end-to-end runs (account keys, enrollment,
one-click protection, sharing, peers, previews) and a live trial deployment.
Unbound devices keep working as before. Windows Device Manager is unchanged.
This release does not force-update or reinstall the current device.
