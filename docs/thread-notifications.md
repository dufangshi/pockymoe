# Thread notifications and browser tab status

## Behavior

Open **Settings → Notifications** to choose the account's notification scope:

- **Subscribed turns** (default): click the bell in a working turn's footer. The
  filled accent bell, pressed state and inline confirmation show that this exact
  turn will notify you. Click again to cancel. Subscriptions persist across
  refreshes, browsers and devices; they do not carry into a subsequent turn.
- **All turns**: every eligible turn notifies, matching the previous push policy.
- **Selected devices and workspaces**: add removable device/workspace cards. A
  device includes all its threads; a workspace includes its threads on that
  specific device. Workspace selection uses the same device-first picker as split
  conversations. Removing a card offers Undo. An empty list sends no notices.

铃铛的点击背景只在按压期间显示，抬手后恢复；已订阅状态由实心铃铛、强调色和确认文字表示。

Preferences save immediately and show Saved or an inline error with Retry. The
turn bell is hidden in All/Selected scope. Settings tabs carry an account icon
only when the entire tab is account-scoped: Notifications qualifies; Preferences
and Upstreams mix browser/device/account settings and do not. Other existing tabs
administer the selected device.

Relay stores settings and exact device/thread/turn subscriptions under the
currently authenticated account. The scope applies equally to Web Push, native
Relay push and the workbench notification list. Push still covers only devices
owned by that account (including threads created there by a guest); devices shared
*to* it do not gain push delivery. Workbench events retain their existing access
checks, and a scope/card/subscription never grants thread access.

Without a Relay account, the standalone local owner stores notification settings,
subscriptions and completed-event evidence in SQLite `kv`, alongside the existing
local access data, under `local:owner:*`. They are shared by browsers reaching that
local owner, not by independent Supervisors with independent local owners. Local
workbench shortcuts still use browser local storage; there was no existing
server-side local account preference table. Treer host-scoped pages do not expose
this account tab.

Enable **system notifications in this browser** separately in every browser that
should receive Relay updates. Browser permission requires an explicit click and
remains browser-specific. Pages may be closed after registering Web Push; local
mode uses the workbench bell. Signing out/revoking a Relay session still disables
that session's push subscriptions.

Notifications report a **completed or failed turn**, or a turn **awaiting input**, regardless of its provider.
Token streaming, tool progress, interruption and uncertain/recovering states do
not send notifications. Interruption ends a turn subscription without notifying.
Relay approval/question notices follow the same account scope, preserve the active
turn subscription, and use `deviceId:requestId` for deduplication. The standalone
local lifecycle feed continues to record terminal turns only. A terminal
turn suppresses delayed request notices; terminal notices still use `deviceId:turnId`.
The shared Rust `should_notify_turn` policy combines the account settings,
authenticated device, device-reported workspace and exact turn subscription. A
queued follow-up may already be working when the preceding turn's notice arrives.
Push notices contain a generic outcome and thread URL, without prompts, responses,
thread titles or workspace names.

Missing settings default to **Subscribed turns**, including accounts that used
notifications before scope settings existed. This deliberately changes the old
notify-every-turn default: old users must subscribe to working turns or choose All
turns. Browser push permissions/subscriptions are preserved. Old clients can still
register push but need updated Web assets to choose scope or subscribe. Old
Supervisors' events without `workspaceId` support turn subscriptions and device
cards; a workspace card cannot match missing metadata and never guesses from a
browser-supplied navigation reference.

## Account API

| Endpoint | Methods | Purpose |
| --- | --- | --- |
| `/relay/account/notifications/preferences` | GET, PUT | Account scope and device/workspace cards |
| `/relay/account/notifications/turns/{device}/{thread}/{turn}` | GET, PUT | Exact turn subscription (`subscribed` boolean) |
| `/relay/account/notifications` | GET | Existing browser/native push registration settings |
| `/api/account/notifications/preferences` | GET, PUT | Standalone local owner scope/cards |
| `/api/account/notifications/turns/{thread}/{turn}` | GET, PUT | Standalone local owner turn subscription |
| `/api/account/notifications/events` | GET | Local workbench events, filtered by the same policy |

Preferences use camelCase JSON: `scope` is `subscribed`, `all` or `selected`;
`devices` contains `{deviceId,name}` and `workspaces` contains
`{deviceId,workspaceId,name,deviceName}`. Standalone cards use `deviceId: "local"`.
There are at most 200 cards. Responses are not cached. All Relay endpoints require
account authentication; local endpoints inherit local authentication and cannot
be used to override a Relay account. A late subscription write after event
acceptance returns conflict. Ended subscriptions are no longer active; their
frozen evidence is retained only with the bounded event history so subsequent
workbench reads agree with push eligibility.

Clicking a notification focuses a tab already showing the same device/thread
path, ignoring query strings and fragments. Otherwise it opens a new tab. The
worker asks tabs for their live route: `WindowClient.url` can retain an older URL
after SPA navigation. A closed or unresponsive tab cannot reliably be focused;
after a short route-response timeout a new tab is the fallback.

Thread tab titles contain only the thread name. Their favicon carries the state:

| State | Favicon |
| --- | --- |
| Working | Amber open ring |
| Completed, unread | Red dot |
| Read, idle | Gray checkmark |

Read failures use a red cross; recovering threads use an amber question mark
rather than claiming completion. Read markers advance only when the thread page is
visible and focused after completion. They persist in this browser's local
storage and synchronize between its tabs. Other browsers have independent read
markers. Running state always takes precedence over older unread completions.
Tab status works for local Supervisor pages too and needs no push permission.

## Delivery and retention

1. Supervisor persists a terminal notice (including workspace identity) in its existing `kv` store, in the
   same transaction as the completed turn. Connected Supervisors retry pending
   notices over their authenticated Relay tunnel every heartbeat.
2. Relay determines the device owner from its authenticated device record. It
   atomically records the event, ends subscriptions and applies the account policy
   before fan-out to that owner's active browser subscriptions, then acknowledges
   the turn to remove the device's notice. Filtered events are still acknowledged
   and deduplicated; pending deliveries recheck the policy before sending.
3. A background Relay worker encrypts each payload with Web Push `aes128gcm`,
   signs VAPID, and sends it to the browser's push service. Browser pages do not
   need to stay open or maintain a connection to each Supervisor.
4. Relay deduplicates by device/turn. Pending deliveries retry with backoff for
   up to 24 hours. Expired notices are discarded; a new subscription does not
   receive older events. HTTP 404/410 removes a defunct subscription. A network
   ambiguity or process crash after acceptance can still cause redelivery;
   notification tags coalesce repeated deliveries of the same turn.

Relay SQLite contains `relay_push_subscriptions`, `relay_push_events`,
`relay_push_deliveries`, `relay_notification_preferences` and
`relay_turn_subscriptions`. Supervisor uses its existing `kv` table. Relay generates
its VAPID key pair once in `relay_settings`; include this database in backups to
preserve subscriptions. The private VAPID key is never returned by the API.

Subscriptions are bound to both the account and its authenticated login session.
Signing out, revoking the session, expiry, or disabling the account stops further
delivery. Re-enabling after login binds the browser subscription to the current
session/account and drops old pending deliveries. Disabling in the browser
removes its Relay subscription and unsubscribes from the push service. Messages
already accepted by a push service cannot be recalled.

Endpoints must be HTTPS URLs for supported browser push services (Google FCM,
Mozilla, Apple or Windows). Arbitrary endpoints and redirects are rejected, so
subscription registration cannot turn the Relay into an HTTP proxy.

## Browser and deployment requirements

- HTTPS, Service Workers, Push API and notification permission are required
  (localhost is suitable for development). On iOS/iPadOS, install the site on
  the Home Screen before enabling Web Push.
- Allow Pockymoe notifications in both browser and operating system settings.
  Focus/Do Not Disturb, power/network restrictions and push-service policies can
  delay or suppress display. Delivery after fully quitting a browser is outside
  this feature's guarantee.
- Deploy the updated Relay and Web assets together; update each Supervisor whose
  completions should generate events. Existing devices continue to work with an
  older Relay, but notifications require the new Relay handler.
- No push vendor API key or extra hosted service is required. The Relay must be
  able to reach browser push endpoints over outbound HTTPS.

## Validation

The focused Rust regressions exercise account isolation, exact-turn subscriptions,
device/workspace scope, missing legacy metadata, owner-only fan-out, share
exclusion, deduplication, session revocation, event expiry, payload encryption and
subscription endpoint restrictions. The workbench caps eligible notices after
filtering, so unrelated turns cannot evict subscribed notices. Runtime interaction
tests cover durable outbox read/ACK, subscriptions through recovery and explicit
interruption, and passive inbox messages producing no notice.

Vitest covers current SPA-route matching and focus/new-tab policy, plus persistent
background unread state. `e2e/notifications.spec.ts` uses an isolated real Relay
and fake-harness Supervisor with built Web assets. It exercises account enable/
disable, encrypted thread access, completion persistence/ACK, tab read state and
Chromium's Service Worker push dispatcher. Only browser vendor subscription
creation is mocked: automated Chromium has no vendor push credentials. This is
not a live FCM/APNs delivery test or an OS notification click test.

`e2e/notification-scope.spec.ts` exercises the local owner through two browser
contexts: refresh and cross-browser subscription persistence, turn completion,
late-write rejection, the next turn, saved scope and workspace cards with Undo.
Component tests cover optimistic save rollback and distinct read/write retries.

Build `cargo build -p pockymoe` and
`pnpm --filter @pockymoe/supervisor-web build` first. Run the spec with explicit
isolated `E2E_API_PORT`, `E2E_WEB_PORT`, `E2E_DATABASE_URL`, `E2E_WORKSPACE_ROOT`
and `--project=desktop-chromium`, following the focused-e2e skill.

## Native Apple push

`POST /relay/account/notifications/native` accepts optional
`appKind: "ios" | "macos-desktop"`; omitted kinds and older stored rows mean iOS.
The server selects `POCKYMOE_APNS_TOPIC` for iOS and
`POCKYMOE_APNS_DESKTOP_TOPIC` for desktop. Clients cannot choose a topic. Both
require `POCKYMOE_APNS_KEY_PATH`, `POCKYMOE_APNS_KEY_ID` and `POCKYMOE_APNS_TEAM_ID`.
The CLI adopts legacy `REMOTE_CODEX_*` environment names at startup as described
in [rename compatibility](rename-pockymoe.md). New names take precedence.

Registration settings add `nativePushKinds`; desktop checks for `macos-desktop`.
The legacy `nativePushAvailable` boolean keeps its iOS meaning. Subscription IDs
include kind, sandbox and token; renewing iOS replaces its legacy hashed ID.

APNs uses the event tag as `apns-collapse-id`, SHA-256/base64url hashed when longer
than 64 bytes. Its payload preserves full `tag`, `url`, `deviceId`, `threadId`,
legacy `eventId` (= tag), and `relayOrigin`. Bodies remain generic. The needs-input
body is `A thread needs your input. Click to respond.`; no command/question text
leaves the Supervisor. Older relays may ignore this status without affecting
request resolution. Unsent notices are removed on resolution; pushes already
accepted by a service cannot be recalled. Coalescing queued APNs notices does
not guarantee replacement of already-delivered alerts by Electron local notices.

## Live Activity updates

An iOS native registration may include `liveActivityToken`, or rotate it with
`PUT /relay/account/notifications/native/live-activity` (`{ id, token }`). The
registration must belong to the authenticated account **and current login
session**, and must be iOS. A token does not grant permission or create an activity;
the foreground shell starts one account activity and supplies its ActivityKit
push token. Registration renewal preserves that token; rebinding an APNs device
registration to another account/session drops its old activity and queued updates.
Logout, revocation and deletion cascade through both outboxes.

Supervisor heartbeats carry an explicit allowlist `threadActivity`: root
conversation IDs, state and state-entry timestamps. Agent threads are grouped with
their root; waiting wins over failure, running, interrupted and completed.
An optional `runningSince` records the earliest still-running member even when
a waiting/failed member takes display priority; that conversation also counts
as running, so priority cannot end an active conversation. The
Relay resolves ownership from the authenticated tunnel, never from these rows.
It persists no titles, device/workspace names, requests, commands or message text
in this channel. Updating state also sends a heartbeat, so requests and completed
turns need not wait for the periodic heartbeat. Older devices can omit the field.

Delivery uses `apns-push-type: liveactivity`, topic
`<POCKYMOE_APNS_TOPIC>.push-type.liveactivity`, the registration's sandbox setting,
and collapse ID `account-activity`. The ActivityKit payload is:

```json
{"aps":{"timestamp":1793606400,"event":"update","content-state":{"counts":{"running":1,"needsInput":0,"failed":0,"completed":0,"interrupted":0},"threads":[{"threadId":"00000000-0000-4000-8000-000000000000","state":"running","since":"2026-11-02T08:00:00Z"}],"updatedAt":"2026-11-02T08:00:00Z"}}}
```

Counts include all conversations; at most 20 prioritized thread rows fit the APNs
4 KiB payload limit. These rows contain only IDs, state and times. Titles remain
in the phone cache supplied by `activity.sync`; absent titles use generic text.
No `alert`, push-to-start attributes or remote approval controls are sent. With no
running/waiting conversations, `event` is `end` and `dismissal-date` is 15 minutes
after the update. Terminal rows older than 15 minutes are excluded; device snapshots
expire after 24 hours. A disconnected device retains its last snapshot until then.

The additive `relay_live_activity_tokens`, `relay_live_activity_devices` and
`relay_live_activity_outbox` tables preserve existing registrations and data.
The outbox coalesces to the newest account snapshot, retries with backoff, and
removes invalid activity tokens without disabling normal native notifications.
APNs acceptance is not a guarantee of system display. Local payload simulation
and mocked HTTP/bridge tests do not establish real-device APNs delivery.
