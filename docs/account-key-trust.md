# Account key trust

Each relay account has an account key that only the user's browsers hold in
clear. A device bound to that key obeys only requests signed by a browser that
the key certified, so a compromised relay can neither command a bound device nor
read its traffic (port previews excepted). Signing in once in a browser (password, plus the relay's second
factor when enabled) lets that browser use every device of the account, with no
expiry. A device is bound once, when it is installed or with `pockymoe bind`.

Devices that are not bound keep trusting the relay; the Web labels them
**Not protected · Relay trusted**.

Code: device `crates/supervisor/src/owner_trust.rs`, `grants.rs`, `tunnel.rs`,
`auth.rs`, `ports.rs`, `secure_transport/client.rs`; relay
`crates/relay/src/account.rs`; Web `apps/supervisor-web/src/lib/account*.ts`,
`deviceEnrollment.ts`, `relayTransportCrypto.ts`, `signedSharing.ts`,
`sharedDeviceTrust.ts`. Shared interop vectors:
`crates/relay/test-fixtures/account-key-v2.json`,
`crates/supervisor/tests/fixtures/account-key-v2.json` and
`apps/supervisor-web/src/lib/fixtures/accountKeyVectors.json`.

## Threat model

Against a bound device, a compromised relay cannot:

- send commands, read or change data, create port mappings or change sharing:
  every request through the relay must be encrypted and signed by a browser
  registered on the device;
- read the account key or the password: the relay stores only the encrypted
  vault and a slow hash of a key derived from the password;
- substitute the account key for a browser that has opened the vault before;
- substitute the device identity: owner browsers trust it only through a device
  certificate signed by the account key;
- create, widen or keep shared access: the device holds its own grants, which
  only owner browsers can change.

Limits:

- **Password strength.** The relay holds `authKey` (sent at every sign-in) and,
  after a database leak, so does anyone with the database. Each password guess
  costs one Argon2id derivation (64 MiB, 3 passes), so a weak password can be
  found offline. A guessed password opens the vault and the account key.
- **Legacy passwords.** A legacy account sends its old password in clear at the
  sign-in that secures it, and the relay keeps that password's older, cheaper hash
  afterwards. Choose a different password when securing the account.
- **Web code.** The JavaScript that holds the password and keys, and the device
  setup scripts, must come from a separate static origin
  ([self-hosting](self-host-relay.md#separate-web-origin)). A relay that serves
  the Web UI can serve modified code and take over the account.
- **First contact during migration.** A browser that has never opened the vault
  for a sign-in name cannot tell a real legacy account (no vault yet) from a relay
  that claims one. On that browser, the relay can obtain the password in clear.
  A browser that has opened the vault after signing in with the same name refuses
  a later legacy answer for it.
- **Fresh browsers.** A browser without a cached vault cannot detect an older
  vault revision or a hidden device certificate. It may then show a device bound
  after that revision as not protected and talk to whatever the relay presents.
  The real device still refuses the relay.
- **Metadata.** The relay sees accounts, devices, shares and grants, which
  devices have certificates, browser labels (user agent) and times, revocations,
  recipients' account public keys, HTTP methods and paths, thread, workspace and
  shell IDs, message types, sizes and timing, and the thread lineage and port
  mappings that devices report in heartbeats.
- **Availability.** The relay can drop, delay or refuse requests, hide devices and
  withhold revocation frames (owner browsers also deliver revocations, see
  [Revocations](#revocations)).
- **Unbound devices** remain fully controlled by the relay.
- **Port previews** are not end-to-end encrypted. The relay terminates preview
  HTTPS, sees the traffic and can open any mapping the device serves
  ([Port previews](#port-previews)).
- **Recipients** trust the owner's account key on first use unless they open an
  invite link. A relay that substitutes a recipient's account key gains only the
  access granted to that recipient; the safety code detects it.

Local authority is outside this model. Whoever can write the Supervisor database
or use its local API (CLI, local mode, server mode with its admin login) can bind
or reset the device and create local port mappings. Strict mode applies only to
requests that arrive through the relay tunnel.

## Keys

| Key | Held by | Lifetime |
| --- | --- | --- |
| Account key (ECDSA P-256) | Private key in clear only in browser memory during an unlock window; encrypted in the vault on the relay | Until the user starts over |
| Vault data key (VDK, 32 bytes, AES-256-GCM) | In clear only in browser memory during an unlock window | Same |
| Browser key (ECDSA P-256) | Non-extractable `CryptoKey` in the Web origin's IndexedDB, one per browser profile and account | Until sign-out or removal |
| Device identity key (ECDSA P-256) | `<database>.transport-identity` next to the Supervisor database | Device lifetime |

`accountKeyId = B64(SHA256(account SPKI))` and `browserId = B64(SHA256(browser SPKI))`.
A device's `identityKey` is the B64 uncompressed SEC1 point from its key
descriptor. The account fingerprint shown by devices is the first 12 characters
of `accountKeyId`.

## Password, vault and recovery code

The browser derives everything; for an account with a vault the relay never
receives the password.

- `master = Argon2id(NFC(password), salt, memoryKiB, iterations, parallelism)`,
  32 bytes (RFC 9106, version 1.3). New vaults use 65536 KiB, 3 iterations,
  parallelism 1 and a random 16-byte salt. These values are also minimums: the
  browser refuses to derive with weaker parameters and the relay refuses to store
  a vault that names them.
- `authKey = HKDF-SHA256(master, salt = empty, info = "pockymoe-relay-auth-v1")`,
  32 bytes, is sent instead of the password. The relay stores its own scrypt hash
  of `B64(authKey)`.
- `wrapKey = HKDF-SHA256(master, empty, "pockymoe-vault-wrap-v1")` encrypts the
  password slot of the vault.
- New and changed passwords, including OAuth unlock passwords, need at least 12
  characters (Unicode code points after NFC).

The vault (at most 64 KiB, [format](#vault-format)) holds the VDK twice, in a
password slot and a recovery slot, and a body encrypted with the VDK that contains
the account private key and the list of bound device IDs. After decrypting, the
browser imports the private key as non-extractable and checks that it signs for
`accountPublicKey` and that `accountKeyId` matches. It caches
`{userId, accountKeyId, accountPublicKey, revision, boundDevices}` and refuses a
later vault with a different account key ("The relay served a different account
key") or a lower revision ("The relay served an older account vault"). Only
**Start over** clears that cache.

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
reloaded, and when another account becomes active. Binding a device (including
copying a setup command), certifying a device, removing a browser or device,
changing the password, replacing the recovery code and starting over need an open
window; otherwise the browser asks for the password with the reason.

## Browsers

At sign-in, a browser without a browser key creates one and has the account key
sign a browser certificate for it. The browser registers the certificate with the
relay at every sign-in (`POST /relay/account/browsers`, labelled with the first 80
characters of the user agent). Certificates have no expiry. A browser stays
authorized until its key is deleted locally (sign-out) or the browser is removed.

On first contact with each protected device in a page session, a browser:

1. sends the account's revocation list, if the device is one of its account's
   bound devices (encrypted `PUT /api/transport/revocations`);
2. registers its certificate (encrypted `POST /api/transport/browser`). The device
   accepts a certificate that verifies with its pinned account key (role `owner`)
   or with the account key of a recipient in a live grant (role `grantee`), and is
   not revoked;
3. signs every later encrypted request, including download chunks and
   service-worker requests, with `x-rcd-session: <browserId>` and
   `x-rcd-signature`. A WebSocket channel inherits the principal of the signed
   request that created it.

If a device answers 401 `browser_registration_required` (it forgot the browser,
or the signature failed), the browser registers again and retries once; the
device rejects before dispatch, so a mutation runs at most once. On 401
`browser_revoked` the browser deletes its key and shows "This browser was
removed. Enter your password to reconnect."; entering the password creates a new
browser key.

A signed-in session whose browser has no browser key shows **Unlock this browser**
once: "Enter your password once to connect to your devices."

## Binding devices

Binding pins the account public key in the Supervisor database, stores a one-time
enrollment secret and turns on strict mode. Enrollment then installs a device
certificate signed by the account key.

**New devices.** On the Devices page, **Add device** and the setup commands copied
from a device's menu include the binding flags. The browser creates a pending
enrollment (a random 32-byte secret, kept in its IndexedDB for 7 days) and builds:

```sh
curl -fsSL 'https://app.example.com/setup.sh' | sh -s -- --relay https://relay.example.com \
  --token rcd_… --port 45679 --web-origin https://app.example.com \
  --account-key <B64 SPKI> --enroll <B64 secret>
```

PowerShell takes `-WebOrigin`, `-AccountKey` and `-Enroll`. Native setup
(`pockymoe setup`, env `POCKYMOE_WEB_ORIGIN`, `POCKYMOE_ACCOUNT_KEY`,
`POCKYMOE_ENROLL`) binds the database before it starts the Supervisor. Running
the command again on an installed device re-arms enrollment.

**Existing devices.** An unbound device shows **Protect this device**, which gives
a command to run on the device:

```sh
pockymoe bind --account-key <B64 SPKI> --enroll <B64 secret> --web-origin https://app.example.com
```

### `pockymoe bind`

The command works offline: it opens the Supervisor database directly and needs
no running service. A running Supervisor applies the change at once.

| Flags | Effect |
| --- | --- |
| `--account-key KEY --enroll SECRET` | Bind and turn on strict mode. Bound to the same key: replaces the enrollment secret. Bound to another key: refuses ("use pockymoe bind --reset to replace it"). |
| `--web-origin URL` | Stored with the binding. HTTPS origin without path, query or credentials (HTTP only for `localhost`, `127.0.0.1`, `[::1]`). Defaults to env `POCKYMOE_WEB_ORIGIN`. |
| `--status` | Prints `Bound to account <fingerprint>. Strict mode: on.` or `Not bound. Strict mode: off.` |
| `--reset` | Asks for confirmation, then deletes the pinned key, enrollment secret, device certificate, browser registrations, revocations, grants, peer registrations and port mappings created by browsers, and turns strict mode off. Threads, local port mappings, the identity key and the relay device ID remain. Combine with `--account-key` and `--enroll` to rebind to another key in one step. |
| `--yes` | With `--reset`, skips the prompt. |
| `--database PATH` | Supervisor database (env `POCKYMOE_DATABASE_PATH`, default `~/.remote-codex/relay-supervisor.sqlite`). |

The database must belong to a Supervisor version that supports binding ("Update
the Supervisor before binding this database"). A bound device refuses a relay
greeting that assigns it a different device ID and keeps reconnecting; reset it
before attaching it to another relay device.

### Enrollment

Enrollment completes on its own in the browser that created the command, on any
page, while the device is online (checked every 15 seconds while the page is
visible):

1. The browser fetches the device's key descriptor and sends an encrypted, unsigned
   `GET /api/transport/enrollment`. The device answers
   `{deviceId, identityKey, accountKeyId, mac}` with
   `mac = HMAC-SHA256(enrollSecret, "pockymoe-acct-enroll-v1\n{deviceId}\n{identityKey}\n{accountKeyId}")`,
   or 404 `enrollment_missing`.
2. The browser checks the MAC with its pending secret, the account key ID and that
   `identityKey` equals the descriptor's, then signs a device certificate (asking
   for the password if the window is closed) and sends it in a signed
   `PUT /api/transport/device-certificate`.
3. The device verifies the certificate against its pinned key, its own device ID
   and identity key, stores it, deletes the secret and answers `{certificate}`.
4. The browser uploads the certificate to the relay
   (`PUT /relay/devices/{id}/certificate`), adds the device to the vault's bound
   devices and deletes the pending enrollment.

### Device states

| Label | Meaning |
| --- | --- |
| **Protected** | The device presented a certificate that verifies with the account key. |
| **Not protected · Relay trusted** | Not bound and not listed in the vault. |
| **Waiting for first connection** | This browser holds a pending enrollment for the device. |
| **Certificate missing** | The vault lists the device, or it reports itself bound, but it presents no valid certificate. The connection is blocked and never falls back to relay trust. |
| **Protection not verified** | No descriptor yet, for example while the device is offline. |

The device's key descriptor (`GET …/transport/key`) carries the device's own
`certificate` (or `null`) and an unsigned hint `trust: {bound, strict,
accountKeyId}`. Browsers use the hint only to refuse a downgrade, never to grant
trust, and verify the certificate the device presents rather than the relay's
copy. **Manage device trust** (the encryption status icon) shows the state, the
bound account fingerprint and the browsers registered on the device.

If a pending enrollment expires or its browser data is lost, copy the device's
setup command again in a browser of the account and run it on the device. A
device that the vault lists as bound must keep a valid certificate: the Web does
not re-enroll it. If such a device loses its identity file or certificate (for
example after `pockymoe bind --reset`), remove it from the account, reset it
locally and set it up again as a new device.

## Strict mode

A bound device is strict. Through the relay tunnel these need no browser
signature:

- `GET /healthz`;
- `GET` of a key descriptor (any path ending in `/transport/key`);
- `GET /api/publications/{token}` ([public links](#public-links));
- encrypted `POST /api/transport/browser`, `GET /api/transport/enrollment` and
  `PUT /api/transport/revocations` (refused in plaintext on every device);
- encrypted `POST /api/peer/transport/certificate` from a relay-identified peer.

Everything else needs an encrypted request signed by a registered owner or
grantee browser, or a verified peer. WebSocket clients must use a secure channel
created by a signed request; others are closed. The device rechecks channels at every relay heartbeat (3 seconds)
and immediately after revocations and grant changes, and closes those whose
browser, grant or peer registration is no longer valid. File saves are attributed
to `owner:<browserId>` or `grantee:<userId>`, never to the relay's
`x-rcd-file-actor` header.

Owner-only paths: `/api/transport/trust`, `/api/transport/device-certificate`,
`/api/transport/grants`, `/api/ports`, `/api/port-mappings`, `/api/config`,
`/api/management`, `/api/settings`. Direct local access (CLI, local mode, server
mode) is not subject to strict mode; the trust and bootstrap endpoints answer
only encrypted tunnel requests.

| Code | Status | Cause |
| --- | --- | --- |
| `device_strict_mode` | 403 | Plaintext request to a strict device outside the exceptions; `/api/peer/*` without a relay peer identity; a plaintext bootstrap or trust endpoint request on any device |
| `browser_registration_required` | 401 | Encrypted request without `x-rcd-session`, from an unregistered browser, with an invalid signature or a registration that is no longer valid; trust endpoints on an unbound device |
| `browser_revoked` | 401 | The browser is revoked (request or registration) |
| `account_mismatch` | 403 | The certificate's account key is neither the pinned key nor a recipient key in a live grant, or the browser ID is registered for another account or user |
| `owner_required` | 403 | Recipient request to an owner-only path |
| `grant_denied` | 403 | Recipient request outside its grants |
| `invalid_grant` | 400 | Malformed grant ([sharing](#sharing)) |
| `grantee_is_owner` | 400 | The recipient key is the device's own account key |
| `grant_limit` | 409 | The device already has 256 grants |
| `enrollment_missing` | 404 | No enrollment secret is pending |
| `invalid_certificate` | 400 | Any other failed verification: certificate for another device, identity or key, revoked device, invalid revocation list, unbound device or device that has not connected to the relay yet |
| `peer_certificate_required` | 401 | Strict target without a valid peer registration and signature |
| `peer_target_unverified` | — | Caller-side error: a bound caller cannot verify the target's certificate |

Owner endpoint `GET /api/transport/trust` returns `{deviceId, bound, strict,
accountKeyId, certificate, browsers: [{browserId, role, userId, registeredAt,
lastSeenAt, current}], revocations}` (`revocations` is a count).

## Revocations

Removing a browser (**Account settings → Account key and browsers → Remove
browser**) or a device (**Devices → Remove device**) signs a revocation
`{kind: "browser" | "device", subjectId}` with the account key and posts it to the
relay. Revocations are permanent. They reach devices by two paths:

- the relay sends `{"type": "relay.revocations", "revocations": [...]}` with all of
  the account's revocations to every connected device of the account when one is
  posted, and to each device right after it connects;
- an owner browser sends the list to each bound device on first contact in a page
  session (`PUT /api/transport/revocations`, encrypted, unsigned).

The device verifies every entry against its pinned key, rejects the whole list if
one entry fails, and keeps accepted entries as tombstones.

A revoked browser loses its registration, the port mappings it created and every
open channel. A revoked device is refused as a peer by the account's other
devices and by owner browsers ("This device was removed from the account.").
**Remove device** then deletes the relay registration, so the device
can no longer connect; it keeps its local binding until `pockymoe bind --reset`.
A removed device ID cannot be enrolled again.

A device stores only revocations signed by its own account key. Removing one of
your browsers therefore cuts it off from your own bound devices, not from devices
that other accounts share with you; for those, the owner must revoke the share.
Removing a browser does not change the password: anyone who knows it can sign in
again and create a new browser key.

## Account flows

| Flow | Behavior |
| --- | --- |
| Register | The browser generates the user ID (UUID v4), account key, VDK, salt and recovery code, shows the code for confirmation, then sends `POST /relay/auth/register` with `authKey`, `recoveryAuthKey` and the vault. With registration approval, the vault waits with the request and is stored on approval. A collision answers 409 `user_id_taken`; registering again generates new keys. |
| Sign in | One form: fetch the KDF parameters for the sign-in name, derive, sign in with `authKey`, complete the second factor, fetch and open the vault, register the browser. The unlock window opens without a separate step. |
| Legacy account | An account without a vault signs in with its password once. The session is restricted until **Secure your account** creates the vault: choose a password of at least 12 characters and save the recovery code. Securing signs out the account's other sessions and forgets its trusted (two-step) browsers. |
| OAuth account | No login password. At the first sign-in the user chooses an **unlock password**, which only encrypts the vault; no `authKey` is sent. A browser without a browser key asks for it after OAuth sign-in. Changing it, replacing the recovery code and starting over need an OAuth sign-in or second factor within 10 minutes (otherwise 403 `reauthentication_required`). |
| Forgot password | **Forgot password? Use your account recovery code** on the sign-in page: the recovery code and a new password. The relay checks `recoveryAuthKey` (10 attempts per 5 minutes per name) and always asks for the configured second factor, even on trusted browsers. The browser opens the recovery slot, writes a new password slot and `authKey`, and registers itself. |
| Admin reset | **Use an admin reset password** with a temporary password from the relay administrator (one use; the configured second factor is asked even on trusted browsers). While the reset is pending, the relay answers `legacy` for the name, so browsers that have opened the vault refuse the normal form and point to this option. Then enter the recovery code and a new password, or **Start over**. |
| Change password | Account settings, unlock window, current `authKey`. New random salt. Signs out other sessions and forgets trusted browsers; browser keys and device registrations stay valid. |
| Regenerate recovery code | Shows a new code once; the old one stops working. Signs out other sessions and forgets trusted browsers. |
| Start over | Deletes the vault, the account's browser certificates and the relay copies of its device certificates, and signs out other sessions; the browser then asks for a password and shows a new recovery code to create a new account key. Every bound device refuses the new key until `pockymoe bind --reset` and a new bind on that device; recipients see a changed safety code. |
| Sign out | Deletes this browser's key. Its certificate stays listed until removed; the cached account key ID and vault revision stay to detect substitution at the next sign-in. |

The relay administrator account signs in with its configured password, holds no
account key and cannot use the portal.

An admin reset of an account with a vault sets a one-time temporary password and
invalidates the old `authKey`; the vault is unchanged, so only the recovery code
(or starting over) restores device access. An admin reset of an account without a
vault sets an ordinary password that the user then uses to secure the account.
Nobody, including the relay operator, can recover an account key without the
password or recovery code.

## Sharing

A bound device enforces shares (one thread) and grants (device or workspace
access) from its own grant list, in addition to the relay. Only owner browsers can
read or change it; each grant mirrors one relay row and names the recipient's
account key ([grant format](#records)). The device recomputes a recipient's access
on every request with the relay's rules (`crates/protocol` access: owner, then a
share with thread lineage up to three levels, then grants merged by highest scope
and access) and filters recipient WebSocket events and messages itself. Grants are
created only while the device is online.

Creating or editing shared access for a protected device:

1. The relay row is created or updated.
2. The browser builds the grant only from the choices in the dialog, accepting
   only the new row ID and recipient user ID from the relay, and refuses when the
   relay row differs ("No device grant was signed").
3. It fetches the recipient's account public key
   (`GET /relay/{shares|grants}/{id}/recipient-account`); without a vault this
   fails with "The recipient must secure their account before a protected device
   can honor this share."
4. **Review device grant** shows the exact device, scope, access, expiry, recipient
   and safety code. An unverified recipient needs an explicit acknowledgement.
5. The grant is sent in a signed `PUT /api/transport/grants/{grantId}`.

If any step after the first fails, the relay change is rolled back.

Validation: a recipient key must hash to its ID and must not be the device's own
key (`grantee_is_owner`); one recipient key maps to one user and public key across
grants; device-scope grants need `workspaceScope: "all"`; shares are thread-scoped;
at most 256 grants (`grant_limit`).

**Safety code.** The first 12 characters of the Crockford base32 encoding of
`SHA256("pockymoe-acct-safety-v1\n" + accountKeyId)`, in groups of four. Each user
sees their own in Account settings; the owner sees the recipient's per share,
marked **Verified** after **I compared the safety code** (stored in that browser).

**Reconciliation.** Share details compare relay rows with device grants: **Not
active on device** (relay row without grant) offers **Review and confirm**;
**Device permissions differ from this share** and **Recipient account key has
changed** offer **Update**; grants without a relay row are listed as **Still
active on device** with **Revoke**. Every signing action shows the review first.

**Revoking** deletes the grant on every device of the account whose certificate
this browser has verified, then the relay row, and fails while such a device is
offline.
Deleting the last grant for a recipient key drops that recipient's browser
registrations; a later grant lets the same, still valid browser certificates
register again.

**Recipients** register their own browser certificates (role `grantee`). They
verify the device certificate with the owner's account key from
`GET /relay/devices/{id}/owner-account`, pinned on first use and labelled so. An
invite link pins it beforehand:

```text
{webOrigin}/shared#v=2&device={deviceId}&owner={ownerAccountKeyId}[&thread={threadId}]
```

After the recipient confirms the device and owner key shown, the browser pins that
key; a different key from the relay, or a certificate that does not verify with
it, blocks the connection. **Copy invite link** is offered only after the owner's
browser verified the device certificate.

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
encryption (version 1) stay plaintext.

## Port previews

In strict mode a device reports and opens only local mappings (created through
its local API, such as `pockymoe preview create`) and mappings created by an
owner browser while that browser's registration exists. Recipients get
`owner_required`. Details: [port previews](port-previews.md#bound-devices).

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

The relay forwards the browser headers only on `/relay/devices/{id}/api/*` and
`/relay/api/*`, and the peer headers only on peer frames. Unbound devices ignore
both pairs.

### Device endpoints

| Endpoint | Auth | Body → response |
| --- | --- | --- |
| `GET …/transport/key?challenge=` | None | Key descriptor plus `trust: {bound, strict, accountKeyId}` and `certificate` (or `null`) |
| `POST /api/transport/browser` | Encrypted | Browser certificate → `{browserId, role: "owner" \| "grantee"}` |
| `GET /api/transport/enrollment` | Encrypted | → `{deviceId, identityKey, accountKeyId, mac}` |
| `PUT /api/transport/device-certificate` | Owner | Device certificate → `{certificate}` |
| `PUT /api/transport/revocations` | Encrypted | `{revocations: [...]}` (at most 4096) → `{}` |
| `GET /api/transport/trust` | Owner | See [Strict mode](#strict-mode) |
| `GET /api/transport/grants` | Owner | → `{grants: [grant + createdAt, createdBy]}` |
| `PUT /api/transport/grants/{grantId}` | Owner | Grant → `{grant}` |
| `DELETE /api/transport/grants/{grantId}` | Owner | → `{}` |
| `POST /api/peer/transport/certificate` | Peer | Caller's device certificate → `{}` |

### Relay endpoints

| Endpoint | Access | Body → response |
| --- | --- | --- |
| `GET /relay/auth/kdf?login=` | Public | → `{mode: "account" \| "legacy", kdf}`. Unknown names get deterministic fake parameters (salt from an HMAC of the name with the session secret) and `account`; existing accounts without a vault, or with a pending admin reset, answer `legacy`. |
| `POST /relay/auth/login` | Public | `{identifier, authKey}`, or `{identifier, password}` for legacy, admin and temporary-reset sign-ins → session or second-factor challenge |
| `POST /relay/auth/register` | Public | `{userId, email, username, authKey, recoveryAuthKey, vault, registrationPassword?}` → session; 202 `pendingApproval`; 409 `user_id_taken` |
| `POST /relay/auth/recover` | Public | `{login, recoveryAuthKey}` → recovery session or challenge |
| `GET /relay/auth/session` | Any | Session with the `account` field |
| `GET /relay/account/vault` | Signed in | → vault; 404 `vault_missing` |
| `PUT /relay/account/vault` | Signed in | `{vault, expectedRevision, authKey?, currentAuthKey?, recoveryAuthKey?}` → vault; 409 `vault_conflict` |
| `DELETE /relay/account/vault` | Signed in | `{currentAuthKey?}` → `{}` |
| `GET /relay/account/browsers` | Signed in | → `{browsers: [{certificate, label, createdAt, lastSeenAt}], revocations}` |
| `POST /relay/account/browsers` | Signed in | `{certificate, label}` (label at most 80 characters) → `{}`; 401 `browser_revoked` |
| `POST /relay/account/revocations` | Signed in | `{revocation}` → `{}` |
| `POST /relay/account/security/reauth` | Signed in | `{authKey}` for accounts with a vault, `{password}` for legacy and admin accounts, `{code}` when a second factor exists |
| `GET /relay/devices/{id}/certificate` | Owner or recipient | Stored certificate; 404 if none |
| `PUT /relay/devices/{id}/certificate` | Owner | Device certificate (at most 16 KiB, matching `deviceId`), stored unverified |
| `GET /relay/devices/{id}/owner-account` | Owner or recipient | → `{accountKeyId, accountPublicKey}`; 404 |
| `GET /relay/shares/{id}/recipient-account`, `GET /relay/grants/{id}/recipient-account` | Owner of the live row | → `{accountKeyId, accountPublicKey}`; 404 `recipient_account_missing` |
| `POST /relay/admin/users/{id}/reset-password` | Relay admin | `{password}` (at least 12 characters, not for admin accounts) |

The relay checks browser certificates and revocations against the vault's account
key and stores device certificates unverified; devices and browsers never rely on
the relay's checks.

`PUT /relay/account/vault` rules: `revision` must be `expectedRevision + 1`;
`userId`, `accountKeyId` and `accountPublicKey` never change. Creating the first
vault needs a setup session or an OAuth-only account. Changing the password slot
or KDF needs the new `authKey`; changing the recovery slot needs the new
`recoveryAuthKey`. Slot changes need `currentAuthKey`, a recovery or reset
session (which must write a new password slot and `authKey`), or, for OAuth-only
accounts, recent authentication. They sign out the account's other sessions and
forget its trusted browsers. `DELETE` needs `currentAuthKey`, a reset session, or
recent authentication for OAuth-only accounts.

Login, register and recover answers include `{token, account, session}`, and
`GET /relay/auth/session` includes `account`:

```text
{ "mode": "account" | "legacy" | "oauth", "setupRequired": false,
  "recovery": false, "hasVault": true, "accountKeyId": "…" | null }
```

| Session | Created by | Allowed requests |
| --- | --- | --- |
| Setup (`setupRequired`) | Sign-in to an account without a vault, start over | Session, logout, `/relay/auth/*`, `GET`/`PUT` vault, `/relay/devices`, `/relay/devices/*`, `/relay/api/*`, `/relay/ws`, `/relay/access` |
| Recovery (`recovery`) | `POST /relay/auth/recover` | Session, logout, `GET`/`PUT` vault |
| Reset (`recovery`) | Temporary-password sign-in | As recovery, plus `DELETE` vault |
| Full | Other sign-ins, any successful vault write | Everything |

Other requests from restricted sessions answer 403 `forbidden`. Missing recent
authentication answers 403 `reauthentication_required`.

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
  `{"version": 1, "accountPrivateKey": "B64 PKCS8", "boundDevices": ["deviceId", …], "updatedAt": 0}`.
- Key check after decryption: the imported key signs
  `pockymoe-account-key-check-v1` and the signature must verify with
  `accountPublicKey`.
