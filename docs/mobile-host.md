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
The Relay start path first renders a plain consent page: 「Pockymoe 手机 App
正在请求登录」 and 「如果不是你刚刚发起的，请关闭此页」, with explicit 「继续」 and
「取消」 buttons. GET/query parameters cannot approve it. A one-use form nonce,
bound to an HttpOnly browser cookie and the Relay origin, is required for POST.
Continuing creates the normal OAuth state, PKCE verifier and provider-binding
cookie; cancelling returns a protected cancellation callback without signing in.
Both Google and GitHub receive `prompt=select_account` so a remembered browser
account does not silently choose the identity. The invite is stored only as a hash.

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
notice. Mobile starts require this consent step; browser and desktop OAuth keep
their existing direct provider redirects. No login session is put in the deep
link. Cancellation, an unsupported auth bridge or a failed connection shows the
existing visible sign-in warning; it never silently succeeds or falls back to
embedded provider sign-in.

**What this does and does not protect.** The proof binds redemption to whichever
client prepared the flow. It prevents a preparer from polling a public start ID
to steal a victim's session, and prevents someone with only an intercepted
callback code from redeeming a flow initiated by the victim's WebView. It does
**not** establish that the initiating client is the genuine Pockymoe app: a
malicious app can prepare its own proof and claim the `remotecodex://` scheme.
The consent warning, explicit button and account picker reduce this risk; they
do not eliminate it. A verified/claimed HTTPS callback (Universal Links/App Links)
would bind the callback more strongly to the app and is **not implemented yet**.
Do not approve this page unless you just requested the sign-in yourself.

`prepare` uses the existing admission limiter before proof/invite validation:
five attempts per resolved client IP per ten minutes. Anonymous attempts also
have separate **2,000-per-provider** ten-minute buckets and an aggregate
**4,000 anonymous attempts per ten minutes** cap, including invalid proofs and
incorrect invites. Correct configured invites or an authenticated account
session bypass the anonymous buckets, so an
anonymous provider flood cannot consume their admission. Per-client limits and
the 10,000 pending-row storage cap still apply. Missing an invite is permitted
for existing-account provider login; a new user without an invite may be refused
after provider identity verification as before. Anonymous preparation can occupy
at most 4,000 new rows per ten-minute window across the two providers, leaving
capacity for protected attempts. Invalid invites/proofs consume the client budget.

Relay has an explicit `POCKYMOE_TRUSTED_PROXY_IPS` comma-separated IP setting.
With no configured proxies, forwarding headers are ignored. If the transport
peer is configured, `X-Forwarded-For` is read from right to left through configured
trusted hops, stopping at the first untrusted IP; attacker-prepended entries
cannot select a bucket. Malformed/missing headers fall back to the peer, and
missing peer metadata shares an `unknown` bucket. Both the general auth middleware
and mobile preparation use this resolver. Configure only actual proxy IPs and
ensure the ingress appends or overwrites the forwarding chain; do not accept
arbitrary client-supplied headers as authoritative.

### Explicit CDN visitor-header mode

With `POCKYMOE_CLIENT_IP_HEADER=CF-Connecting-IP`, a request from a configured
trusted **direct peer** uses that header's single valid IP. It does not walk XFF.
An untrusted peer ignores it; a missing, malformed or duplicated value falls back
to the transport peer rather than consulting a potentially forged XFF. The setting
is empty/unset by default, preserving the self-hoster XFF-chain policy above.
Both the existing general auth IP limiter (120/minute) and mobile prepare use the
same resolver. Before trusted-proxy support those auth buckets were keyed on the
proxy peer, so this corrects a pre-existing shared-proxy limit too. Login/recovery,
MFA and other account limits remain keyed to account/identifier, not the proxy.

The owner has confirmed this deployment path: client → Cloudflare → host reverse
proxy or tunnel → Relay on loopback. Deployment uses the explicit visitor-header
mode and checks that NPM preserves that header. It no longer requires Nginx to
rewrite `$remote_addr` to the visitor first. In a single private pre-change backup
step, it appends missing `POCKYMOE_TRUSTED_PROXY_IPS=127.0.0.1` and
`POCKYMOE_CLIENT_IP_HEADER=CF-Connecting-IP` settings independently, preserving
existing canonical or legacy values. Explicit incompatible values stop the
preflight rather than being silently overwritten. Rollback restores the original
environment.

**Origin bypass is outside this preflight's proof.** The origin must be restricted
to Cloudflare or its authenticated tunnel, and the host proxy must preserve the
CDN's visitor header. A probe from inside the host cannot establish that restriction.
If someone bypasses Cloudflare through that trusted host proxy, they may supply
an arbitrary `CF-Connecting-IP`. This only changes the apparent rate-limit key;
it does not authenticate them, change authorization, skip consent, prove an OAuth
identity, or provide the callback code/page proof. But IP fairness is lost: they
can evade their own client quota or exhaust a chosen apparent IP's quota. The
per-provider/aggregate anonymous budgets, pending-row bound, account/identifier
limits, password-work limits, MFA and login handoff protections remain in force.
Valid-invite/account-session bypass of anonymous budgets is unchanged and still
requires that invite/session; it is not granted by an IP header. This mitigation
is not a substitute for restricting direct access to the origin.

See [runtime deployment](github-runtime.md#portable-release-addresses) for audit
boundaries. No production ingress/firewall restriction was queried or verified
by local fixture tests; the confirmed CDN topology comes from the owner.

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
