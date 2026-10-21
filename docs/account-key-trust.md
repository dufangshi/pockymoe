# Account key trust

Each relay account has an account key that only the user's browsers hold in
clear. A device bound to that key obeys only requests signed by a browser that
the key certified, so a compromised relay can neither command a bound device nor
read its traffic (port previews excepted). Signing in once in a browser (password,
plus the relay's second factor when enabled) lets that browser use every device of
the account, with no expiry. A device is bound once: when it is installed, with
`pockymoe bind` on the device, or from the Devices page with
[Protect all devices](#protect-all-devices).

Devices that are not bound keep trusting the relay; the Web labels them
**Not protected**.

Code: device `crates/supervisor/src/owner_trust.rs`, `grants.rs`, `tunnel.rs`,
`auth.rs`, `ports.rs`, `secure_transport/client.rs`; relay
`crates/relay/src/account.rs`, `preview.rs`, `oauth.rs` and `lib.rs` (API-only
mode); Web `apps/supervisor-web/src/lib/account*.ts`, `deviceEnrollment.ts`,
`pendingEnrollmentStore.ts`, `remoteDeviceProtection.ts`,
`deviceSetupCommands.ts`, `relayTransportCrypto.ts`, `signedSharing.ts`,
`sharedDeviceTrust.ts`, `portPreview.ts`, `staticWeb.ts` and
`apps/supervisor-web/staticHosting.ts`; Pages build
`.github/workflows/pages-deploy.yml`. Shared interop vectors:
`crates/relay/test-fixtures/account-key-v2.json`,
`crates/supervisor/tests/fixtures/account-key-v2.json` and
`apps/supervisor-web/src/lib/fixtures/accountKeyVectors.json`.

## Threat model

Against a bound device, a compromised relay cannot:

- send commands, read or change data, create port mappings or change sharing:
  every request through the relay must be encrypted and signed by a browser
  registered on the device;
- read the account key or the password: the relay stores only the encrypted
  vault and a slow hash of a key derived from the password, and nothing it
  answers makes a browser send the password (only the explicit
  [security upgrade sign-in](#account-flows) does);
- substitute the account key for a browser that has opened the vault before;
- substitute the device identity: owner browsers trust it only through a device
  certificate signed by the account key, and the vault records each bound
  device's identity key;
- create, widen or keep shared access: the device holds its own grants, which
  only owner browsers can change;
- open a port preview on its own: a bound device serves a preview connection only
  under a lease that an owner browser or the device's local user created.

Limits:

- **Password strength.** The relay holds `authKey` (sent at every sign-in) and,
  after a database leak, so does anyone with the database. Each password guess
  costs one Argon2id derivation (64 MiB, 3 passes), so a weak password can be
  found offline. A guessed password opens the vault and the account key.
- **Security upgrade sign-in.** An account without a vault is secured after one
  sign-in that sends its existing password to the relay
  (**Continue with this password**). A relay that is compromised at that moment
  learns that password; choose a different one when securing the account. A
  relay can also make an ordinary sign-in fail, and the Web then offers the
  upgrade. Browsers never offer it for sign-in names they have opened a vault
  for, and ask for an explicit confirmation when they have opened any vault. A
  Web build without the upgrade removes the risk ([migration](#migration)).
- **Web code.** The JavaScript that holds the password and keys, and the device
  setup scripts, must come from a separate static origin
  ([self-hosting](self-host-relay.md#separate-web-origin)). A relay that serves
  the Web UI can serve modified code and take over the account. With a separate
  origin, the static host and whoever can change what it publishes hold that
  power instead: for the public service, GitHub Pages and the repository's `main`
  with its pinned shared UI commit ([static hosting](#static-hosting)).
- **Remote binding.** [Protect all devices](#protect-all-devices) trusts the
  relay at the moment of binding: a relay that is compromised then can answer in
  the device's place with an identity of its own and have the browser certify it,
  while the real device stays unbound or is bound to another key. The local bind
  command does not have this window.
- **Fresh browsers.** A browser without a cached vault cannot detect an older
  vault revision or a hidden device certificate. It may then show a device bound
  after that revision as not protected and talk to whatever the relay presents.
  The real device still refuses the relay.
- **Metadata.** The relay sees accounts, devices, shares and grants, which
  devices have certificates, browser labels (browser and platform, such as
  "Chrome on macOS") and times, revocations, recipients' account public keys,
  HTTP methods and paths, thread, workspace and shell IDs, message types, sizes
  and timing, preview lease IDs, and the thread lineage and port mappings that
  devices report in heartbeats.
- **Availability.** The relay can drop, delay or refuse requests, hide devices and
  withhold revocation frames (browsers also deliver revocations, see
  [Revocations](#revocations)).
- **Unbound devices** remain fully controlled by the relay, which can also bind
  them to a key of its choice.
- **Port previews** are not end-to-end encrypted. The relay terminates preview
  HTTPS and sees the traffic. It also sees every lease ID it forwards and can open
  further connections to that mapping until the lease expires
  ([Port previews](#port-previews)).
- **Recipients** trust the owner's account key on first use unless they open an
  invite link. A relay that substitutes a recipient's account key gains only the
  access granted to that recipient; the safety code detects it.
- **Control access.** A recipient with control access to a thread can make its
  agent run commands on the device, including `pockymoe bind --reset`, which
  returns the device to relay trust, and `pockymoe preview create`, which creates
  a local port mapping and preview lease that the thread's recipients can open
  ([Port previews](#port-previews)).

Local authority is outside this model. Whoever can write the Supervisor database
can bind or reset the device (`pockymoe bind`), and whoever can use its local API
(the CLI, including agents running on the device, local mode, server mode with
its admin login) can create local port mappings and preview leases. Strict mode
applies only to requests that arrive through the relay tunnel.

## Keys

| Key | Held by | Lifetime |
| --- | --- | --- |
| Account key (ECDSA P-256) | Private key in clear only in browser memory during an unlock window; encrypted in the vault on the relay | Until the user starts over |
| Vault data key (VDK, 32 bytes, AES-256-GCM) | In clear only in browser memory during an unlock window | Same |
| Browser key (ECDSA P-256) | Non-extractable `CryptoKey` in the Web origin's IndexedDB, one per browser profile and account | Until sign-out or removal |
| Device identity key (ECDSA P-256) | Next to the Supervisor database, with `.transport-identity` in place of its extension (`relay-supervisor.transport-identity`) | Device lifetime |

`accountKeyId = B64(SHA256(account SPKI))` and `browserId = B64(SHA256(browser SPKI))`.
A device's `identityKey` is the B64 uncompressed SEC1 point from its key
descriptor. The account fingerprint shown by devices is the first 12 characters
of `accountKeyId`.

## Password, vault and recovery code

The browser derives everything; for an account with a vault the relay never
receives the password.

- `master = Argon2id(NFC(password), salt, memoryKiB, iterations, parallelism)`,
  32 bytes (RFC 9106, version 1.3, run as WebAssembly). New vaults use 65536 KiB,
  3 iterations, parallelism 1 and a random 16-byte salt. These values are also
  minimums: the browser refuses to derive with weaker parameters and the relay
  refuses to store a vault that names them.
- `authKey = HKDF-SHA256(master, salt = empty, info = "pockymoe-relay-auth-v1")`,
  32 bytes, is sent instead of the password. The relay stores its own scrypt hash
  of `B64(authKey)`.
- `wrapKey = HKDF-SHA256(master, empty, "pockymoe-vault-wrap-v1")` encrypts the
  password slot of the vault.
- New and changed passwords, including OAuth unlock passwords, need at least 12
  characters (Unicode code points after NFC).

`GET /relay/auth/kdf` returns the vault's real parameters for every sign-in name
whose account has a vault (also when it is disabled or an admin reset is pending)
and deterministic fake parameters for every other name (unknown, not yet secured,
OAuth-only without a vault, administrator), so it reveals neither which accounts
exist nor whether they have a vault.

The vault (at most 64 KiB, [format](#vault-format)) holds the VDK twice, in a
password slot and a recovery slot, and a body encrypted with the VDK that contains
the account private key and the bound devices with their identity keys. After
decrypting, the browser imports the private key as non-extractable and checks that
it signs for `accountPublicKey` and that `accountKeyId` matches. It caches
`{userId, accountKeyId, accountPublicKey, revision, boundDevices}` and refuses a
later vault with a different account key ("The relay served a different account
key") or a lower revision ("The relay served an older account vault"). Only
**Start over** clears that cache.

The relay keeps only the current vault; it stores no earlier revisions.
Changing the password or the recovery code re-encrypts the same VDK and keeps the
same account key, so a password or recovery code that leaked still opens any copy
of the vault taken while it was valid. After such a leak, use **Start over** and
bind the devices again; Account settings says so.

**Recovery code.** 20 random bytes in Crockford base32
(`0123456789ABCDEFGHJKMNPQRSTVWXYZ`), shown once as 8 groups of 4 characters
joined by `-`. The user re-types the last group to confirm saving it. Input is
case-insensitive; `-` and spaces are ignored, `O` reads as `0` and `I`/`L` as `1`.
`recoveryKey = HKDF-SHA256(code bytes, empty, "pockymoe-vault-recovery-v1")`
encrypts the recovery slot, and
`recoveryAuthKey = HKDF-SHA256(code bytes, empty, "pockymoe-relay-recovery-v1")`
authenticates recovery sign-ins (the relay stores its scrypt hash). This code is
unrelated to the one-time recovery codes of the relay's two-step verification.

**Unlock window.** After the password or recovery code is entered, the browser
keeps the account private key and the VDK in memory for at most 10 minutes. They
are never persisted and are dropped on sign-out, when the page is closed or
reloaded, and when another account becomes active. Creating this browser's key,
binding a device (copying a setup or bind command, or remote protection),
certifying a device, confirming a replaced device identity, removing a browser or
device, changing the password and replacing the recovery code need an open
window; otherwise the browser asks for the password with the reason.

## Browsers

At sign-in, a browser without a browser key creates one and has the account key
sign a browser certificate for it. The browser registers the certificate with the
relay at every sign-in (`POST /relay/account/browsers`, labelled with the browser
and platform name). Certificates have no expiry. A browser stays authorized until
its key is deleted locally (sign-out) or the browser is removed. The relay keeps
at most 32 browser rows per account key, counting only rows that are not revoked;
a browser that already has a row registers again as usual. A new one beyond the
limit replaces the least recently seen row: browsers that lost their key (cleared
storage, a phone that keeps signing in again) leave rows behind, and dropping a row
revokes nothing; that browser can register again. Only if no row can go does it
answer 429 `browser_limit`. Signing out in the Web first deletes this browser's
own row (`DELETE /relay/account/browsers/{browserId}`), so sign-outs do not use up
the limit.

Registering binds the relay session to that browser and account key: the session
cannot register a different browser (403 `forbidden`), and removing the browser
(a signed revocation) ends every relay session bound to it.

On first contact with each protected device in a page session, a browser:

1. sends revocations (encrypted `PUT /api/transport/revocations`): an owner
   browser sends its account's full list to its own devices, a recipient browser
   sends its account's browser revocations to the shared devices it uses;
2. registers its certificate (encrypted `POST /api/transport/browser`). The device
   accepts a certificate that verifies with its pinned account key (role `owner`)
   or with the account key of a recipient in a live grant (role `grantee`), and is
   not revoked. A device keeps at most 32 browser registrations per recipient key;
   a new one beyond that replaces the least recently seen (dropping a row revokes
   nothing, and that browser can register again), so private tabs and cleared
   storage cannot lock a recipient out; a browser whose registration a device refused
   does not ask that device again for one minute, doubling to thirty;
3. signs every later encrypted request, including download chunks and
   service-worker requests, with `x-rcd-session: <browserId>` and
   `x-rcd-signature`. A WebSocket channel inherits the principal of the signed
   request that created it.

If a device answers 401 `browser_registration_required` (it forgot the browser,
or the signature failed), the browser registers again and retries once; the
device rejects before dispatch, so a mutation runs at most once. While this browser
still has its key, that and `device_strict_mode` never ask for the password, which
could not help: after the retry the request fails like any other, for example on
a shared device that keeps refusing this browser. The password prompt is only for
a browser without a key or one that was removed. On 401
`browser_revoked` in an encrypted device response the browser deletes its key and
shows "This browser was removed. Enter your password to reconnect."; entering the
password creates a new browser key. A plaintext `browser_revoked` (which the relay
could forge) never deletes the key.

Every unlock prompt states its cause under the description: no browser key in
this browser, a device that refused a request (device name, method, path and
code), a removed browser, or the action that needs the account key, with a button
that copies these details for support.

A signed-in session whose browser has no browser key shows **Unlock this browser**
on every account page until it has one: "Enter your password once to connect to
your devices. Your account key stays unlocked for sensitive changes for 10
minutes."

## Binding devices

Binding pins the account public key in the Supervisor database, stores a one-time
enrollment secret and turns on strict mode. Enrollment then installs a device
certificate signed by the account key.

The enrollment secret is 32 random bytes in base64url, created by the browser and
kept in its IndexedDB with the pending enrollment for 7 days.
[One-click protection](#protect-all-devices) sends it inside the encrypted bind
request. In a command it travels only in the environment variable
`POCKYMOE_ENROLL_SECRET` or on standard input (`pockymoe bind --enroll-stdin`;
`pockymoe setup` and the setup scripts accept the same option), never as a
process argument. The setup scripts read it, remove it from their environment
and hand it only to native setup; `pockymoe` removes it from its own environment before starting
anything, and the Supervisor service and its detached workers never inherit it.
Shell history still holds the copied command; its secret is useless once the
device has its certificate or the pending enrollment has expired. The Supervisor
database, which holds the secret until enrollment, is kept at mode 0600 on Unix.

The Web builds commands only from a device token matching
`^[A-Za-z0-9_-]{16,512}$` and from printable ASCII values ("Setup command
refused: the device token is invalid." or "…a value contains unsupported
characters.").

**New devices.** On the Devices page, **Add device** and the setup commands copied
from a device's menu include the binding. Creating a device and fetching its token
also need an authentication within the last 10 minutes. The commands are:

```sh
curl -fsSL 'https://app.example.com/setup.sh' | POCKYMOE_ENROLL_SECRET='<secret>' sh -s -- \
  --relay 'https://relay.example.com' --token 'rcd_…' --port 45679 \
  --web-origin 'https://app.example.com' --account-key '<B64 SPKI>'
```

```powershell
$env:POCKYMOE_ENROLL_SECRET = '<secret>'; try { & ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing 'https://app.example.com/setup.ps1').Content)) -Relay 'https://relay.example.com' -Token 'rcd_…' -Port 45680 -WebOrigin 'https://app.example.com' -AccountKey '<B64 SPKI>' } finally { Remove-Item Env:POCKYMOE_ENROLL_SECRET -ErrorAction SilentlyContinue }
```

Native setup (`pockymoe setup`, env `POCKYMOE_WEB_ORIGIN`, `POCKYMOE_ACCOUNT_KEY`,
`POCKYMOE_ENROLL_SECRET`) binds the database before it starts the Supervisor.
Run on a device that is already installed for the same relay and port, a setup
command with an account key binds the existing installation's database (after
authenticating to its Supervisor with the saved local credentials when it is
running) and keeps the saved registration; the token or setup code in the command
is not used. It refuses when the command's Web origin differs from the one saved
at installation, or when the installation saved none, as installations made
before Web origins were saved did ("The saved Web origin differs; update the
local configuration before resuming setup"). Protect such devices with the bind
command or [Protect all devices](#protect-all-devices).

**Existing devices.** An unbound device shows **Protect this device**. Its dialog
offers **Protect this device remotely** ([one-click protection](#protect-all-devices))
or **Show local bind command**, a command to run on the device:

```sh
POCKYMOE_ENROLL_SECRET='<secret>' pockymoe bind --account-key '<B64 SPKI>' --web-origin 'https://app.example.com'
```

The PowerShell form sets and removes the variable around `pockymoe bind` in the
same `try`/`finally` shape as the setup command.

### `pockymoe bind`

The command works offline: it opens the Supervisor database directly and needs
no running service. A running Supervisor applies the change at once.

| Flags | Effect |
| --- | --- |
| `--account-key KEY` with `POCKYMOE_ENROLL_SECRET` or `--enroll-stdin` | Bind and turn on strict mode. Bound to the same key: stores the new secret, unless the device already holds a valid certificate for its current identity, in which case enrollment stays complete; a stored certificate for another identity is deleted. Bound to another key: refuses ("use pockymoe bind --reset to replace it"). Also deletes the device's live links that have no encryption key ([public links](#public-links)). Standard input is read up to 1024 bytes. |
| `--web-origin URL` | With `--account-key`. Stored with the binding. HTTPS origin without path, query, fragment or credentials (HTTP only for `localhost`, `127.0.0.1`, `[::1]`). Defaults to env `POCKYMOE_WEB_ORIGIN`. |
| `--status` | Prints the device identity fingerprint (`SHA-256 <B64 of SHA-256 of the identity key>`), then `Web origin (installation hint): <origin>` when one is stored, a notice when the relay assigned a different device ID, and `Bound to account <fingerprint>. Strict mode: on.` or `Not bound. Strict mode: off.` |
| `--reset` | Asks for confirmation, then deletes the pinned key, enrollment secret, device certificate, stored Web origin, browser registrations and their preview leases, grants, peer registrations, port mappings created by browsers and the device-ID mismatch status, and turns strict mode off. Revocation tombstones, threads, local port mappings and their leases, the identity key and the relay device ID remain. Combine with `--account-key` to rebind to another key in one step. |
| `--yes` | With `--reset`, skips the prompt. |
| `--database PATH` | Supervisor database (env `POCKYMOE_DATABASE_PATH`, default `~/.remote-codex/relay-supervisor.sqlite`). |

The database must belong to a Supervisor version that supports binding ("Update
the Supervisor before binding this database").

A bound device refuses a relay greeting that assigns it a different device ID. It
retries with exponential backoff (1 second, doubling up to 5 minutes) and reports
`"relayStatus": {"code": "device_id_mismatch", "retryAt": <ms>}` on its local
`/healthz` (`relayStatus` is `null` otherwise). Restore the original relay
registration, or reset the binding before attaching the device to another relay
device.

### Protect all devices

**Protect all devices** on the Devices page binds the account's unprotected
devices without a command on each device; **Protect this device remotely** in a
device's **Protect this device** dialog does the same for one device. The dialog
first states the trade-off: "Remote protection trusts the relay at the moment of
binding. After binding, your account key protects device commands and traffic."
and "For the most secure option, run the local bind command on each device."

After one unlock ("Authorize binding a device to your account"), the browser goes
through the devices the account owns:

- devices removed from the account are skipped;
- devices the vault already records are never bound remotely. They show
  "Already bound; remote binding was skipped.", unless the device is online and
  the certificate it presents does not verify for the recorded identity: then the
  row shows the missing-certificate or changed-identity state, which
  [repair](#identity-changes-and-repair) handles;
- devices for which this browser holds a pending local-command enrollment are
  skipped ("Waiting for the local bind command. …"), so a secret meant for a local
  command never reaches the relay. A device uses one path at a time: while remote
  protection is in progress, the dialog refuses a local command ("Remote
  protection is already in progress for this device. …");
- every other device gets a pending enrollment marked for remote protection.

For each such device that is online, the browser reads its key descriptor:

1. A runtime that does not advertise the `remote-bind` capability, or has no
   encryption descriptor, shows "Update this device before protecting it
   remotely." with **Update and protect**. That button starts the device's update
   through the management API and the device restarts. Binding resumes when the
   device reconnects; a failed update shows "The update did not enable remote
   protection. …".
2. A device that reports a binding to another account key, presents a certificate
   of another key, or answers the bind with another key or device ID is reported
   as "This device is bound to another account key. Run pockymoe bind --reset
   locally, then retry or use the local bind command." That report reaches the
   browser through the relay before the device is protected, so first run
   `pockymoe bind --status` on the device; reset only a device that is bound to a
   key other than yours. A device already bound to this account key by another
   browser's command is left to that enrollment.
3. Otherwise the browser sends an encrypted `POST /api/transport/bind` with
   `{accountPublicKey, enrollSecret, webOrigin}`. The device does what
   `pockymoe bind` does (pin, enrollment secret, strict mode) and answers
   `{bound: true, accountKeyId, deviceId}`. The browser then completes
   [enrollment](#enrollment) as usual: MAC check, certificate, relay copy and the
   vault's bound devices. Signing the certificate asks for the password again if
   the unlock window has closed.

Offline devices stay queued (**Waiting for device**) and are bound when they
connect while this browser has a page open, within the pending enrollment's 7
days. Each device row shows its progress or error as a short label; opening it
explains the state in one sentence, offers **Update and protect** or **Retry**,
and links to the matching answer on the in-app help page (`/help/devices`). A
**Bound to another account** failure clears itself once the online device reports
that it is unbound again (after `pockymoe bind --reset`), and the row returns to
**Not protected**. The local command stays available as the most secure option.

**Trade-off.** Until it is bound, a device obeys the relay, and its key
descriptor carries no certificate the browser could check. The bind request is
encrypted to whatever key the relay presents, so a relay that is compromised at
that moment can bind the device to a key of its own or answer in the device's
place with an identity it controls and have that certified. A bound and certified
device is protected like any other. The local command avoids this window: the
secret reaches the device outside the relay, and the enrollment MAC proves that
the certified identity belongs to the device that received it.

The device accepts the bind only while it is not bound (409 `already_bound`
otherwise), only encrypted (403 `device_strict_mode` for plaintext, including
direct local requests) and only as a request the relay forwards for the device
owner (403 `owner_required` for a peer or a share recipient). An invalid key,
secret or Web origin, or a device that has not connected to the relay yet,
answers 400 `invalid_bind`. The secret is never logged. The key descriptor's
unsigned trust hint carries `capabilities: ["remote-bind"]` on runtimes with the
endpoint; it is a feature hint, never a security decision.

### Enrollment

Enrollment completes when the reader asks for it: the device's protection chip
on the devices page offers **Finish protection** while the device is online and
an enrollment from this browser is pending. Nothing runs it in the background: a
background loop that needed the account key interrupted work with password
prompts, so it was removed. Finishing may ask for the password once.

1. The browser fetches the device's key descriptor. If the descriptor already
   carries a certificate that verifies for this account, device and identity, and
   no identity replacement is involved, it continues with step 4. Otherwise it
   sends an encrypted, unsigned `GET /api/transport/enrollment`. The device answers
   `{deviceId, identityKey, accountKeyId, mac}` with
   `mac = HMAC-SHA256(enrollSecret, "pockymoe-acct-enroll-v1\n{deviceId}\n{identityKey}\n{accountKeyId}")`,
   or 404 `enrollment_missing` (no secret pending, or the device already holds a
   valid certificate for its identity, in which case it drops the secret).
2. The browser checks the MAC with its pending secret, the account key ID and that
   `identityKey` equals the descriptor's. If the vault records another identity key
   for the device, it asks **Confirm device reinstall** first (see
   [repair](#identity-changes-and-repair)). It then signs a device certificate
   (asking for the password if the window is closed) and sends it in a signed
   `PUT /api/transport/device-certificate`. A certificate older than the last one
   this browser verified for the device is refused ("Enrollment refused: this
   device certificate is older than the one already trusted. …").
3. The device verifies the certificate against its pinned key, its own device ID
   and identity key, refuses a revoked device ID, stores the certificate, deletes
   the secret and answers `{certificate}`.
4. The browser uploads the certificate to the relay
   (`PUT /relay/devices/{id}/certificate`), records `{deviceId, identityKey}` in the
   vault's bound devices, confirms the owner role with a signed request and deletes
   the pending enrollment.

### Device states

Each device row shows one label (a shield icon alone for **Protected**). Opening
it gives a one-sentence explanation, the available action and a link to the
matching answer on `/help/devices`.

| Label | Meaning |
| --- | --- |
| **Protected** | The device presented a certificate that verifies with this account's own key and answered a signed request in the `owner` role. A device shared with you is never labelled Protected. |
| **Not protected** | Not listed in the vault, no pending enrollment in this browser, and the device's connection trusts the relay. Offers **Protect this device**. |
| **Waiting for bind** | Not listed in the vault, and this browser holds a pending local-command enrollment for the device. Offers **Show local bind command**. |
| **Waiting for device**, **Update needed**, **Updating**, **Protecting** | Remote protection is queued for an offline device, needs a runtime update (**Update and protect**), is updating, or is binding and certifying. |
| **Bound to another account**, **Update failed**, **Protection failed** | Remote protection stopped: the device reports another account key, the update did not enable binding, or the attempt failed. Offers **Retry** or **Update and protect**. |
| **Needs repair** | The vault lists the device, but this page has no verified owner connection to it: the device is offline, presents no valid certificate, or presents a certificate for an identity other than the one the vault records. Also shown for a device removed from the account. The connection never falls back to relay trust. Offers **Repair device binding**. |
| **Identity changed** | Enrollment found an identity key other than the one the vault records; see [repair](#identity-changes-and-repair). |

No label is shown while the protection state is unknown, for example while a
device that is not in the vault is offline.

The device's key descriptor (`GET …/transport/key`) carries the device's own
`certificate` (or `null`) and an unsigned hint `trust: {bound, strict,
accountKeyId, capabilities}`. Browsers use the hint only to refuse a downgrade
and to offer features, never to grant trust, and verify the certificate the device
presents rather than the relay's copy. Devices in the user's own device list and devices with a pending enrollment
always take these owner checks, never the recipient checks of
[sharing](#sharing). **Manage device trust** (the encryption status icon) shows the
state, the bound account fingerprint and the browsers registered on the device.

If a pending enrollment expires or its browser data is lost, copy the setup or
bind command again in a browser of the account and run it on the device.

### Identity changes and repair

**Repair device binding** gives a fresh `pockymoe bind` command for a device in
the **Certificate missing** state. Running it on the device arms enrollment, and
the browser that copied it completes it:

- after `pockymoe bind --reset` (or a lost certificate) with the same identity
  file, enrollment installs a new certificate without further questions;
- after the identity file was lost or the device was reinstalled, the device has a
  new identity key. Once the enrollment MAC verifies, the browser shows
  **Confirm device reinstall** with the device name and the previous and new
  identity fingerprints (`SHA-256 …`), and asks the user to compare the new one
  with `pockymoe bind --status` or `pockymoe relay-fingerprint` run on the device.
  **Confirm replacement and unlock** signs the new certificate and records the new
  identity key in the vault.

Until that confirmation, a browser that finds a valid account certificate for an
identity other than the recorded one blocks the device: "This device has a
different identity. Unlock and explicitly confirm the reinstalled device before
connecting." A device removed from the account (device revocation) cannot be
repaired or enrolled again; reset it locally and set it up as a new device.

## Strict mode

A bound device is strict. Through the relay tunnel these need no browser
signature:

- `GET /healthz`;
- `GET` of a key descriptor (any path ending in `/transport/key`; a `challenge`
  must be 8 to 128 characters of `[A-Za-z0-9_-]`);
- `GET /api/publications/{token}` ([public links](#public-links));
- encrypted `POST /api/transport/browser`, `GET /api/transport/enrollment` and
  `PUT /api/transport/revocations` (refused in plaintext on every device);
- encrypted `POST /api/transport/bind`, which a bound device answers only with
  409 `already_bound` or 403 `owner_required`;
- encrypted `POST /api/peer/transport/certificate` from a relay-identified peer.

Everything else needs an encrypted request signed by a registered owner or
grantee browser, or a verified peer. WebSocket clients must use a secure channel
created by a signed request; others are closed. The device rechecks channels at
every relay heartbeat (3 seconds) and immediately after revocations and grant
changes, and closes those whose browser, grant or peer registration is no longer
valid. File saves are attributed to `owner:<browserId>` or `grantee:<userId>`,
never to the relay's `x-rcd-file-actor` header.

Owner-only paths: `/api/transport/trust`, `/api/transport/device-certificate`,
`/api/transport/grants`, `/api/ports`, `/api/port-mappings`, `/api/config`,
`/api/management`, `/api/settings`, `/api/thread-activity`. Direct local access (CLI, local mode, server
mode) is not subject to strict mode; the trust and bootstrap endpoints answer
only encrypted tunnel requests.

| Code | Status | Cause |
| --- | --- | --- |
| `device_strict_mode` | 403 | Plaintext request to a strict device outside the exceptions; `/api/peer/*` without a relay peer identity; a plaintext bootstrap or trust endpoint request on any device |
| `browser_registration_required` | 401 | Encrypted request without `x-rcd-session`, from an unregistered browser, with an invalid signature or a registration that is no longer valid; trust endpoints on an unbound device |
| `browser_revoked` | 401 | The browser is revoked by the pinned key or by the key of a recipient in a live grant (request or registration) |
| `account_mismatch` | 403 | The certificate's account key is neither the pinned key nor a recipient key in a live grant, or the browser ID is registered for another account or user (an owner certificate replaces a recipient registration with the same browser ID) |
| `browser_limit` | 429 | No longer sent by the device: a new recipient browser beyond 32 replaces the least recently seen one |
| `revocation_limit` | 429 | A revocation push that would store more than 1024 tombstones for one recipient key; the whole push is refused |
| `owner_required` | 403 | Recipient request to an owner-only path, a preview lease for a browser that is not a registered owner, or a bind request from a peer or recipient |
| `already_bound` | 409 | Bind request to a bound device |
| `invalid_bind` | 400 | Bind request with an invalid key, secret, Web origin or body, or to a device that has not connected to the relay yet |
| `grant_denied` | 403 | Recipient request outside its grants |
| `invalid_grant` | 400 | Malformed grant ([sharing](#sharing)) |
| `grantee_is_owner` | 400 | The recipient key is the device's own account key |
| `grant_limit` | 409 | The device already has 256 grants |
| `enrollment_missing` | 404 | No enrollment secret is pending |
| `mapping_not_found` | 404 | Preview lease for a mapping the device does not have |
| `invalid_certificate` | 400 | Any other failed verification: certificate for another device, identity or key, revoked device, more than 4096 revocations in one push, unbound device or device that has not connected to the relay yet |
| `peer_certificate_required` | 401 | Strict target without a valid peer registration and signature |
| `peer_target_unverified` | — | Caller-side error: a bound caller cannot verify the target's certificate |
| `peer_source_shared` | 403 | Peer request from a device shared with other people who can control its agents; the relay refuses it, and a target refuses `sharedControl: true` in the relay's `peer` identity ([agent isolation](agent-isolation.md#devices-shared-with-other-people)) |

Owner endpoint `GET /api/transport/trust` returns `{deviceId, bound, strict,
accountKeyId, certificate, browsers: [{browserId, role, userId, registeredAt,
lastSeenAt, current}], revocations}` (`revocations` is a count).

## Revocations

Removing a browser (**Account settings → Account key and browsers → Remove
browser**) or a device (**Devices → Remove device**) signs a revocation
`{kind: "browser" | "device", subjectId}` with the account key and posts it to the
relay. The relay checks it against the vault's account key, stores it (at most
4096 per account key; a new one beyond that answers 429 `revocation_limit`, a
repeated one is accepted) and, for a browser, deletes its browser row and ends the
relay sessions bound to that browser. Revocations are permanent. They reach
devices by three paths:

- the relay sends `{"type": "relay.revocations", "revocations": [...]}` to every
  connected device the account owns when one is posted, and to each such device
  right after it connects. The list holds only revocations signed by the account's
  current key;
- an owner browser sends its account's list to each of its own devices on first
  contact in a page session and after each removal;
- a recipient browser sends its account's browser revocations to the shared
  devices it uses, on first contact in a page session and after each removal (the
  relay forwards this encrypted push for any live share or grant).

Devices and browsers verify each entry on its own and skip entries they cannot
verify or that belong to another key; one bad entry never rejects the rest. A
device accepts entries signed by its pinned key, and browser revocations signed by
the account key of a recipient in a live grant, which apply only to browsers
registered under that recipient key. Accepted entries are kept as tombstones,
also across `pockymoe bind --reset`, so a removed browser cannot register again
after the device is bound to the same key. A device keeps at most 1024 tombstones
per recipient key; a push that would add more is refused as a whole (429
`revocation_limit`).

A revoked browser loses its registration, its preview leases, every open channel
and its relay sessions; for an owner browser, also the port mappings it created. A
revoked device is refused as a peer by the account's other devices and by owner
browsers ("This device was removed from the account."). **Remove device** then
deletes the relay registration, so the device can no longer connect, and the
browser forgets its stored certificate; the device keeps its local binding until
`pockymoe bind --reset`.

Removing one of your browsers cuts it off from your own bound devices through the
relay frame. On devices that other accounts share with you, it takes effect when
one of your browsers delivers the revocation: at once from the browser that
removed it, for shared devices that page has open, otherwise at its next contact.
Until then the owner can revoke the share. Removing a browser does not change the
password: anyone who knows it can sign in again and create a new browser key.

After **Start over**, the relay sends only revocations signed by the new key.
Devices still bound to the old key keep the tombstones they already hold but can
receive no new ones, because the old key is gone: browsers certified by the old
key stay registered there until the device is reset and bound to the new key,
after which their certificates no longer verify.

## Account flows

| Flow | Behavior |
| --- | --- |
| Register | The browser generates the user ID (canonical UUID v4), account key, VDK, salt and recovery code, shows the code for confirmation, then sends `POST /relay/auth/register` with `authKey`, `recoveryAuthKey` and the vault. With registration approval, the vault waits with the request and is stored on approval; a second registration for a name with a pending request answers 409 `conflict`. A user ID collision answers 409 `user_id_taken`; registering again generates new keys. |
| Sign in | One form: fetch the KDF parameters for the sign-in name, derive, sign in with `authKey`, complete the second factor, fetch and open the vault, register the browser. The unlock window opens without a separate step. |
| Security upgrade | For an account without a vault. After a sign-in fails with "Incorrect login or password.", the sign-in page offers **Continue with this password**, which posts `{login, password, upgrade: true}`; the relay accepts it only for password accounts without a vault that have not used an admin reset password. Browsers never offer it for names they have opened a vault for; a browser that has opened any vault warns and requires the box "I am upgrading a different account that has never been secured." Web builds with `VITE_POCKYMOE_LEGACY_UPGRADE=0` do not offer it. A stored SHA-256 password hash is upgraded to scrypt at this sign-in. The session stays restricted until **Secure your account** creates the vault, which needs this sign-in to be less than 10 minutes old. A password of at least 12 characters is reused right away; a shorter one asks for a new password. The vault is stored before the recovery code is shown, and the code must be confirmed by typing its last 4 characters. Creating the vault deletes the relay's login password hash, signs out the account's other sessions and forgets its trusted (two-step) browsers. A setup session whose sign-in is older than 10 minutes, such as one from before the update, shows **Please sign in again** instead of the setup form. |
| OAuth account | No login password. At the first sign-in the user chooses an **unlock password**, which only encrypts the vault; no `authKey` is sent. A browser without a browser key asks for it after OAuth sign-in. Every vault write (including recording a bound device) and starting over need an OAuth sign-in or second factor within 10 minutes (otherwise 403 `reauthentication_required`). An account with both a password and an OAuth identity is a password account. |
| Forgot password | **Forgot password?** on the sign-in page, **Recovery code** tab: the recovery code and a new password. The relay checks `recoveryAuthKey` (10 attempts per 5 minutes per name) and always asks for the configured second factor, even on trusted browsers. The browser opens the recovery slot, writes a new password slot and `authKey`, and registers itself. |
| Admin reset | **Forgot password?**, **Admin password** tab, with a temporary password from the relay administrator (one use; the configured second factor is asked even on trusted browsers). For an account with a vault, **Set a new password** then asks for the recovery code and a new password; **I don't have my recovery code** leads to **Start over**. For an account without a vault (including OAuth-only accounts), the sign-in opens a setup session that leads to **Secure your account** (or the unlock password); this works in every Web build. |
| Change password | Account settings, unlock window. Proves the current `authKey`, or the recovery code when the window was opened with it. New random salt. Signs out other sessions and forgets trusted browsers; browser keys and device registrations stay valid. |
| Regenerate recovery code | Shows a new code once; the old one stops working. Signs out other sessions and forgets trusted browsers. |
| Start over | In Account settings; outside a recovery or reset session it first asks for the current password ("Confirm your current password to start over."). Deletes the vault, the account's browser certificates and the relay copies of its device certificates, and signs out other sessions. The session becomes a setup session, and the browser asks for a new password and shows a new recovery code to create a new account key. Until the new vault exists, the old recovery code still signs in (into a setup session); the password does not work in the Web, because the relay then answers fake KDF parameters. Every bound device refuses the new key until `pockymoe bind --reset` on that device, which then shows **Protect this device**. The account's safety code changes, and a recipient browser that pinned the old owner key blocks the owner's devices until it uses **Forget pinned owner account**. |
| Sign out | Offered on every account screen after sign-in (setup, unlock, reset) and in the unlock dialog. Deletes this browser's key. Its certificate stays listed until removed; the cached account key ID and vault revision stay to detect substitution at the next sign-in. |

The relay administrator signs in at `/relay-admin` with the configured password,
holds no account key and cannot use the portal.

Every admin reset sets a one-time temporary password. For an account with a vault
it also invalidates the old `authKey`; the vault is unchanged, so only the
recovery code (or starting over) restores device access. For an account without a
vault, the temporary-password sign-in yields a setup session in which the user
secures the account; afterwards neither the temporary password nor the security
upgrade sign-in works for it. Nobody, including the relay operator, can recover an
account key without the password or recovery code.

## Migration

Updating a relay converts nothing by itself: existing accounts and devices keep
working until their owners secure and bind them.

**What is kept.** Accounts and sign-in names, two-step factors, trusted
browsers, devices and their tokens, threads, shares and grants, and public links.
Unsecured accounts keep their password hash until their vault exists. Sessions
from before the upgrade are restricted like a setup session, so each user signs
in once more. The administrator account is unchanged. Unbound devices keep
working as **Not protected**.

**Each user, once.**

1. Sign in with the current password. It fails once with "Incorrect login or
   password."; choose **Continue with this password** (the only time the relay
   receives it) and complete the second factor. **Secure your account** then
   stores the vault: a password of at least 12 characters is kept, a shorter one
   must be replaced. Save the recovery code. Users who sign in with Google or
   GitHub sign in as usual and choose an unlock password.
2. Other browsers then sign in with the ordinary form.
3. Recipients of shares secure their accounts before a protected device can honor
   those shares.

**Binding existing devices.**

1. On the Devices page choose **Protect all devices**
   ([one-click protection](#protect-all-devices)). One unlock binds every online
   unprotected device and queues the offline ones; a device whose runtime cannot
   bind remotely offers **Update and protect**, which updates it first. For the
   most secure option, choose **Protect this device** → **Show local bind
   command** instead and run the command on the device; it needs a runtime that
   supports binding (update it from device Settings first). Rerunning the
   device's setup command also binds the existing installation, but only when the
   Web origin saved at installation matches ([new devices](#binding-devices)).
2. Keep the browser open, or come back to it within 7 days, until the device
   shows **Protected**.
3. Afterwards the device honors only signed grants: in each share's details,
   **Not active on device** offers **Review and confirm**. Port mappings created
   through the relay before binding must be enabled again, and the device deletes
   live public links created before link encryption (the relay keeps their last
   snapshot, marked stale).

**Operators.** Update the relay and the Web UI together. A relay that has served
the Web UI itself moves it to a static origin as described
[below](#moving-to-a-separate-web-origin). When every account has a vault, build
the Web with `VITE_POCKYMOE_LEGACY_UPGRADE=0` to remove the security upgrade
(for the public site, add it to the build environment in `pages-deploy.yml`).
The relay still accepts the request, but no browser running that build sends a
password. An account that still has no vault then signs in only after an
[admin reset](#account-flows): **Forgot password?** → **Admin password** leads
to **Secure your account**. After **Start over**, the recovery code still
signs in until the new vault exists.

### Moving to a separate Web origin

This is the procedure for a relay that serves the Web UI today and moves it to a
static origin with an API-only relay
([self-hosting](self-host-relay.md#separate-web-origin)). The public service
moved this way: the Web UI from `https://remote.lnz-study.com` to GitHub Pages at
`https://app.lnz-study.com`, while the relay kept `https://remote.lnz-study.com`.

**Choosing hostnames.** Keeping the relay's hostname and giving the Web UI a new
host on the same site (registrable domain) leaves everything that points at the
relay unchanged: the relay URL devices saved at setup and their
`/supervisor/tunnel` connection, the OAuth callback URLs
(`…/relay/auth/oauth/{google,github}/callback` stay on
`POCKYMOE_PUBLIC_BASE_URL`), the relay's session and trusted-browser cookies and
the port preview hosts (`p-<id>.<base>` keep routing to the relay; an exact DNS
record for the Web host takes precedence over a preview wildcard). What moves is
the Web origin. Browsers keep browser keys, verified vault state, pending
enrollments and pinned owner keys per origin, so these start empty on the new
one, and passkeys registered as a second factor are bound to the old host. Moving the API to a new host instead keeps the
Web origin and passkeys, but needs new OAuth callback URLs and the old host's
`/supervisor/tunnel` routed to the relay
([existing devices](self-host-relay.md#existing-devices)).

**Trial instance.** Try the move first on a second relay with a copy of the
data: a consistent SQLite backup of `relay-store.sqlite` together with the same
`session-secret` (it decrypts stored two-step factors and device credentials),
its own hostname and port, `POCKYMOE_PUBLIC_BASE_URL` and `POCKYMOE_WEB_ORIGIN`
set to trial hosts on one site, and a static build for that pair. Check sign-in,
two-step verification, OAuth (register the trial callback URL with the
provider), securing an account and the Devices page; try binding with a test
device set up against the trial relay. Use Web and relay hosts that production
will never use: browsers keep the vault state they verified per Web origin and
would later refuse a different production vault ("The relay served a different
account key"). Nothing done on the trial carries over. The copy holds every
account's credentials; protect it like the production database and delete it
afterwards.

**Cutover in place.**

1. Publish the static Web UI on the new origin
   ([building it](self-host-relay.md#building-the-static-web-ui)). For GitHub
   Pages, set up the custom domain and its DNS record first.
2. Stop the relay and back up its data directory consistently
   (`relay-store.sqlite`, `session-secret`) and its environment file.
   `relay-deploy.yml` does this for the public relay before replacing the binary.
3. Add `POCKYMOE_WEB_ORIGIN` (keep `POCKYMOE_PUBLIC_BASE_URL`), install the new
   relay and start it.
4. Check that `/healthz` answers, that `/` on the relay redirects (307) to the
   Web origin, that sign-in works on the Web origin and that devices reconnect.

**Rollback.**

- Removing `POCKYMOE_WEB_ORIGIN` again (with `POCKYMOE_RELAY_WEB_DIST_DIR` set)
  returns to single-origin mode without losing data. Browsers then use the relay
  origin's storage again; those whose old browser was removed after the move sign
  in again there, and browser keys created on the static origin stay registered
  until removed.
- Restoring the database backup discards everything written since: vaults
  created or changed, browser registrations, revocations and device
  certificates. Browsers that opened a newer vault refuse the restored one ("The
  relay served an older account vault. Connection blocked.") or its absence. An account
  secured after the backup has lost its account key with its vault; devices bound
  to that key need `pockymoe bind --reset` before they can be bound again.
- A relay binary from before account keys cannot sign in accounts that have
  created a vault, because their old password hash was deleted. Run such a binary
  only on the backup taken before the upgrade.

**What each user does once.**

1. Open the Web origin (the relay's address redirects there) and sign in again;
   the browser gets a new browser key. A passkey registered on the old host does
   not work on the new one: use another second factor or a two-step recovery code
   and register the passkey again.
2. Secure the account there if that has not happened yet
   ([each user, once](#migration)).
3. In **Account settings → Account key and browsers**, remove the browsers created
   before the move (the current one is marked). Their keys stay in the old
   origin's storage, which is now the relay's own host, so a compromised relay
   could serve a page there that uses them.
4. Bind devices that are still unprotected with **Protect all devices** or the
   local command. A command copied on the old origin cannot complete: its pending
   enrollment stays in the old origin's storage, so copy it again. Devices bound
   before the move keep their binding.

**Old Web origin.** Browsers that had the Web UI installed from the relay are
cleaned up by the relay ([old Web origin](#old-web-origin)).

## Sharing

A bound device enforces shares (one thread) and grants (one workspace or the
whole device) from its own grant list, in addition to the relay. Only owner
browsers can read or change it; each grant mirrors one relay row and names the
recipient's account key ([grant format](#records)). The device recomputes a
recipient's access on every request with the relay's rules (`crates/protocol`
access: owner, then a share with thread lineage up to three levels, then grants
merged by highest scope and access) and filters recipient WebSocket events and
messages itself. Grants are created only while the device is online.

| Scope | Conversations | Files | New conversations |
| --- | --- | --- | --- |
| Thread (share) | The thread and the agent threads it started, three levels deep | Its workspace, with read or write file access | None |
| Workspace (grant) | Every thread in the workspace, and the agent threads they started elsewhere, three levels deep | That workspace only | Only there, with control and **Allow new threads** |
| Device (grant) | Every thread | Every workspace; with write, also files a conversation links to outside them | Anywhere, with control and **Allow new threads** |

**Workspace shares.** The thread page's share dialog offers this thread, this
workspace or the whole device. A workspace share is a relay grant
(`scope: "workspace"`, `workspaceId`, `workspaceLabel`) signed for a bound device
like a device grant. Its recipient lists the conversations with
`GET /api/workspaces/{id}/threads` and starts one with
`POST /api/workspaces/{id}/threads`; the device refuses a body naming another
workspace. `GET /api/threads`, `GET /api/workspaces` and `POST /api/threads/start`
stay whole-device routes. The relay learns each thread's workspace from the
device's heartbeat (`threadLineage[].workspaceId`) and a bound device reads its own
database, so a thread the device has not reported is outside every workspace share.
The recipient finds the workspace as a row on the device's card under **Devices
with me**; the device's Workspaces page lists only the workspaces shared with them,
without the owner's actions. Runtime metadata (`/api/agent-runtimes…`), which names
no thread or workspace, is readable with any live share or grant on the device, so
a workspace share can choose a harness and model for a new thread. Devices on
0.13.36 or earlier report no workspaces and have no workspace thread routes: there
a workspace share reaches its files only.

**No terminal for recipients.** No share or grant, read or control, reaches a
terminal: thread shells (`/api/threads/{id}/shell`, the relay's
`/transport/shell-scope` probe), `/api/shells/*`, one-off commands
(`/api/threads/{id}/commands*`), the terminal plugin's routes, and the WebSocket
`shell.*` messages and shell output (`access::terminal_route` and
`client_message_allowed`, enforced by the relay and by a bound device). A device
also refuses a script automation (`runScript`) from a recipient. The relay reads
the cleartext message type of every forwarded WebSocket message, so recipients of
older devices lose `shell.*` too once the relay is updated. The Web hides the
terminal panel, its buttons and its plugin switch from recipients. This is a fixed
rule, not a share option; a whole-device share that used the terminal before loses
it.

**Files stay in the workspace.** Thread and workspace shares reach files only
through their workspace's `/api/workspaces/{id}/files/*` routes, where every path
resolves `..` and symbolic links inside the workspace root; prompt attachments are
written inside it even when `.temp` links elsewhere. The Web's file tree for them
starts at the workspace root and has no way above it.

Creating or editing shared access for a protected device:

1. The relay row is created or updated.
2. The browser builds the grant only from the choices in the dialog, accepting
   only the new row ID and recipient user ID from the relay, and refuses when the
   relay row differs ("No device grant was signed"). An edit must carry every
   permission and target from the dialog.
3. It fetches the recipient's account public key
   (`GET /relay/{shares|grants}/{id}/recipient-account`); without a vault this
   fails with "The recipient must secure their account before a protected device
   can honor this share."
4. **Review device grant** describes the grant that will be signed in words:
   recipient (verified or not), device, scope, conversation and file access, new
   conversations, expiry and safety code, and for an edit each changed field as
   before → after. The exact signed content, with the current device grant next to
   the new one, is folded under technical details. Every value wraps; the dialog
   never scrolls sideways. An unverified recipient needs an explicit
   acknowledgement, and **Sign and share** is the only button that signs.
5. The grant is sent in a signed `PUT /api/transport/grants/{grantId}`.

If any step after the first fails, the relay change is rolled back.

Files a conversation links to outside its workspace open as read-only previews
through `/api/threads/{id}/linked-files/{stat|preview|raw}`. Besides the owner,
only a whole-device grant with workspace write access reaches them
(`access::linked_files_allowed`, enforced by the relay and by a bound device);
narrower grants see those paths as plain text.

Validation: a recipient key must hash to its ID and must not be the device's own
key (`grantee_is_owner`); one recipient key maps to one user and public key across
grants; device-scope grants need `workspaceScope: "all"`; workspace-scope grants
need a `workspaceId`; shares are thread-scoped; at most 256 grants (`grant_limit`).

**Safety code.** The first 12 characters of the Crockford base32 encoding of
`SHA256("pockymoe-acct-safety-v1\n" + accountKeyId)`, in groups of four. Each user
sees their own in Account settings; the owner sees the recipient's per share,
marked **Verified** after **I compared the safety code** (stored in that browser
for that recipient user and key).

**Reconciliation.** Share details compare relay rows with device grants: **Not
active on device** (relay row without grant) offers **Review and confirm**;
**Device permissions differ from this share** and **Recipient account key has
changed** offer **Update**; grants without a relay row are listed as **Still
active on device** with **Revoke**. Every signing action shows the review first.

**Revoking** deletes the grant on every device in the vault's bound list and every
device whose certificate this browser has verified, except devices removed from
the account, then deletes the relay row. It reads the grants of all those devices
first and fails before deleting anything while one of them is unreachable.
Deleting the last grant for a recipient key drops that recipient's browser
registrations; a later grant lets the same, still valid browser certificates
register again.

**Recipients** register their own browser certificates (role `grantee`). They
verify the device certificate with the owner's account key from
`GET /relay/devices/{id}/owner-account`, pinned on first use and labelled so. An
invite link pins it beforehand:

```text
{webOrigin}/shared#v=2&device={deviceId}&owner={ownerAccountKeyId}[&thread={threadId}|&workspace={workspaceId}]
```

A thread share's invite opens the thread, a workspace share's invite opens that
workspace's threads, and a whole-device invite opens the device's workspaces.

After the recipient confirms the device and owner key shown, the browser pins that
key; a different key from the relay, or a certificate that does not verify with
it, blocks the connection. An invite whose owner key differs from a key already
pinned for that device is refused; **Forget pinned owner account** in the device's
encryption status clears the pin. **Copy invite link** is offered only after the
owner's browser verified the device certificate.

**Who sent a prompt.** A prompt from anyone but the device owner carries its
sender (`sender: {userId, name}` on the user message item and on the queued prompt),
shown as a small initial on the bubble that opens a card with the account name and
ID. A bound device takes it from the signed grantee browser and the name from that
recipient's newest grant (`grantee.label`); relay headers cannot change it. An
unbound device takes the relay's `x-rcd-file-actor`, `x-rcd-actor-owner` and
`x-rcd-actor-name`; a relay that does not send `x-rcd-actor-owner` leaves prompts
unmarked. The owner's own prompts, and prompts from agents, automations and other
devices, carry no sender.

## Peers

Devices bound to the same account key authenticate each other
([cross-device peers](cross-device-peer.zh.md)):

- **Caller checks target.** The target's peer key descriptor carries its device
  certificate. A bound caller fetches a fresh descriptor for every request and
  requires a certificate that verifies with its own pinned key for the target's
  device ID and identity key and is not revoked; otherwise the call fails with
  `peer_target_unverified`. A bound caller needs its own certificate (completed
  enrollment) to call any peer.
- **Target checks caller.** A strict target accepts a peer request only when
  `x-rcd-peer-device` equals the relay-asserted caller, a registration exists for
  that device under the pinned key, the caller is not revoked, and
  `x-rcd-peer-signature` verifies with the registered identity key. Otherwise it
  answers 401 `peer_certificate_required`; a bound caller then registers its
  certificate (`POST /api/peer/transport/certificate`) and retries once.

An unbound caller or target keeps the trust-on-first-use behavior and cannot talk
to a bound peer. `pockymoe bind --reset` deletes peer registrations.

Whether a device is shared with people who can control its agents comes from the
relay, which adds `sharedControl` to the `peer` identity it forwards and refuses
such a device as a caller (`peer_source_shared`). Certificates cannot prove this:
a relay that hides a share lets the shared device reach your other devices, but
it still cannot command any device itself.

## Public links

Each public link is encrypted with a random 256-bit AES-GCM key (AAD
`pockymoe-public-link-v1`) that exists only in the link fragment
(`{webOrigin}/s/{id}#k={key}`) and in the creating browser's storage. The relay
stores only `{"version": 2, "iv", "ciphertext"}`; only that browser can copy the
full link later.

- Snapshot links: the browser encrypts the selected turns.
- Live links: the browser sends the key to the device in an encrypted owner
  request (`POST /api/threads/{id}/publications`). The device returns ciphertext
  from `GET /api/publications/{token}`, which stays public in strict mode. When a
  link is opened, the relay refreshes its cached ciphertext at most every 5
  seconds, marks it stale when the device does not answer and never replaces an
  encrypted cache with plaintext.

Revoking deletes the relay link and then sends an encrypted owner
`DELETE /api/publications/{token}`; a strict device refuses the relay's own
unsigned delete, so revoke while the device is online. Links from before
encryption (version 1) stay plaintext. A bound device never serves a live link
without an encryption key, and binding deletes such links from the device; the
relay then keeps serving its last cached snapshot, marked stale, until the link is
revoked.

## Port previews

In strict mode a device reports and serves only local mappings (created through
its local API, such as `pockymoe preview create`) and mappings created by an
owner browser while that browser's registration exists. It also opens a preview
connection only under a live lease for that mapping:

- an owner browser obtains one with a signed
  `POST /api/port-mappings/{id}/lease` → `{leaseId, expiresAt}` before **Open**
  in Port mappings. A lease lasts 8 hours; asking again from the same browser for
  the same mapping renews the same lease ID. It ends early when that browser is
  removed or the mapping is stopped;
- `pockymoe preview create` creates a local lease with the mapping (creating the
  same port again renews it). Its `openUrl` carries
  `lease=<leaseId>&leaseExpiresAt=<ms>`. Local leases survive browser removal and
  `pockymoe bind --reset`;
- the signed DSH console request returns a lease for the console mapping.

Lease IDs are 43 base64url characters. The Web uses a lease it received from a
CLI link or the DSH console as long as its `leaseExpiresAt` has not passed (or a
link has none), and asks the device for a new one otherwise.

The open request carries the lease (`POST /relay/devices/{id}/port-mappings/{mappingId}/open?lease=<leaseId>`),
and the relay passes it as `leaseId` in every `preview.open` frame for that preview
session. The relay refuses a malformed lease (400 `invalid_lease`), and without a
lease it refuses with 403 `preview_lease_required` unless the device reports that
it is not strict. It removes `lease`, `leaseExpiresAt` and its launch ticket from
URLs before they reach the previewed application. The device ignores a
`preview.open` without a live lease and closes open connections when their lease
expires or ends. Recipients get `owner_required` when they ask for a lease.

A mapping that an agent created from a thread (`threadId` in its heartbeat
entry) also opens for that thread's recipients, with the lease in the CLI link.
The device checks only that the lease is live, not which account uses it, so the
relay's shares and grants alone decide this, as they decide who may use any
lease the relay forwards. Details:
[port previews](port-previews.md#who-can-open-a-mapping).

## Web origin

The Web code holds passwords and keys, so the public service serves it from a
static origin and runs the relay API-only
([self-hosting](self-host-relay.md#separate-web-origin)).

### Static hosting

`pnpm --filter @pockymoe/supervisor-web build:static` (`vite build --mode
static`) produces a site that needs no server logic:

- The relay origin is compiled in (`VITE_POCKYMOE_RELAY_ORIGIN`), so the page
  needs no inline script; every script is external and same-origin.
- `index.html` starts with the Content Security Policy in a
  `<meta http-equiv="Content-Security-Policy">` tag, ahead of every script and
  link, and a `<meta name="referrer" content="no-referrer">` tag. The meta policy
  has every directive of the header version except `frame-ancestors`, which a
  meta policy cannot carry: `script-src 'self' 'wasm-unsafe-eval'`, `connect-src`
  limited to the Web origin and the relay's `https:` and `wss:` origins,
  `object-src 'none'`, `base-uri 'none'` and `form-action` limited to the Web
  origin and the relay.
- The app refuses to start in a frame. When it is not the top window, it empties
  the page, tries to navigate the top window to itself and stops ("The account
  Web app requires a top-level window"). This stands in for `frame-ancestors
  'none'` and `X-Frame-Options: DENY`.
- The service worker is `/relayServiceWorker.js` at the site root, registered
  with scope `/`, so it needs no `Service-Worker-Allowed` header.
- `404.html` is a copy of `index.html`. GitHub Pages serves it for every unknown
  path, which gives the single-page app its routes (with status 404). `CNAME`
  holds the custom domain (`POCKYMOE_WEB_DOMAIN`).
- `_headers` and `_redirects` carry the full policy as response headers and the
  fallback rewrite for hosts that apply them.
- Each deployment replaces the hashed chunks. A page opened before it reloads
  once when it next fails to load a lazy chunk (for example the Explorer), and any
  render error shows a reload message instead of an empty page.

`scripts/check-static-web.mjs` checks the output: the meta policy equals the
`_headers` policy without `frame-ancestors` and allows the configured relay, the
meta tags precede scripts and links, scripts are external, `404.html` equals
`index.html`, `CNAME` matches, the worker is at the root and the bundle contains
the frame check.

GitHub Pages cannot set custom response headers, so the public site goes without
those in `_headers`:

- `frame-ancestors` and `X-Frame-Options`; the frame check replaces them once the
  app's script runs;
- `X-Content-Type-Options: nosniff`;
- a policy for the service worker: a meta policy applies only to the document, so
  `relayServiceWorker.js` runs without a Content Security Policy;
- `Cache-Control: no-cache` for `index.html` and the service worker; Pages applies
  its own caching, so a new deploy can take a few minutes to reach browsers.

The referrer meta tag covers the `Referrer-Policy` header for the page.

**Deployment.** `.github/workflows/pages-deploy.yml` builds and deploys the
public site on every push to `main` that changes the Web, its packages, the setup
scripts, `thread-ui.sha` or the workflow itself, and on manual dispatch from
`main`. It reads the full shared UI commit from the tracked file `thread-ui.sha`,
checks out `dufangshi/pockymoe-thread-ui-rust` at exactly that commit and refuses
one that is not on that repository's `main`, builds with
`VITE_POCKYMOE_RELAY_ORIGIN` from the repository variable `POCKYMOE_RELAY_ORIGIN`
and `POCKYMOE_WEB_DOMAIN=app.lnz-study.com`, runs the check script and deploys
with `actions/deploy-pages`. Before deploying, `scripts/carry-web-assets.mjs` copies the
hashed assets the live site serves (and those it carried itself) into the new build
for 72 hours after they stopped being current, up to 300 MB, and records them in
`asset-history.json`. A page opened before a deployment then still loads its lazy
chunks (Files, the editor) instead of reloading; past that window it reloads once
(`installStaleBuildReload`). The carried assets come only from this site's own
earlier deployments. Whoever can push to `main`, change that variable or
publish to the shared UI's `main` decides the code that receives users' passwords.

The relay at `https://remote.lnz-study.com` runs API-only: its environment file
sets `POCKYMOE_WEB_ORIGIN=https://app.lnz-study.com` (the runtime also reads the
old name `REMOTE_CODEX_WEB_ORIGIN`, but the deploy preflight checks the new one).
`relay-deploy.yml`, dispatched from `main`, builds and replaces only the relay and
then checks that `/` redirects to the Web origin.

### Old Web origin

In API-only mode the relay answers the service worker script URLs of the Web UI it
used to serve (`/assets/relayServiceWorker-<hash>.js`, `/service-worker.js` and
`/relayServiceWorker.js`, `GET` and `HEAD`) with a small worker instead of a
redirect: it activates at once, unregisters itself and reloads the open pages,
which then follow the redirect to the Web origin. The response is `no-store`; for
the `/assets/` URL it also carries `Service-Worker-Allowed: /`, because the old
worker was registered there with scope `/` and the browser's update check needs
the same maximum scope. The relay serves no other script.

## Wire reference

`B64` is base64url without padding; times are milliseconds since the epoch;
multi-line inputs are joined with `\n`. Signatures are ECDSA P-256 with SHA-256,
encoded as raw 64-byte `r||s`. Public keys are B64 SPKI DER unless noted.

### Records

| Name | Shape |
| --- | --- |
| Browser certificate | `{version: 1, accountKeyId, browserId, publicKey, createdAt, signature}`; `browserId` must equal `B64(SHA256(publicKey))` |
| Device certificate | `{version: 1, accountKeyId, deviceId, identityKey, createdAt, signature}` |
| Revocation | `{version: 1, accountKeyId, kind: "browser" \| "device", subjectId, revokedAt, signature}` |
| Grant | `{version: 1, grantId (B64 of 16 bytes), relayRef: {kind: "share" \| "grant", id}, grantee: {userId, label, accountKeyId, accountPublicKey}, scope: "thread" \| "workspace" \| "device", threadId, workspaceId, workspaceScope: "all" \| "selected", workspaceIds, threadAccess: "read" \| "control", workspaceAccess: "none" \| "read" \| "write", canCreateThreads, expiresAt}` |

### Signing inputs

| Use | Key | Input |
| --- | --- | --- |
| Browser certificate | Account | `pockymoe-acct-browser-v1\n{accountKeyId}\n{browserId}\n{publicKey}\n{createdAt}` |
| Device certificate | Account | `pockymoe-acct-device-v1\n{accountKeyId}\n{deviceId}\n{identityKey}\n{createdAt}` |
| Revocation | Account | `pockymoe-acct-revoke-v1\n{accountKeyId}\n{kind}\n{subjectId}\n{revokedAt}` |
| Enrollment MAC | HMAC-SHA256, enrollment secret | `pockymoe-acct-enroll-v1\n{deviceId}\n{identityKey}\n{accountKeyId}` |
| Request | Browser | `pockymoe-request-v1\n{aad}\n{B64(SHA256(sealed))}` |
| Peer request | Caller's device identity | `pockymoe-peer-request-v1\n{callerDeviceId}\n{aad}\n{B64(SHA256(sealed))}` |

`aad` is the encrypted request's `rcd-http-v1\n{keyId}\n{requestId}\n{method}\n{path}\n{resource}`;
`sealed` is the exact HPKE ciphertext (the body, or the decoded `x-rcd-sealed`).

### Headers

| Header | Sent on | Value |
| --- | --- | --- |
| `x-rcd-session` | Browser → device API | `browserId` |
| `x-rcd-signature` | Browser → device API | Request signature |
| `x-rcd-peer-device` | Device → device peer | Caller's relay device ID |
| `x-rcd-peer-signature` | Device → device peer | Peer signature |
| `x-rcd-file-actor` | Relay → device API | Relay account ID that the relay authorized |
| `x-rcd-actor-owner` | Relay → device API | `1` for the device owner, else `0` |
| `x-rcd-actor-name` | Relay → device API | That account's username |

The relay forwards the browser headers only on `/relay/devices/{id}/api/*` and
`/relay/api/*`, and the peer headers only on peer frames. Unbound devices ignore
both pairs. The relay sets the three actor headers itself on device API requests,
replacing any a browser sent; a device uses them only for requests without a
signed browser (`x-rcd-file-actor` for file save attribution, all three for prompt
senders).

### Device endpoints

| Endpoint | Auth | Body → response |
| --- | --- | --- |
| `GET …/transport/key?challenge=` | None | Key descriptor plus `trust: {bound, strict, accountKeyId, capabilities}` (`capabilities: ["remote-bind"]` when the next endpoint exists) and `certificate` (or `null`); `challenge` is 8 to 128 characters of `[A-Za-z0-9_-]` |
| `POST /api/transport/bind` | Encrypted, relay-forwarded owner | `{accountPublicKey, enrollSecret, webOrigin?}` → `{bound: true, accountKeyId, deviceId}`; 409 `already_bound`, 400 `invalid_bind`, 403 `owner_required` or `device_strict_mode` |
| `POST /api/transport/browser` | Encrypted | Browser certificate → `{browserId, role: "owner" \| "grantee"}` |
| `GET /api/transport/enrollment` | Encrypted | → `{deviceId, identityKey, accountKeyId, mac}` |
| `PUT /api/transport/device-certificate` | Owner | Device certificate → `{certificate}` |
| `PUT /api/transport/revocations` | Encrypted | `{revocations: [...]}` (at most 4096) → `{}`; 429 `revocation_limit` |
| `GET /api/transport/trust` | Owner | See [Strict mode](#strict-mode) |
| `GET /api/transport/grants` | Owner | → `{grants: [grant + createdAt, createdBy]}` |
| `PUT /api/transport/grants/{grantId}` | Owner | Grant → `{grant}` (with `createdAt`, `createdBy`) |
| `DELETE /api/transport/grants/{grantId}` | Owner | → `{}` |
| `POST /api/port-mappings/{id}/lease` | Owner | → `{leaseId, expiresAt}`; 404 `mapping_not_found`, 403 `owner_required` |
| `POST /api/peer/transport/certificate` | Peer | Caller's device certificate → `{}` |
| `GET /healthz` | None | Includes `relayStatus`: `null` or `{code: "device_id_mismatch", retryAt}` |

### Relay endpoints

| Endpoint | Access | Body → response |
| --- | --- | --- |
| `GET /relay/auth/kdf?login=` | Public | → `{mode: "account", kdf}`: the vault's parameters, or deterministic fake parameters (salt from an HMAC of the name with the session secret) for every name without a vault |
| `POST /relay/auth/login` | Public | `{identifier, authKey}`; `{login, password, upgrade: true}` for the security upgrade; `{identifier, password}` for the administrator and temporary-reset sign-ins → session or second-factor challenge |
| `POST /relay/auth/register` | Public | `{userId, email, username, authKey, recoveryAuthKey, vault, registrationPassword?}` → session; 202 `pendingApproval`; 409 `user_id_taken` or `conflict` |
| `POST /relay/auth/recover` | Public | `{login, recoveryAuthKey}` → recovery session (setup session for an account without a vault) or challenge |
| `GET /relay/auth/session` | Any | Session with the `account` field |
| `GET /relay/account/vault` | Signed in | → vault; 404 `vault_missing` |
| `PUT /relay/account/vault` | Signed in | `{vault, expectedRevision, currentAuthKey?, currentRecoveryAuthKey?, authKey?, recoveryAuthKey?}` → vault; 409 `vault_conflict`; 429 `rate_limited` (with `DELETE`, 10 per 5 minutes) |
| `DELETE /relay/account/vault` | Signed in | `{currentAuthKey?}` → `{}` |
| `GET /relay/account/browsers` | Signed in | → `{browsers: [{certificate, label, createdAt, lastSeenAt}], revocations}` (revocations of the current key only) |
| `POST /relay/account/browsers` | Signed in | `{certificate, label}` (label at most 80 characters) → `{}`; binds the session to the browser; at the limit drops the least recently seen other row; 401 `browser_revoked`; 403 `forbidden` for a session bound to another browser; 429 `browser_limit` only when no row can go |
| `DELETE /relay/account/browsers/{browserId}` | Signed in | → `{}`; deletes a browser row of the current key without a revocation; 409 `browser_in_use` while another live session is bound to it (the calling session does not count), 404 `not_found`. The Web calls it for its own browser when signing out |
| `POST /relay/account/revocations` | Signed in | `{revocation}` → `{}`; 429 `revocation_limit` |
| `POST /relay/account/security/reauth` | Signed in | `{authKey}` for accounts with a vault, `{password}` for accounts without one and the administrator, `{code}` when a second factor exists |
| `POST /relay/devices`, `POST /relay/devices/{id}/setup-token`, `/bootstrap`, `/token` | Owner, recent authentication | Create a device or return, issue or replace its token; 403 `reauthentication_required` |
| `GET /relay/devices/{id}/certificate` | Owner or recipient | Stored certificate; 404 if none |
| `PUT /relay/devices/{id}/certificate` | Owner | Device certificate (at most 16 KiB, matching `deviceId`), stored unverified |
| `GET /relay/devices/{id}/owner-account` | Owner or recipient | → `{accountKeyId, accountPublicKey}`; 404 |
| `GET /relay/shares/{id}/recipient-account`, `GET /relay/grants/{id}/recipient-account` | Owner of the live row | → `{accountKeyId, accountPublicKey}`; 404 `recipient_account_missing` (recipient without a vault) or `not_found` |
| `POST /relay/devices/{id}/port-mappings/{mappingId}/open?lease=` | Owner, or a recipient of the thread that created the mapping | `{path}` → `{url}`; 403 `preview_lease_required`; 400 `invalid_lease` (not 43 characters of `[A-Za-z0-9_-]`) |
| `POST /relay/admin/users/{id}/reset-password` | Relay admin | `{password}` (at least 12 characters, not for admin accounts) → one-time temporary password |

The relay checks browser certificates and revocations against the vault's account
key and stores device certificates unverified; devices and browsers never rely on
the relay's checks. "Recent authentication" means a sign-in, second factor or
reauthentication within the last 10 minutes. A password sign-in on a trusted
browser that skipped the second factor does not count, except a sign-in into a
setup session; an OAuth sign-in counts.

`PUT /relay/account/vault` rules: `revision` must be `expectedRevision + 1`;
`userId` (a canonical lowercase UUID v4), `accountKeyId` and `accountPublicKey`
never change. Every write needs a proof: `currentAuthKey` (the current password)
or `currentRecoveryAuthKey` (the current recovery code), or a recovery or reset
session, which must write a new password slot (and, for password accounts, a new
`authKey`); OAuth-only accounts need recent authentication instead and never send
`authKey`. `authKey` and `recoveryAuthKey` carry new values: changing the password
slot or KDF needs the new `authKey`, changing the recovery slot needs the new
`recoveryAuthKey`, and either sent with an unchanged slot is refused (400). For
compatibility, a write that changes only the body may prove the current recovery
code as `recoveryAuthKey`. Creating the first vault needs a setup session (or an
OAuth-only account), recent authentication and, for password accounts, the new
`authKey`; it deletes the account's login password hash. Slot changes sign out
the account's other sessions and forget its trusted browsers. A successful write
makes the session a full session, and a new password slot completes a pending
admin reset. `DELETE` needs `currentAuthKey`, a recovery or reset session, or
recent authentication for OAuth-only accounts; it signs out other sessions and
turns the current session into a setup session that is no longer bound to a
browser. The relay keeps the account's `authKey` and recovery hashes.

Login, register and recover answers include `{token, account, session}`, and
`GET /relay/auth/session` includes `account`:

```text
{ "mode": "account" | "legacy" | "oauth", "setupRequired": false,
  "recovery": false, "hasVault": true, "accountKeyId": "…" | null,
  "recentAuth": true, "browserBound": true }
```

`mode` is `oauth` for an OAuth-only account (with or without a vault), `account`
for any other account with a vault and `legacy` otherwise, including the
administrator. `recentAuth` is true when the session signed in or confirmed a
second factor within the last 10 minutes, so vault creation and OAuth slot
changes will not answer `reauthentication_required`; the Web asks a setup
session without it to sign in again before showing the setup form.
`browserBound` is true once a browser certificate was registered on the session.
A bound session refuses any other browser's certificate (403 `forbidden`), so a
browser that lost its key (for example after its site data was cleared while
the relay cookie stayed) is asked to sign in again instead of unlocking.

| Session | Created by | Allowed requests |
| --- | --- | --- |
| Setup (`setupRequired`) | Sign-in to an account without a vault (security upgrade, OAuth, temporary password, recovery code after start over), start over, sessions from before account keys | Session, logout, `/relay/auth/*`, `/relay/account/security/reauth`, `GET`/`PUT` vault, `/relay/devices`, `/relay/devices/*`, `/relay/api/*`, `/relay/ws`, `/relay/access` |
| Recovery (`recovery`) | `POST /relay/auth/recover` for an account with a vault | Session, logout, `GET`/`PUT`/`DELETE` vault |
| Reset (`recovery`) | Temporary-password sign-in to an account with a vault | As recovery |
| Full | Other sign-ins, any successful vault write, reauthentication of an account with a vault | Everything |

Other requests from restricted sessions answer 403 `forbidden`. The relay resolves
the session once per request, from the bearer token, the session query parameter
or the cookie, and every handler uses that session. Missing recent authentication
answers 403 `reauthentication_required`.

### Vault format

```json
{ "version": 1, "userId": "…", "accountKeyId": "…", "accountPublicKey": "B64 SPKI",
  "kdf": { "name": "argon2id", "memoryKiB": 65536, "iterations": 3, "parallelism": 1, "salt": "B64(16 bytes)" },
  "slots": { "password": { "iv": "B64(12)", "ciphertext": "B64" },
             "recovery": { "iv": "B64(12)", "ciphertext": "B64" } },
  "body": { "iv": "B64(12)", "ciphertext": "B64" },
  "revision": 1 }
```

- Slots: AES-256-GCM of the VDK with `wrapKey` (password) or `recoveryKey`
  (recovery), AAD `pockymoe-vault-slot-v1\n{userId}\n{accountKeyId}\n{slotName}`.
- Body: AES-256-GCM with the VDK, AAD
  `pockymoe-vault-body-v1\n{userId}\n{accountKeyId}\n{revision}`, plaintext
  `{"version": 1, "accountPrivateKey": "B64 PKCS8", "boundDevices": [{"deviceId": "…", "identityKey": "…"}, …], "updatedAt": 0}`.
- Key check after decryption: the imported key signs
  `pockymoe-account-key-check-v1` and the signature must verify with
  `accountPublicKey`.
