# Relay admin console

The relay administrator signs in at `/relay-admin` with the account from
`POCKYMOE_ADMIN_USERNAME` and `POCKYMOE_ADMIN_PASSWORD` (it has no account key;
it can add an authenticator or passkey under **Settings › Admin account**). The
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
| `relay_security_events` | See [security operations](relay-security-operations.zh.md#审计) | 90, 180 (default) or 365 days, at most 100,000 rows |

Retention is applied when the relay starts and then hourly; a shorter period
chosen in **Settings › Data retention and privacy** applies at once. Charts show
unknown, not zero, before recording started or while the relay was down, and say
where history begins when it is shorter than the range. Older devices answer
`/healthz` too; one that reports no version shows none.

A user's `last_seen_at` is written at most once a minute (it was written on every
request). A tunnel that was open when the relay stopped is ended at the last
sample (`relay_stopped`).

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
`q` (search) and `filter`, and return `{ items, total, page, pageSize }` plus
`counts` per filter.

| Route | Returns or does |
| --- | --- |
| `GET /relay/admin/overview?range=` | `attention`, `users` (with `dailyActive`), `activity`, `devices`, `shares`, `registrations`, `signupsWeekly`, `publicLinks`, `securityEvents`, `relay`, `onlineDevices` (series), `versions`, `tunnels`, `adminActions`. A series is `{ resolutionSeconds, points: [[ms, value or null]], historyStart }` |
| `GET /relay/admin/search?q=` | Up to six users and six devices by name, email or ID |
| `GET /relay/admin/users` · `GET /relay/admin/users/{id}` | Users list (`filter`: `active7d`, `noAccountKey`, `noTwoFactor`, `disabled`, `neverSeen`; `sort`: `lastSeen`, `name`, `created`) with `methods`; one user's devices, sessions, shares, events, approval |
| `PATCH` · `DELETE /relay/admin/users/{id}` | Enable/disable (not the admin) · delete (not the admin) |
| `POST /relay/admin/users/{id}/reset-password` | Temporary password of 12–1024 characters ([password resets](self-host-relay.md#password-resets)) |
| `DELETE /relay/admin/users/{id}/sessions` | End all sessions of a non-admin user |
| `GET /relay/admin/registrations?status=pending\|reviewed` | Requests with `summary` |
| `POST /relay/admin/registrations/{id}/approve` · `/reject` | Approve (creates the user) · reject (keeps the request, deletes its encrypted vault) |
| `GET /relay/admin/devices` · `GET /relay/admin/devices/{id}` | Devices (`filter`: `online`, `offline`, `unstable`, `unprotected`, `outdated`; `sort`: `name`, `owner`, `created`, `threads`) with `version`, `outdated`, `lastSeenAt`, `reconnects1h`, `unstable`, `traffic7d` · details with `connections` and `traffic` (24 hours) |
| `POST /relay/admin/devices/{id}/disconnect` | Drop the device's tunnel and browser sockets |
| `GET /relay/admin/shares` | Shares and grants (`filter`: `device`, `workspace`, `thread`, `expiring` within 7 days, `unused`) with `summary` and `usage` |
| `DELETE /relay/admin/shares/{share\|grant}/{id}` | Revoke at the relay |
| `GET /relay/admin/public-links?filter=legacy\|encrypted\|live` | Public links with `summary` |
| `GET /relay/admin/previews` | Port mappings, sessions and streams per device |
| `GET /relay/admin/security?range=` | Sign-ins, factor coverage, sessions, events per day, session environments, `loginFailures`, `rateLimited`, `retention`, `adminActions` |
| `GET /relay/admin/security/events?type=&user=&range=` | Audit events (`type`: `login`, `challenge`, `reauth`, `credential`, `failure`, `activity`) |
| `GET /relay/admin/sessions` · `DELETE /relay/admin/sessions/{id}` | Live sessions · end one (not the admin's) |
| `GET /relay/admin/system` | `relay`, `connections`, `checks`, `push` (with `delivery`), `storage` (with `growth`; integrity cached for an hour, table sizes for ten minutes), `versions`, `metrics` |
| `GET /relay/admin/actions?type=` | The admin action log |
| `GET` · `PATCH /relay/admin/settings/registration` | Registration settings |
| `GET` · `PATCH /relay/admin/settings/retention` | `metricsDays` (30, 90, 365) and `securityDays` (90, 180, 365) |
| `GET /relay/admin` | The single summary older Web builds read; it lists shares and grants without thread titles |

A Web UI that finds no console API (an older relay answers `404`) says so and
asks to update the relay.
