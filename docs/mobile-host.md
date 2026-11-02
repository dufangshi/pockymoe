# Mobile Web host

The iOS and Android clients load the deployed Web UI in a full-screen WebView.
`window.pockymoeHost.capabilities()` identifies `surface: "mobile"`, a platform
(`ios` or `android`), and `liveActivity`, `nativePush`, `haptics` booleans. Host
presence alone never selects desktop navigation or window chrome. The bridge is
available only to the configured Web origin's top frame; the shell enforces this.
Browsers retain their normal layout, and `surface: "desktop"` selects the existing
Electron interface. The Web waits for capabilities before selecting a surface.

Mobile keeps the normal responsive phone layout. Its account activity index uses
the same store and change-only `activity.sync` snapshots as desktop, without
mounting DesktopShell, its rail, or its window frame. Titles, device names,
workspace labels and request summaries stay on the device via the encrypted
Supervisor connection. Snapshots run independently of the selected device's
capability discovery gate and clear on logout.

## Bridge behavior

- `back` closes the last visible menu, dialog or error bubble using its existing
  Escape handler (including unsaved/busy guards). Otherwise it uses React Router's
  history index. A cold deep link is a root entry. `commandResult` returns
  `{ ok: true, data: { handled: boolean } }`; only an unhandled root back exits.
- `push.register()` is called from notification enable, or to renew a previously
  enabled registration. The iOS token is posted to the existing native notification
  endpoint. Only its registration ID is persisted in page storage, scoped by Relay
  origin and account, and checked against the current session's settings. Android
  keeps its native background service; there is no Web Push/FCM substitution.
- The shell's `liveActivityToken` command updates the current registration through
  `PUT /relay/account/notifications/native/live-activity` with `{ id, token }`.
  Tokens received before notification registration are held in memory. A token
  command never requests permission or re-enables disabled notifications.
- Haptics occur only on successful message submission (`sent`), a newly arriving
  foreground input/approval request (`attention`), and activating a dangerous
  confirmation button (`warning`). Ordinary clicks and scrolling do not vibrate.
- `appearance.sync` includes effective theme, pack, accent, page background, locale
  and font scale. The shell uses the background for launch/restore and system bars.
- Shared activity commands, `window.setTitle`, `shell.openExternal`, `files.saveAs`,
  settings and badges retain their contract. Mobile `window.openThread` navigates
  in the same WebView; preferences and pet methods are shell no-ops.

## System-browser sign-in

Google and GitHub use `auth.openExternal(url)`, never embedded provider login.
The Web creates a random proof and sends its SHA-256/base64url hash to
`POST /relay/auth/mobile/prepare` with the provider and optional invite code.
The returned Relay start path binds the normal OAuth state, PKCE verifier and
system-browser HttpOnly cookie. The invite code is stored only as a hash.

The preparation ID is public and cannot be redeemed. Once the provider returns,
the Relay generates a **fresh random 256-bit code**, stores only its hash, and
returns it only through `remotecodex://auth/callback?code=…`. The shell passes that
URL through `auth.openExternal`; the page redeems `{ code, proof }` at
`POST /relay/auth/mobile/exchange`. The code expires with the ten-minute attempt
and is consumed atomically. Missing, wrong, pending, expired or used codes all
produce the same unauthorized response without cookies. Someone who prepared a
phishing link and knows the preparation ID/proof still cannot obtain the callback
code by polling. A stolen callback code alone also lacks the page's proof.

Login cookies are issued into the WebView, preserving MFA, trusted-browser,
account setup/unlock and the remembered destination. Refusal and pending
registration use the same secret-code handoff before showing the localized portal
notice. There is no custom authorization/start page: the start endpoint redirects
directly to the provider's configured application authorization UI. Browser and
desktop OAuth keep their existing redirects. No login session is put in the deep
link. Cancellation, an unsupported auth bridge or a failed connection shows the
existing visible sign-in warning; it never silently succeeds or falls back to
embedded provider sign-in.

`prepare` uses the existing admission limiter before proof/invite validation:
five attempts per transport-peer IP per ten minutes and 200 globally per ten
minutes. Invalid invites/proofs consume the same budget. Raw peer IPs are used;
unauthenticated forwarding headers cannot choose a new bucket. Requests without
peer metadata share an `unknown` bucket. Existing browser auth limits also apply.

## Web and Relay connection origins

The native shell resolves the person's one entered address before loading Web:
an HTTPS redirect to the Web UI keeps the typed Relay origin; a direct Web UI
uses one origin for both; an advanced Relay override and stored Web/Relay pairs
are preserved. A failed first connection stays on the native connection page
with an inline reason; native offline recovery offers Retry and Change address.
Those native screens and migration probes belong to the client repository.

Before rendering or sending any session request, the page reads the mobile
shell's `settings.get()` pair and uses its `relayOrigin` in preference to the
Web build's origin. This applies to sign-in, account/device HTTP, sockets,
notification registration and media. Web identity/passkey trust stays bound to
the Web origin. The service worker keeps a separate Relay setting for each
controlled page and delegates custom-origin encrypted reads back to that exact
page; it cannot route them through another open window. Invalid or unavailable
shell settings block requests with a visible failure instead of contacting a
potentially unrelated Relay built into the Web site.

## System activity surfaces

One activity summarizes the account. It starts only in the foreground, never via
push-to-start. Native titles come only from local `activity.sync`; unknown titles
use the shell's generic thread-running text. Running uses a static ring plus the
system timer, the documented [status-language exception](ui-principles.md#6-状态语言).
See [thread notifications](thread-notifications.md#live-activity-updates) for the
content-free APNs schema, retention and delivery rules. The native client owns
safe areas, keyboard viewport handling, launch fade, gestures and background
service/activity rendering; Web tests with a mocked bridge cannot certify those.
