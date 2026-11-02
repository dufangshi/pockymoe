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

The Relay returns to the fixed `remotecodex://auth/callback?id=…` deep link, matching
the installed apps' existing scheme. The shell returns that URL through
`auth.openExternal`. The page checks the callback and identifier, then redeems
`{ id, proof }` at `POST /relay/auth/mobile/exchange`. The identifier expires after
ten minutes and is single-use; intercepting it without the page's proof cannot
establish a session. Login cookies are issued into the WebView. Existing MFA,
trusted-browser, account setup/unlock and remembered destination rules still apply.
No login session or native credential is placed in the deep link. Cancellation
keeps the current page; refusal and pending registration resume the existing
localized portal notices. Browser and desktop OAuth keep their normal redirects.

## System activity surfaces

One activity summarizes the account. It starts only in the foreground, never via
push-to-start. Native titles come only from local `activity.sync`; unknown titles
use the shell's generic thread-running text. Running uses a static ring plus the
system timer, the documented [status-language exception](ui-principles.md#6-状态语言).
See [thread notifications](thread-notifications.md#live-activity-updates) for the
content-free APNs schema, retention and delivery rules. The native client owns
safe areas, keyboard viewport handling, launch fade, gestures and background
service/activity rendering; Web tests with a mocked bridge cannot certify those.
