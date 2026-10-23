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

The console is a sidebar of sections on desktop and a bottom tab bar on phones
(Overview, Users, Devices, Security, and **More** for Performance, Server, Shares
and access, System and Settings). The sidebar folds into an icon rail (the button
beside the title; names show on hover and keyboard focus and stay the buttons'
accessible names); the browser remembers the choice. The top bar's tools wrap
under the title when they do not fit beside it.

Every section has a summary or chart band above a filtered, paged table; a row
opens its details in a drawer on the right (a sheet from the bottom on phones).
Every table sorts by its headers (on the relay, before paging), filters by
column with a row of filter pills, resizes its columns and hides or shows them
from **Columns**; widths, shown columns and the sort are kept per table in the
browser ([UI principles](ui-principles.md#管理后台的表格)). The URL holds the
section (`tab`), sub-tab (`sub`), open row (`id`), time range (`range`: `24h`,
`7d`, `30d`, `90d`, or `custom` with `from` and `to` days), the device drawer's
own range (`drange`: `7d` or `30d`; 24 hours when absent), the sort (`sort`,
`dir`) and each column filter, so a view can be linked.

One time range in the top bar applies to the whole page on Overview,
Performance, Server, Shares and access, Sessions and security and System:
every chart over time on that page follows it, and the range (custom windows
included) stays when switching between these sections. Cards that show the
present rather than the range (when people were last active, device versions)
carry a **Now** badge.

Every device is named with its owner, because names such as "mac" or "wsl"
exist for many people: in tables the device name sits over the owner's name;
the device name opens the device's details and the owner's name the person's,
with the owner's email as its title (and in the person's details, where touch
reaches it). Phone rows read "mac · alice". This holds for the device list,
the performance lists, port previews, shares, public links, security events
about a device, admin actions on a device and the overview's lists.

Code: `crates/relay/src/admin/` (API), `apps/supervisor-web/src/pages/relayAdmin/`
(console), strings in the shared UI's `admin` i18n namespace.

## What each section shows

| Section | Contents | Actions |
| --- | --- | --- |
| Performance | Forwarding over a range or custom window: requests, error rate and errors by kind (timeout, busy, offline, device error), latency p50/p95/p99 over time and the latency distribution, bytes to and from devices, tunnel reconnects over time, how long tunnels last, the devices that reconnect most and traffic per device (10 a page, sorted and paged by the relay; whole hours for two days or less, whole days beyond). Every chart has a legend, a tooltip and a table view; "How latency is measured" states the buckets | — |
| Server | Whether the relay's host keeps up, see [Server](#server) | — |
| Overview | Needs attention (registrations waiting, devices reconnecting often, accounts without an account key, legacy plaintext public links); figures for devices, active people (with daily active people), shares, account-key and second-factor coverage, registrations; online devices over the range; when people were last active (now); new accounts over the range (per hour, day or week); device versions; tunnels and forwarding (reconnects, error rate, p95 latency, with **Details** for Performance); recent security events and admin actions | Links to the lists behind each item |
| Users | People with status, account key, second factors, when they registered, their last sign-in and its browser ("Chrome · macOS"; no addresses), last activity, and as optional columns email, role, sign-in method, devices and the browser alone; filters by role, status, account key, second factor, sign-in method, registration time and last activity (including never), and search. Details: devices (at most 50, with a link to all of them on Devices), live sessions, recent security events, how the account was approved. Sub-tabs: registration requests (pending, review history, median time to approval) and sign-in methods | Enable/disable, temporary password, end all sessions or one, delete; approve/reject registrations |
| Devices | Tunnel state (online, offline with when it was last seen, reconnecting often), protection (device certificate), version (older than the relay is marked), threads, agent threads and workspaces counted from heartbeat thread IDs, port mappings, requests per day over 7 days, and as optional columns owner, last seen, reconnects in the last hour and when it was added; filters by owner (searchable), connection, protection, version (older, same or newer, not reported, or one version), last seen (including never) and added, and search. Details, over 24 hours, 7 or 30 days: tunnel connections (uptime, reconnects, longest offline, why each ended), forwarded requests per hour (per day beyond a day), upstream protocols (protocol only, never connection names), shares of the device, device ID | Disconnect the tunnel (the device reconnects by itself; no access changes) |
| Shares and access | Every active share as one list: thread shares and grants of a workspace or the whole device, with permissions, last use and expiry, filtered by scope, owner, recipient, permission, expiry, last use and creation; scope and permission summary; recipients' use over the range, per day (per hour for two days or less; approximate: visits repeat at most every 5 minutes and 100 are kept per share). Public links (encrypted or legacy plaintext, live or snapshot, size). Port previews held in memory | Revoke a share |
| Sessions and security | The relay-wide security audit (per day by category, filterable list); failed sign-ins per day; browsers and systems of live sessions; live sessions; the admin action log; rate-limit refusals by kind | End a session |
| System | Version, uptime, public address, Web origin, data directory; tunnels, browser sockets and requests in flight; configuration checks (database integrity, session secret, sign-in providers, port previews, APNs, legacy links); push subscriptions, delivery queue, delivery rate and turn results of the last day; device versions now and per day over the range; database, WAL and table sizes and database size over the range | — |
| Settings | Registration (open, approval, invite code; each applies to Google and GitHub sign-up too, see [OAuth and registration](auth-and-connectivity-modes.md)), Google and GitHub sign-in, data retention and privacy, the admin's own security | Each switch saves at once |

Every change asks first where it signs someone out or deletes something
(`confirmAsync`). Revoking a share as admin only stops the relay from forwarding
it: a protected device also holds the owner's signed grant, which only the owner
can delete; the owner's Sharing tab then shows it as still active on the device
([account key trust](account-key-trust.md#sharing)).

## Server

The **Server** section answers whether the relay is overloaded. Each figure has
a status that is a shape and a word as well as a colour: a check for normal, a
triangle (amber) for high, an octagon (red) for critical, a dashed circle for
not available. "What turns amber or red" lists every check with its value and
thresholds; each figure states in one line what it means. Charts follow the
page's range.

| Check | Measured as | High | Critical |
| --- | --- | --- | --- |
| Machine CPU | busy share of all cores, mean of the last 5 minutes | 80% | 95% |
| Relay process CPU | CPU time of the relay per second over the cores it may use (the cgroup's CPU quota, or every core), 5 minutes | 80% | 95% |
| Load | 5-minute load average per core | 1.0 | 2.0 |
| Memory | the service's cgroup memory against its limit when the cgroup has one, otherwise memory in use on the machine against its total | 85% | 95% |
| Swap in use | used swap of all swap | 50% | 90% |
| Disk | used space of the filesystem holding the data directory, as `df` counts it | 85% | 95% |
| Open files | the relay's open file descriptors against its soft limit | 80% | 95% |
| Write-ahead log | size of `relay-store.sqlite-wal` (checkpoints not keeping up) | 256 MB | 1 GB |
| Store lock held | share of each minute the one writing SQLite connection was held, 5 minutes | 50% | 80% |
| Longest store wait | the longest any caller waited for that connection, 5 minutes | 1 s | 5 s |
| Runtime delay | how late the async runtime woke a 500 ms timer, the longest in 5 minutes | 200 ms | 1 s |
| Tunnel send queue | the fullest device tunnel's queued messages against its capacity | 50% | 90% |

It also shows the relay process (process ID, uptime, resident and virtual
memory, CPU in cores, threads, open files, the cgroup and its limits), the host
(cores and each core's busy share over the last minute, load averages, memory,
swap, network throughput on every interface but loopback), the disk (size,
used, free, the database and its WAL), and the relay's own load (device
tunnels, browser connections, requests in flight, messages waiting in tunnel
and browser send queues, uses of the store lock per minute).

How it is measured: once a minute the sampler reads `/proc/self/status`,
`/proc/self/stat`, `/proc/self/fd`, `/proc/self/limits`, `/proc/stat`,
`/proc/loadavg`, `/proc/meminfo`, `/proc/net/dev` and the cgroup's
`memory.current`, `memory.max` and `cpu.max` (v1: `memory.usage_in_bytes`,
`memory.limit_in_bytes`, `cpu.cfs_quota_us`), and `statvfs` on the data
directory, all outside the store lock. Rates (CPU, network) are the difference
to the previous minute, so they appear after the second sample. The store lock
counts every wait and hold in memory; a 500 ms timer task measures the runtime
delay. On systems without `/proc` (macOS and others) only the disk, the
database and the relay's own load are measured; everything else reads "not
available". These are host figures only: nothing about people, devices or
their content.

## Load and reads at scale

Admin reports never hold the store lock that tunnels, heartbeats and sign-ins
need: they read on a second, read-only SQLite connection (WAL lets it read
while the main one writes) and run off the async worker threads. The integrity
check and table sizes run on their own connection in a blocking thread and are
cached. Lists filter, sort, count and page in SQL; per-device figures (threads,
last seen, traffic) are read for the page's rows only. Long ranges read day
rows, and every read seeks an index. An ignored test
(`admin_endpoints_on_a_large_relay` in `crates/relay/src/admin/load_tests.rs`)
seeds 10,000 people, 50,000 devices, 15,000 online and months of statistics and
checks every endpoint against a 400 ms budget in release mode.

## Recorded statistics

Code: `crates/relay/src/metrics.rs`. Counters live in memory; a sampler writes
them once a minute in one transaction, so a heartbeat or a forwarded request
never writes to SQLite by itself. The tables are created when missing and never
change existing ones.

| Table | What it holds | Kept |
| --- | --- | --- |
| `relay_metrics` | Per minute, hour and day: online devices, browser sockets, requests in flight, preview sessions and streams (means and peaks); forwarded requests, request and response bytes, failures (device offline, timeout, busy), device 5xx answers, a latency histogram, WebSocket messages and bytes, tunnel connections, failed sign-ins, rate-limit refusals by kind (`login`, `auth-ip`, `factor`, …), push results, database, main file and WAL size; for the Server section the relay process (`proc_rss`, `proc_vm`, `proc_threads`, `proc_fds`, `proc_cpu`), the host (`host_cpu`, `host_load1`, `host_load5`, `host_mem_used`, `host_mem_total`, `host_swap_used`, `net_rx`, `net_tx`), the cgroup (`cgroup_mem`, `cgroup_mem_max`), the disk (`disk_used`, `disk_available`) and the relay's load (`tunnel_queue`, `tunnel_queue_max`, `browser_queue`, `store_busy`, `store_wait_ms`, `store_hold_ms`, `store_locks`, `store_wait_max_ms`, `store_hold_max_ms`, `loop_lag_ms`). Per day also online devices per reported version. An `up` row marks every minute the relay sampled | Minutes 48 hours; hours and days the metrics retention: 30, 90 (default) or 365 days |
| `relay_device_traffic` | Per device and hour: requests, request and response bytes, errors | 30 days (less with a shorter metrics retention); deleted with the device |
| `relay_device_traffic_daily` | The same per device and day | Metrics retention; deleted with the device |
| `relay_device_lineage_counts` | Threads, agent threads and workspaces per device, written with each lineage snapshot so sorting by threads never counts the lineage table | Latest only; deleted with the device |
| `relay_public_link_stats` | Each public link's size and whether it is encrypted, kept by triggers so the console never parses a snapshot | Deleted with the link |
| `relay_device_connections` | Each tunnel connection: device, connect and end time, why it ended (`closed`, `timeout` after 90 s without frames, `replaced`, `queue_full`, `deleted`, `token_rotated`, `admin`, `send_failed`, `error`, `relay_stopped`) | 30 days, at most 200,000 rows; deleted with the device |
| `relay_device_versions` | The version a device's `/healthz` reports, read within a minute of each connection and every 6 hours | Latest only; deleted with the device |
| `relay_daily_active` | Which accounts made an authenticated request and which devices were online, per day | Metrics retention |
| `relay_admin_actions` | Every console change: admin, action, target and its name at the time, changed setting values (never the invite code or a password) | 365 days, at most 50,000 rows |
| `relay_user_sign_ins` | Each account's latest sign-in: when, and the browser label its session got ("Chrome · macOS"). Written by triggers on `relay_auth_sessions`, so it outlives the session; filled from live sessions when first created | Latest only; deleted with the account |
| `relay_security_events` | See [security operations](relay-security-operations.zh.md#审计) | 90, 180 (default) or 365 days, at most 100,000 rows |

A relay upgraded from a version without day rows, daily device traffic or
lineage counts builds them once from what it already holds at its first start.
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
agents. The Server section's figures describe the relay process and its host
only. Traffic of protected devices is counted as encrypted sizes; the relay
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

Every route that charts over time (`overview`, `performance`,
`performance/devices`, `server`, `shares`, `security`, `system`) takes `range`
or a custom window `from`/`to` (epoch ms, at most 400 days) and answers with
`window` (`range`, `start`, `end`, `resolutionSeconds`). Rows that name a
device carry `deviceId`, `name` (and `deviceName`), `ownerUserId`,
`ownerUsername` and `ownerEmail`.

| Route | Returns or does |
| --- | --- |
| `GET /relay/admin/overview?range=` | `attention`, `users` (with `dailyActive`), `activity`, `devices`, `shares`, `registrations`, `signupsWeekly`, `publicLinks`, `securityEvents`, `relay`, `onlineDevices` (series), `versions` (online and all devices per version, `outdated` when older than the relay), `tunnels`, `adminActions` (device targets with their owner), `signups` (new accounts over the window: `resolutionSeconds`, `points`, `total`), and `shares.createdInRange`/`revokedInRange`. A series is `{ resolutionSeconds, points: [[ms, value or null]], historyStart }` |
| `GET /relay/admin/search?q=` | Up to six users and six devices by name, email or ID (devices with their owner) |
| `GET /relay/admin/users` · `GET /relay/admin/users/{id}` | Users list (`filter`: `active7d`, `noAccountKey`, `noTwoFactor`, `disabled`, `neverSeen`; `role`: `admin`, `user`; `status`: `active`, `disabled`, `resetPending`; `key`: `set`, `missing`; `factor`: `any`, `none`, `authenticator`, `passkey`; `method`: `password`, `google`, `github`, `oauthOnly`; `seen…`, `created…`; `sort`: `name`, `email`, `role`, `status`, `created`, `lastSeen`, `lastSignIn`, `browser`, `devices`, `key`, `factors`, `method`) with `methods`, each user with `lastSignInAt` and `lastSignInBrowser`; one user's devices (at most 50, `devicesTotal`), sessions, shares, events, approval |
| `PATCH` · `DELETE /relay/admin/users/{id}` | Enable/disable (not the admin) · delete (not the admin) |
| `POST /relay/admin/users/{id}/reset-password` | Temporary password of 12–1024 characters ([password resets](self-host-relay.md#password-resets)) |
| `DELETE /relay/admin/users/{id}/sessions` | End all sessions of a non-admin user |
| `GET /relay/admin/registrations?status=pending\|reviewed` | Requests with `summary`; the review history sorts by `reviewed`, `requested`, `user`, `result` |
| `POST /relay/admin/registrations/{id}/approve` · `/reject` | Approve (creates the user) · reject (keeps the request, deletes its encrypted vault) |
| `GET /relay/admin/devices` · `GET /relay/admin/devices/{id}` | Devices (`filter`: `online`, `offline`, `unstable`, `unprotected`, `outdated`; `owner` (user ID); `state`: `online`, `connected`, `offline`, `unstable`; `protection`: `protected`, `unprotected`; `version`: `outdated`, `current`, `unknown` or `v:<version>`; `seen…` (a connected device is seen now), `created…`; `sort`: `name`, `owner`, `state`, `protection`, `version`, `threads`, `ports`, `created`, `lastSeen`, `reconnects`, `traffic`) with `version`, `outdated`, `lastSeenAt`, `reconnects1h`, `unstable`, `traffic7d`, and `versions` (each reported version with its device count); search also matches the owner's email · details with `connections` and `traffic` over `range` `24h` (hours), `7d` or `30d` (days) |
| `POST /relay/admin/devices/{id}/disconnect` | Drop the device's tunnel and browser sockets |
| `GET /relay/admin/shares` | Shares and grants (`filter`: `device`, `workspace`, `thread`, `expiring` within 7 days, `unused`; `scope`; `owner` and `recipient` (user IDs); `access`: `control`, `read`, `filesWrite`, `filesRead`, `createThreads`; `expiry`: `expiring`, `set`, `none`; `seen…` (last use), `created…`; `sort`: `scope`, `owner`, `recipient`, `device`, `access`, `lastUsed`, `expires`, `created`) with `summary` and `usage` over the window (7 days by default), rows with `ownerEmail` and `targetEmail` |
| `DELETE /relay/admin/shares/{share\|grant}/{id}` | Revoke at the relay |
| `GET /relay/admin/public-links?filter=legacy\|encrypted\|live` | Public links with `summary` (`sort`: `created`, `owner`, `device`, `storage`, `live`, `refreshed`, `size`) |
| `GET /relay/admin/previews` | Port mappings, sessions and streams per device, busiest first, at most 200 (`devicesTotal`) |
| `GET /relay/admin/security?range=` | Sign-ins (window), factor coverage, sessions, events per day, session environments, `loginFailures`, `rateLimited`, `retention`, `adminActions` |
| `GET /relay/admin/security/events?type=&user=&range=` | Audit events (`type`: `login`, `challenge`, `reauth`, `credential`, `failure`, `activity`; `from`/`to` instead of `range`; `sort`: `time`, `event`, `user`) |
| `GET /relay/admin/sessions` · `DELETE /relay/admin/sessions/{id}` | Live sessions (`user`, `created…`; `sort`: `created`, `expires`, `user`, `environment`, `verified`) · end one (not the admin's) |
| `GET /relay/admin/system` | `relay`, `connections`, `checks`, `push` (with `delivery`), `storage` (with `growth` over the window, 30 days by default; integrity cached for an hour, table sizes for ten minutes), `versions` (`current`, and `history` per day of the window: `null` on a day the relay did not sample), `metrics` |
| `GET /relay/admin/actions?type=` | The admin action log (`from`/`to`; `sort`: `time`, `action`, `admin`, `target`); device targets with `targetOwnerUserId`, `targetOwnerUsername`, `targetOwnerEmail` |
| `GET /relay/admin/performance?range=` (or `from=&to=`) | Forwarding over a window, in steps of 5 minutes to a day (about 200 points): `totals` (requests, `errors` by kind, `errorRate`, bytes, reconnects, WebSocket messages, `p50Ms`, `p95Ms`, `p99Ms`), `series` (`requests`, `errors.{timeout,busy,offline,other}`, `errorRate`, `p50`, `p95`, `p99`, `reconnects`, `onlineDevices`, `requestBytes`, `responseBytes`), `latency` (`boundsMs`, `openAboveMs`), `latencyHistogram` (`fromMs`, `toMs` or null for the open bucket, `count`), `connections` (tunnel `durations` of connections that ended in the window, `open`, `connects`, the first 10 `topDevices` by reconnects with `shortConnections` (opened in the window, ended under 10 minutes later) and `topDevicesTotal`; `historyStart` when the 30-day connection log is shorter than the window) and `deviceTraffic` (the first 10 devices by requests with errors and bytes, `total` devices and all their `requests`; whole hours for two days or less, whole days beyond) |
| `GET /relay/admin/performance/devices?view=` | One page of a per-device list: `view=reconnects` (`sort`: `connects`, `short`, `device`) or `view=traffic` (`sort`: `requests`, `errors`, `rate`, `sent`, `received`, `device`), with `page`, `pageSize`, `dir` |
| `GET /relay/admin/server?range=` | `platform`, `sampledAt`, `status` (the worst check), `process`, `host` (with `perCore`), `cgroup`, `memoryScope` (`cgroup` or `host`), `cpuCapacityCores`, `disk`, `relay`, `checks` (`key`, `status`: `ok`, `warn`, `critical`, `unknown`; `value`, `warnAt`, `criticalAt`), `series` (each a series over the window) |
| `GET` · `PATCH /relay/admin/settings/registration` | Registration settings |
| `GET` · `PATCH /relay/admin/settings/retention` | `metricsDays` (30, 90, 365) and `securityDays` (90, 180, 365) |
| `GET /relay/admin` | The single summary older Web builds read; it lists shares and grants without thread titles, at most 500 rows per list (`truncated` when a list is cut) |

A Web UI that finds no console API (an older relay answers `404`) says so and
asks to update the relay.
