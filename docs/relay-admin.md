# Relay admin console

The relay administrator signs in at `/relay-admin` with the account from
`POCKYMOE_ADMIN_USERNAME` and `POCKYMOE_ADMIN_PASSWORD` (it has no account key;
it can add an authenticator or passkey under **Settings › Admin account**).

## Signing in

- `/relay-admin` has its own sign-in card, **Relay admin sign-in**: the
  administrator's name and password, then a second factor when it has one.
  A wrong name or password says so and points people with a relay account to
  **User sign-in** (`/relay-portal`), which the card links.
- The user sign-in links **Admin console** under its form. An administrator who
  signs in there anyway is taken to the console instead of being refused; a
  browser that still holds an administrator session from before sees "Signed in
  as the relay administrator" with **Open the admin console** and **Sign out**,
  rather than being signed out silently.
- The console session lives only in the `remote_codex_relay_admin_session`
  cookie (sent with the `X-Remote-Codex-Auth-Realm: admin` header); the
  administrator never gets the ordinary `remote_codex_relay_session` cookie, so
  signing in or out as a user in the same browser does not touch it.
- The console and its sign-in always show the Classic look, light or dark as the
  browser's color mode says, whatever theme pack the browser chose; leaving the
  console brings the pack back ([theme packs](theme-packs.md)).

## Layout

The
console is a sidebar of sections on desktop and a bottom tab bar on phones
(Overview, Users, Devices, Security, and **More** for Shares and access, System
and Settings). Every section has a summary or chart band above a filtered,
paged table; a row opens its details in a drawer on the right (a sheet from the
bottom on phones). The URL holds the section (`tab`), sub-tab (`sub`), filter,
open row (`id`) and time range (`range`: `24h`, `7d`, `30d`, `90d`).

Code: `crates/relay/src/admin/` (API), `apps/supervisor-web/src/pages/relayAdmin/`
(console), strings in the shared UI's `admin` i18n namespace.

## What each section shows

| Section | Contents | Actions |
| --- | --- | --- |
| Overview | Needs attention (registrations waiting, devices reconnecting often, accounts without an account key, legacy plaintext public links); figures for devices, active people (with daily active people), shares, account-key and second-factor coverage, registrations; online devices over the range; when people were last active; new accounts per week; device versions; tunnels and forwarding (reconnects, error rate, p95 latency); recent security events and admin actions | Links to the lists behind each item |
| Users | People with sign-in method, account key, second factors, devices online and last activity; filters (active in 7 days, no account key, no second factor, disabled, never signed in) and search. Details: devices, live sessions, recent security events, how the account was approved. Sub-tabs: registration requests (pending, review history, median time to approval) and sign-in methods | Enable/disable, temporary password, end all sessions or one, delete; approve/reject registrations |
| Devices | Tunnel state (online, offline with when it was last seen, reconnecting often), protection (device certificate), version (older than the relay is marked), threads, agent threads and workspaces counted from heartbeat thread IDs, port mappings, requests per day over 7 days; filters and search. Details: the last 24 hours of tunnel connections (uptime, reconnects, longest offline, why each ended), forwarded requests per hour, upstream protocols (protocol only, never connection names), shares of the device, device ID | Disconnect the tunnel (the device reconnects by itself; no access changes) |
| Shares and access | Every active share as one list: thread shares and grants of a workspace or the whole device, with permissions, last use and expiry; scope and permission summary; recipients' use per day (approximate: visits repeat at most every 5 minutes and 100 are kept per share). Public links (encrypted or legacy plaintext, live or snapshot, size). Port previews held in memory | Revoke a share |
| Sessions and security | The relay-wide security audit (per day by category, filterable list); failed sign-ins per day; browsers and systems of live sessions; live sessions; the admin action log; rate-limit refusals by kind | End a session |
| System | Version, uptime, public address, Web origin, data directory; tunnels, browser sockets and requests in flight; configuration checks (database integrity, session secret, sign-in providers, port previews, APNs, legacy links); push subscriptions, delivery queue, delivery rate and turn results of the last day; device versions now and per day; database, WAL and table sizes and database size over 30 days | — |
| Settings | Registration (open, approval, invite code), Google and GitHub sign-in, data retention and privacy, the admin's own security | Each switch saves at once |

Every change asks first where it signs someone out or deletes something
(`confirmAsync`). Revoking a share as admin only stops the relay from forwarding
it: a protected device also holds the owner's signed grant, which only the owner
can delete; the owner's Sharing tab then shows it as still active on the device
([account key trust](account-key-trust.md#sharing)).

## Recorded statistics

Code: `crates/relay/src/metrics.rs`. Counters live in memory; a sampler writes
them once a minute in one transaction, so a heartbeat or a forwarded request
never writes to SQLite by itself. The tables are created when missing and never
change existing ones.

| Table | What it holds | Kept |
| --- | --- | --- |
| `relay_metrics` | Per minute and per hour: online devices, browser sockets, requests in flight, preview sessions and streams (means and peaks); forwarded requests, request and response bytes, failures (device offline, timeout, busy), device 5xx answers, a latency histogram, WebSocket messages and bytes, tunnel connections, failed sign-ins, rate-limit refusals by kind (`login`, `auth-ip`, `factor`, …), push results, database size. Per day: online devices per reported version. An `up` row marks every minute the relay sampled | Minutes 48 hours; hours and days the metrics retention: 30, 90 (default) or 365 days |
| `relay_device_traffic` | Per device and hour: requests, request and response bytes, errors | Metrics retention; deleted with the device |
| `relay_device_connections` | Each tunnel connection: device, connect and end time, why it ended (`closed`, `timeout` after 90 s without frames, `replaced`, `queue_full`, `deleted`, `token_rotated`, `admin`, `send_failed`, `error`, `relay_stopped`) | 30 days, at most 200,000 rows; deleted with the device |
| `relay_device_versions` | The version a device's `/healthz` reports, read within a minute of each connection and every 6 hours | Latest only; deleted with the device |
| `relay_daily_active` | Which accounts made an authenticated request and which devices were online, per day | Metrics retention |
| `relay_admin_actions` | Every console change: admin, action, target and its name at the time, changed setting values (never the invite code or a password) | 365 days, at most 50,000 rows |
| `relay_user_sign_ins` | Each account's latest sign-in: when, and the browser label its session got ("Chrome · macOS"). Written by triggers on `relay_auth_sessions`, so it outlives the session; filled from live sessions when first created | Latest only; deleted with the account |
| `relay_security_events` | See [security operations](relay-security-operations.zh.md#审计) | 90, 180 (default) or 365 days, at most 100,000 rows |

Retention is applied when the relay starts and then hourly; a shorter period
chosen in **Settings › Data retention and privacy** applies at once. Charts show
unknown, not zero, before recording started or while the relay was down, and say
where history begins when it is shorter than the range. Older devices answer
`/healthz` too; one that reports no version shows none.

A user's `last_seen_at` is written at most once a minute (it was written on every
request). A tunnel that was open when the relay stopped is ended at the last
sample (`relay_stopped`).

### Latency

Forwarding latency is the time from the relay sending a request down a device's
tunnel to its answer, so it includes the device's own work. Each answer is
counted in a histogram with upper bounds of 5, 10, 20, 30, 50, 75, 100, 150,
200, 300, 400, 500, 650, 800 ms, 1, 1.25, 1.5, 2, 2.5, 3, 4, 5, 7.5, 10, 20 and
30 s, and one open bucket above. A percentile (p50, p95, p99) is interpolated
linearly inside the bucket that holds it, so it is off by at most that bucket's
width; one in the open bucket is shown as "≥ 30 s". Relays before this change
recorded fewer bounds and reported the upper bound of the bucket itself, which is
why a p95 could read exactly 1,000 ms; their rows still read correctly. Requests
the relay refused (device offline, busy) or that timed out have no latency and
count as errors instead.

Errors are counted by kind: `timeout` (the device did not answer in time),
`busy` (too many requests were waiting for it), `offline` (no tunnel) and
`other` (the device answered with a 5xx status). The error rate is all four over
all forwarded requests.

## Privacy

Admin views never carry conversation content, thread titles or the names of a
device's upstream connections. Shares show the workspace label and the device
name; public links show owner, device, dates, size and whether the stored copy is
encrypted. The security audit holds account ID, event name, resource ID and time
only: no passwords, tokens, one-time codes, request bodies, IP addresses or raw
user agents. The statistics hold counts, sizes, durations and times: no request
or response bodies, paths, thread or workspace IDs, titles, IP addresses or user
agents. Traffic of protected devices is counted as encrypted sizes; the relay
cannot read it. A failed sign-in is attributed to an account only when it
exists; the name that was tried is not kept.

## API

All routes require the relay admin session (bearer token or the
`remote_codex_relay_admin_session` cookie); a normal user's session gets `401`.
JSON is camelCase. Lists take `page` (from 1), `pageSize` (1–100, default 25),
`q` (search), `filter`, `sort` with `dir` (`asc` or `desc`) and the column
filters below, and return `{ items, total, page, pageSize }` plus `counts` per
filter and, where a filter names a person, `labels` with that person's name.
Column filters that take a time use epoch milliseconds: `seenFrom`/`seenTo`
(last seen or last used; `seen=never` for never), `createdFrom`/`createdTo`,
and `from`/`to` (an event's or action's time). Sorting is done by the relay on
the whole list, before paging.

| Route | Returns or does |
| --- | --- |
| `GET /relay/admin/overview?range=` | `attention`, `users` (with `dailyActive`), `activity`, `devices`, `shares`, `registrations`, `signupsWeekly`, `publicLinks`, `securityEvents`, `relay`, `onlineDevices` (series), `versions` (online and all devices per version, `outdated` when older than the relay), `tunnels`, `adminActions`. A series is `{ resolutionSeconds, points: [[ms, value or null]], historyStart }` |
| `GET /relay/admin/search?q=` | Up to six users and six devices by name, email or ID |
| `GET /relay/admin/users` · `GET /relay/admin/users/{id}` | Users list (`filter`: `active7d`, `noAccountKey`, `noTwoFactor`, `disabled`, `neverSeen`; `role`: `admin`, `user`; `status`: `active`, `disabled`, `resetPending`; `key`: `set`, `missing`; `factor`: `any`, `none`, `authenticator`, `passkey`; `method`: `password`, `google`, `github`, `oauthOnly`; `seen…`, `created…`; `sort`: `name`, `email`, `role`, `status`, `created`, `lastSeen`, `lastSignIn`, `browser`, `devices`, `key`, `factors`, `method`) with `methods`, each user with `lastSignInAt` and `lastSignInBrowser`; one user's devices, sessions, shares, events, approval |
| `PATCH` · `DELETE /relay/admin/users/{id}` | Enable/disable (not the admin) · delete (not the admin) |
| `POST /relay/admin/users/{id}/reset-password` | Temporary password of 12–1024 characters ([password resets](self-host-relay.md#password-resets)) |
| `DELETE /relay/admin/users/{id}/sessions` | End all sessions of a non-admin user |
| `GET /relay/admin/registrations?status=pending\|reviewed` | Requests with `summary`; the review history sorts by `reviewed`, `requested`, `user`, `result` |
| `POST /relay/admin/registrations/{id}/approve` · `/reject` | Approve (creates the user) · reject (keeps the request, deletes its encrypted vault) |
| `GET /relay/admin/devices` · `GET /relay/admin/devices/{id}` | Devices (`filter`: `online`, `offline`, `unstable`, `unprotected`, `outdated`; `owner` (user ID); `state`: `online`, `connected`, `offline`, `unstable`; `protection`: `protected`, `unprotected`; `version`: `outdated`, `current`, `unknown` or `v:<version>`; `seen…` (a connected device is seen now), `created…`; `sort`: `name`, `owner`, `state`, `protection`, `version`, `threads`, `ports`, `created`, `lastSeen`, `reconnects`, `traffic`) with `version`, `outdated`, `lastSeenAt`, `reconnects1h`, `unstable`, `traffic7d`, and `versions` (each reported version with its device count) · details with `connections` and `traffic` (24 hours) |
| `POST /relay/admin/devices/{id}/disconnect` | Drop the device's tunnel and browser sockets |
| `GET /relay/admin/shares` | Shares and grants (`filter`: `device`, `workspace`, `thread`, `expiring` within 7 days, `unused`; `scope`; `owner` and `recipient` (user IDs); `access`: `control`, `read`, `filesWrite`, `filesRead`, `createThreads`; `expiry`: `expiring`, `set`, `none`; `seen…` (last use), `created…`; `sort`: `scope`, `owner`, `recipient`, `device`, `access`, `lastUsed`, `expires`, `created`) with `summary` and `usage` |
| `DELETE /relay/admin/shares/{share\|grant}/{id}` | Revoke at the relay |
| `GET /relay/admin/public-links?filter=legacy\|encrypted\|live` | Public links with `summary` (`sort`: `created`, `owner`, `device`, `storage`, `live`, `refreshed`, `size`) |
| `GET /relay/admin/previews` | Port mappings, sessions and streams per device |
| `GET /relay/admin/security?range=` | Sign-ins, factor coverage, sessions, events per day, session environments, `loginFailures`, `rateLimited`, `retention`, `adminActions` |
| `GET /relay/admin/security/events?type=&user=&range=` | Audit events (`type`: `login`, `challenge`, `reauth`, `credential`, `failure`, `activity`; `from`/`to` instead of `range`; `sort`: `time`, `event`, `user`) |
| `GET /relay/admin/sessions` · `DELETE /relay/admin/sessions/{id}` | Live sessions (`user`, `created…`; `sort`: `created`, `expires`, `user`, `environment`, `verified`) · end one (not the admin's) |
| `GET /relay/admin/system` | `relay`, `connections`, `checks`, `push` (with `delivery`), `storage` (with `growth`; integrity cached for an hour, table sizes for ten minutes), `versions` (`current`, and `history` per day: `null` on a day the relay did not sample), `metrics` |
| `GET /relay/admin/actions?type=` | The admin action log (`from`/`to`; `sort`: `time`, `action`, `admin`, `target`) |
| `GET /relay/admin/performance?range=` (or `from=&to=`) | Forwarding over a window, in steps of 5 minutes to a day (about 200 points): `totals` (requests, `errors` by kind, `errorRate`, bytes, reconnects, WebSocket messages, `p50Ms`, `p95Ms`, `p99Ms`), `series` (`requests`, `errors.{timeout,busy,offline,other}`, `errorRate`, `p50`, `p95`, `p99`, `reconnects`, `onlineDevices`, `requestBytes`, `responseBytes`), `latency` (`boundsMs`, `openAboveMs`), `latencyHistogram` (`fromMs`, `toMs` or null for the open bucket, `count`), `connections` (tunnel `durations` of connections that ended in the window, `open`, `connects`, `topDevices` by reconnects with `shortConnections` under 10 minutes; `historyStart` when the 30-day connection log is shorter than the window) and `deviceTraffic` (up to 100 devices by requests, with errors and bytes, in whole hours) |
| `GET` · `PATCH /relay/admin/settings/registration` | Registration settings |
| `GET` · `PATCH /relay/admin/settings/retention` | `metricsDays` (30, 90, 365) and `securityDays` (90, 180, 365) |
| `GET /relay/admin` | The single summary older Web builds read; it lists shares and grants without thread titles |

A Web UI that finds no console API (an older relay answers `404`) says so and
asks to update the relay.
