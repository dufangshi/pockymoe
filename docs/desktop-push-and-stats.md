# Desktop push and device statistics

## Native subscriptions

`POST /relay/account/notifications/native` accepts the existing authenticated
APNs subscription plus optional `appKind: "ios" | "macos-desktop"`. Omitted kinds
(including existing stored JSON) mean `ios`. Unknown kinds are rejected. Clients
cannot choose an APNs topic: iOS uses `REMOTE_CODEX_APNS_TOPIC`, macOS desktop uses
`REMOTE_CODEX_APNS_DESKTOP_TOPIC`. Both require the existing APNs key path, key ID,
and team ID configuration. Configure the topic matching the signed app's bundle
identifier. An unconfigured kind returns 503 without altering a subscription.

`GET /relay/account/notifications` adds `nativePushKinds`, the list of configured
kinds. The legacy `nativePushAvailable` remains the iOS availability boolean so
existing iOS clients retain their existing meaning. New desktop clients must
check `nativePushKinds.includes('macos-desktop')`.

Subscription identity includes kind, sandbox and case-normalized token. Renewal
of an iOS subscription removes its legacy ID transactionally, including pending
old deliveries. Account/session ownership and revocation remain unchanged.

APNs delivery includes `apns-collapse-id`: the event tag when it fits 64 ASCII
bytes, otherwise its SHA-256 base64url digest. Payloads preserve the full original
`tag`, `url`, `deviceId`, `threadId`, legacy `eventId` (= tag), and `relayOrigin`.
Bodies are generic; neither command/question text nor thread titles are sent to
the relay. APNs collapse coalesces pending remote deliveries; replacing an
already-delivered APNs alert with an Electron local notification is a separate
platform behavior and is not guaranteed by this server change.

## Needs-input outbox

The supervisor persists `awaiting_input` when `thread.request.created` has a
request ID and belongs to an in-progress/recovering turn. ACP emits that event
only when approval or user input is actually waiting (auto-approved permissions
do not emit it). Approval request IDs use UUIDs to avoid process-local ID reuse.
It sends a best-effort immediate tunnel notice and retries through the existing
24-hour durable outbox. Resolution removes an unsent notice. Already-fanned-out
pushes cannot be recalled. A supervisor restart does not recreate stale action
requests; preexisting pending notices expire normally.

Terminal events retain tags `deviceId:turnId`; awaiting-input tags are
`deviceId:requestId`. The relay deduplicates by that tag and returns the event
key in the existing `relay.notification.ack.turnId` field (a request ID for
awaiting-input). The payload carries the actual turn ID separately. Rejection
or absent acknowledgement by an older relay is non-fatal: encrypted operation
and approval responses continue independently; only the bounded notice retries.

The fixed body is `A thread needs your input. Click to respond.`. The Web Push
worker (`apps/supervisor-web/src/lib/notificationWorker.ts`) and iOS APNs delegate
(`ios/RemoteCodex/App/RemoteCodexApp.swift` in the app repo) display the supplied
body and route by URL or device/thread IDs without status filtering. No change
is needed in either client for offline push display. iOS foreground polling has
its own lifecycle notification path; this change does not add requests there.

## Stats summary

`GET /api/stats/summary?range=today|7d|30d&tz=<timezone>` is an authenticated
supervisor API, available through the usual encrypted relay device `/api/*`
transport. It is not an account-wide relay aggregation. Health responses advertise
`capabilities.statsSummary: true`; older supervisors omit that capability and
return 404 for the endpoint. Clients should feature-detect and retain partial-data
fallbacks. `range` defaults to `today`, `tz` defaults to `UTC`. Invalid values
return 400.

Timezones accept IANA names (including historical DST transitions) or signed
integer minutes **east of UTC** from -840 to 840. This is the inverse of
JavaScript's `Date.getTimezoneOffset()`. Percent-encode query values normally.
Ranges are calendar days including today: 7d means today plus six prior dates.
Turns are assigned to their start date in the requested timezone. Future starts,
missing/invalid start timestamps, and deleted threads are excluded. Imported
turns already in SQLite count; requesting stats never scans external transcripts.

Counts include running turns; completed/failed/interrupted are terminal subsets.
Durations include terminal turns with valid start/end times only, including time
waiting for tools/input. `totals.durationSamples` counts these turns (zero when
empty), so clients can weight average durations across devices exactly. Average and p50 are null with no durations. p50 averages
the middle pair for even samples. `longestThreads` is the top five threads by
**mean** terminal-turn duration in the range; `durationMs` is that mean. This
matches the desktop's "threads with longest average duration" chart.

Tokens reuse normalized persisted usage (`inputTokens` includes cached input;
`cachedTokens` means cache reads and is a subset of input, not an extra token
count). Costs reuse the existing model pricing helper, user overrides, model
aliases, accumulated per-request estimates, tiers and time-sensitive rates.
An aggregate `costUsd` is null if any included turn is unpriced or has unknown
usage; empty aggregates cost zero. Missing token usage contributes zero known
tokens, so totals can be partial. Daily buckets are zero-filled and ascending;
model/workspace rows sort by turns descending, with ID tie-breaks. Workspace
last activity is the latest included completion/start. Current `activeThreads`
(running/recovering, open) and `runningTurns` (inProgress/recovering) are device-wide,
independent of range.

Migration 11 adds an idempotent `julianday(started_at)` SQLite expression index,
so offset timestamps and UTC timestamps use the same indexed range scan. The
summary joins only turn/thread/workspace rows and never reads history items.
HTTP computes in a blocking worker instead of blocking the async reactor.
The focused production latency benchmark runs with
`cargo test --release -p remote-codex-runtime stats_indexed_five_thousand_turns_under_150ms --lib -- --ignored --nocapture`;
it is intentionally ignored in debug test runs because unoptimized code is not
a production latency measurement.

## Device rename

`PATCH /relay/devices/:deviceId` accepts `{ "name": "Work Laptop" }` for the
signed-in **owning user only**. Device/thread/workspace control grants, thread
shares and hosted membership do not confer ownership.
Non-owners receive 403, unauthenticated requests receive 401, and missing devices
receive 404. Names are trimmed, must contain 1–64 Unicode characters after trimming,
and must contain no control characters (including leading/trailing tabs/newlines).
Invalid names receive 400.

The response is the updated `RelayDeviceDto` directly (no `device` wrapper), with
the same camelCase fields and presence/hosted metadata as an entry in
`GET /relay/devices`. It does not return device credentials. This updates the
relay display name only and does not restart or reconnect a device.

## TypeScript DTOs for consumers

No changes to `packages/shared` or the frozen desktop bridge are required here.
The following types describe the added HTTP fields for routing by the parent:

```ts
export type NativePushKind = 'ios' | 'macos-desktop';
export interface NativePushSubscription {
  nativePlatform: 'apns';
  deviceToken: string;
  relayOrigin: string;
  sandbox?: boolean;
  appKind?: NativePushKind; // omitted = ios
}
export interface NotificationSettingsAdditions {
  nativePushAvailable: boolean; // legacy iOS availability
  nativePushKinds: NativePushKind[];
}
export interface RelayNotificationEvent {
  threadId: string;
  turnId: string;
  status: 'completed' | 'failed' | 'awaiting_input';
  requestId?: string; // required for awaiting_input
  occurredAt: string;
}
export interface PushRoutingPayload {
  tag: string; // deviceId:turnId, or deviceId:requestId
  url: string;
  deviceId: string;
  threadId: string;
  // APNs additionally preserves eventId (= tag) and relayOrigin.
}
export interface StatsSummary {
  range: 'today' | '7d' | '30d';
  generatedAt: string;
  totals: {
    turns: number;
    completed: number;
    failed: number;
    interrupted: number;
    durationSamples: number;
    avgDurationMs: number | null;
    p50DurationMs: number | null;
    inputTokens: number;
    outputTokens: number;
    cachedTokens: number;
    costUsd: number | null;
  };
  daily: Array<{
    date: string; // YYYY-MM-DD in requested timezone
    turns: number;
    failed: number;
    tokens: number;
    costUsd: number | null;
  }>;
  byModel: Array<{
    model: string;
    turns: number;
    tokens: number;
    costUsd: number | null;
  }>;
  byWorkspace: Array<{
    workspaceId: string;
    label: string;
    turns: number;
    tokens: number;
    costUsd: number | null;
    lastActivityAt: string;
  }>;
  longestThreads: Array<{
    threadId: string;
    title: string;
    workspaceId: string;
    durationMs: number; // mean terminal-turn duration
  }>;
  activeThreads: number;
  runningTurns: number;
}
```
