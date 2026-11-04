# Device turn statistics

`GET /api/stats/summary?range=today|7d|30d|all&tz=<timezone>&origin=<origin>`
reports this Supervisor's **persisted** execution ledger. It is available through
normal encrypted relay device transport, to the owner only. Shared grants and
Treer thread-scoped viewers cannot read device-wide totals. Health advertises
`capabilities.statsSummary: true`; clients must also handle older devices' 404
responses and missing additive fields. Defaults are `today`, `UTC`, `all` origins.
Invalid range, timezone or origin returns 400.

## Turns and initiating origin

A turn is one admitted harness execution in one thread. It is not an API request,
a chat message, a tool call, a steering message or a subscription allowance unit.
A tool-heavy turn can contain many model requests. All origins count by default:

| Origin | Authoritative evidence at admission |
| --- | --- |
| `owner` | Local/device-owner prompt or goal, with no verified non-owner sender |
| `guest` | Verified share recipient recorded on the accepted prompt or goal |
| `agent` | Parent/delegate or peer message, or an explicit Supervisor delegate wake |
| `automation` | Automation run bound to the queued prompt, or an explicitly scheduled native Claude turn recovered from its transcript |
| `unknown` | Pre-migration rows and imported executions without trustworthy initiating identity |

`origin` is the initiator of the execution. Steering by another source does not
change the already-running turn's origin. Queue acceptance alone is not a turn;
a cancelled pending prompt and a command-only hook create no turn. A delegate
execution is counted in its own thread; it is not also a parent execution.
Resume/connect/recovery observes or continues the same turn ID, and adds no turn
unless a new prompt actually starts another execution. Interruptions and confirmed
failures count; `recovering` means completion is unconfirmed, not success/failure.

Migration 21 adds `thread_turns.origin` with default `unknown`. It **does not guess
old origins** from prompt text, titles, thread lineage or current ownership.
Per-turn responses expose `origin`; older devices may omit it. The query's origin
filter applies to totals, daily/hourly, models, workspaces, longest threads and
origin groups. Device-wide `activeThreads` and `runningTurns` are current counts
and remain independent of the historical range/origin filter.

Forked history is context, not a fresh execution. New fork copies retain source
IDs in `history_copy_of`; summary and activity exclude those copies while the
conversation still displays them. Old unmarked copies cannot be distinguished
safely and are not guessed away. A subsequent execution in a fork counts normally.

## Time, counts and duration

Timezone accepts an IANA name or integer minutes **east of UTC**, -840 to 840
(the inverse of JavaScript `Date.getTimezoneOffset()`). A finite range contains
today plus zero, six or 29 prior local dates, assigned by execution **start time**.
Future starts are excluded. Undated/invalid starts cannot be assigned to a finite
range; `coverage.excludedUndatedTurns` reports their device-wide count for the
selected origin without pretending to know whether they fall inside the range.

`all` includes undated persisted turns in its totals/model/workspace/origin
breakdowns. `coverage.undatedTurns` counts them. They have no daily/hourly bucket.
Finite daily series is zero-filled in local-date order; `all` returns only dates
with recorded executions. The totals can therefore exceed the sum of daily rows
by the number of undated turns. Deleted threads and their orphan records are
excluded. No unrelated devices are included; the client aggregates devices.

`today` adds 24 zero-filled local-clock `hourly` rows (`hour: 0..23`). Both repeated
fall-back hours share their clock-hour bucket; a skipped spring-forward hour stays
zero. Fractional offsets use the local clock hour. Other ranges omit `hourly`.

`completed`, `failed`, `interrupted` are terminal subsets of `turns`; running and
recovering turns remain in the total. `durationSamples` counts terminal executions
with valid, ordered start/end timestamps. Average and median p50 use only those
samples; p50 averages the middle pair for even samples. No valid durations means
null, not zero. Duration is wall time including tools, user input and waits, not
model decoding time. An import with no final time contributes a turn but no
duration; known completion times are preserved during import.

`longestThreads` contains the top five threads by **mean** valid terminal-turn
duration in the selected range. `durationMs` is that mean. Workspace
`lastActivityAt` is the latest selected completion/start, or null if all included
turns are undated. Workspaces sort by turn count; model groups sort by known tokens.
`activeThreads` counts open running/recovering threads; `runningTurns` counts
non-copy inProgress/recovering rows whose thread is also running/recovering.
These are current persisted observations, not proof that a disconnected harness
is still executing.

## Tokens, requests and models

Every aggregate uses the same reporting helper over `token_usage_json`. When
native per-request snapshots exist, they replace the turn aggregate, **never add
to it**. Repeated snapshots of one `messageId`/`requestId` retain the largest final
output counter and count once. Parent and native subagent snapshots are grouped
under their actual reported models and priced at their observation times. Native
subagent calls are requests inside the owning execution, not extra thread turns.
Models are canonicalized with this device's existing pricing catalog and aliases;
ambiguous labels without an explicit alias are not guessed from current settings.

- `tokens` = recorded input plus output. Token source counters are normalized
  through the existing harness adapter, including session-counter deltas/resets.
- `inputTokens` includes cached reads and cache writes. `cachedTokens` and
  `cacheWriteTokens` are subsets of input; do not add them again to total tokens.
- `outputTokens` includes reasoning. `reasoningTokens` is a subset of output;
  do not add it again. One-hour cache writes are a subset of cache writes and
  retain their distinct rate in the pricing helper.
- `turns` on a model row means distinct executions touching that model, including
  missing-usage rows with a known recorded model. A mixed-model execution touches
  several rows, so **do not sum model-row turns to calculate total turns**.
- `requests` is the count of distinct recorded native response IDs when that
  count is known for every included execution. Otherwise it is null. A turn
  counter, output interval or tool event is never substituted for requests.
  `knownRequests` remains the confirmed subtotal when `requests` is null.
- `usageTurns` counts executions with any usable token evidence.
  `partialUsageTurns` counts executions missing usage or explicitly known to have
  partial evidence (for example imported parent-only Claude history).

Live Claude native usage includes observed parent and child snapshots once.
Claude imports/scheduled recovery retain parent request models and usage, but do
not claim to have backfilled every child log: their request/usage coverage stays
partial. Codex uses rollout turn totals in preference to a late ACP final-request
fallback, and subtracts cumulative baselines across resumed processes. Generic
ACP and Grok can report totals without request IDs: tokens remain usable while
request counts stay null. Context-window occupancy is never billable usage.

## Costs and completeness

Costs reuse this Supervisor's existing pricing helper, saved user overrides,
per-request models/times, fast mode, cache durations and long-context rates.
`costUsd` is the **known priced subtotal**. An unpriced model does not erase priced
siblings or earlier turns. `unpricedTokens` counts known tokens lacking a usable
price. Entirely unpriceable nonempty groups have null cost; empty groups cost zero.
Missing usage cannot be expressed as a fabricated token number: it contributes
no known tokens, and is reported separately in coverage.

A UI should show a lower-bound cost when `unpricedTokens > 0` **or** usage coverage
is incomplete, and a missing value when no price is available. Price estimates
are API token estimates, not subscription invoices or hardware/tool costs.
Per-turn mixed-request `priceEstimate` also preserves priced siblings and adds
`unpricedTokens`/`pricePartial`. Historical custom-rate changes reuse saved request
snapshots when present; older totals cannot reconstruct missing request sizes,
models or times. Subscription-usage/allowance endpoints measure another quantity
and must not be added to token charges.

Statistics never open native transcript files or contact a harness. Imports,
explicit detail hydration and native scheduled recovery persist data through
their existing paths, so a later summary can gain evidence. There is no promise
that every run before installation, every external native session, or a missing
provider log has been recorded. `coverage.source: 'persisted'` and the coverage
counts describe the available ledger, not an account billing audit. Existing
missing timestamps/origins/child logs are not silently backfilled or inferred.

## Thread activity and compatibility

`GET /api/thread-activity?hours=24` remains an all-time per-thread map endpoint:
`turns`, tokens and costs are lifetime; `activeMs`/`hourlyMs` clip duration to the
requested UTC window and `totalMs` includes valid historical time. An open turn
adds time only while it is `inProgress` and its thread is `running`. Unconfirmed
recovering turns add no invented time after their last known bound. Completed imported
turns without end times add no time. Its tokens/prices now use the same normalized,
request-aware evidence and exclude marked fork context. `byOrigin` and
`usageCoverage` expose source counts and missing/partial usage; `pricePartial`
is true for unpriced tokens **or** missing/partial usage. This endpoint does not
provide finite-range model totals; clients must not attribute its lifetime usage
to a thread's current model or relabel it as today's activity.

The summary complements hardware metrics, watch/automation attribution and
subscription allowance rather than replacing those interfaces. The indexed
`julianday(started_at)` scan (migration 20) reads turn metadata only; finite ranges
never load timeline/tool payloads. HTTP computes on a blocking worker. Run the
focused production latency test with
`cargo test --release -p pockymoe-runtime stats_indexed_five_thousand_turns_under_150ms --lib -- --ignored --nocapture`.

## TypeScript DTOs

Fields added to existing endpoints are optional when consuming older devices.
`hourly` is present only for today. The current summary response is:

```ts
export type TurnOrigin = 'owner' | 'guest' | 'agent' | 'automation' | 'unknown';
export interface UsageStats {
  turns: number;
  usageTurns: number;
  partialUsageTurns: number;
  requests: number | null;
  knownRequests: number;
  tokens: number;
  inputTokens: number;
  cachedTokens: number;
  cacheWriteTokens: number;
  outputTokens: number;
  reasoningTokens: number;
  costUsd: number | null;
  unpricedTokens: number;
}
export interface StatsSummary {
  range: 'today' | '7d' | '30d' | 'all';
  origin: 'all' | TurnOrigin;
  generatedAt: string;
  totals: UsageStats & {
    completed: number;
    failed: number;
    interrupted: number;
    durationSamples: number;
    avgDurationMs: number | null;
    p50DurationMs: number | null;
  };
  daily: Array<UsageStats & { date: string; failed: number }>;
  hourly?: Array<UsageStats & { hour: number; failed: number }>;
  byModel: Array<UsageStats & { model: string }>;
  byOrigin: Array<UsageStats & { origin: TurnOrigin }>;
  byWorkspace: Array<UsageStats & {
    workspaceId: string; label: string; failed: number;
    lastActivityAt: string | null;
  }>;
  longestThreads: Array<{
    threadId: string; title: string; workspaceId: string; durationMs: number;
  }>;
  activeThreads: number;
  runningTurns: number;
  coverage: {
    source: 'persisted';
    missingUsageTurns: number;
    partialUsageTurns: number;
    unknownOriginTurns: number;
    undatedTurns: number;
    excludedUndatedTurns: number;
    excludedHistoryCopies: number;
  };
}
export interface ThreadActivityAdditions {
  byOrigin?: Partial<Record<TurnOrigin, number>>;
  usageCoverage?: {
    source: 'persisted';
    usageTurns: number;
    missingUsageTurns: number;
    partialUsageTurns: number;
    requests: number | null;
    knownRequests: number;
    unpricedTokens: number;
  };
}
```

On PC, device management and statistics share `/desktop/devices`, in the desktop
client and in a browser signed in to the relay. The top **Devices / Sharing** tabs
keep totals and the amber share review count. Sharing retains `tab=shared` and
`dir=out` in the URL and uses the same SharedAccessPanel and permission dialogs.
**Protect all devices** appears before **Add device** when a device is known to be
unprotected. Device cards link to Workspaces; offline cards retain their state and
cached-data explanation. Device details retain sharing, isolation, setup commands,
token replacement and removal. Incoming shared access remains in Sharing.

Wide signed-in browsers use the same Devices workbench, rail entries and activity
section as the desktop client. The redundant ProductHeader is absent. Workspaces
and account settings reuse their existing workbench frames too. One retained
`PersistentWorkbenchNavigation` owns the browser rail/sidebar DOM across Devices,
threads and workspaces, including sidebar folding, width and scroll. Sidebar
threads open the browser conversation UI; focused viewing uses the same activity
read marks as the client. Native window controls, updater,
drag regions and pet remain outside the browser.

The browser shell owns one account-scoped `DesktopIndexStore`; Devices consumes
it rather than starting a route-local index. Navigation reads owned devices from
that index, while shared grants retain their scoped fallback reads. The Web map
dialog also consumes the index and enables activity reads only while open. Leaving
Devices does not stop the shell; leaving the shell or signing out releases its
connections. A focused transcript may still own its existing thread-specific
stream, rather than another account-wide set of streams.

The browser map remains its existing dialog over the workbench. Settings remains
its existing settings window/full-screen route. Workspace/thread creation, import,
share review, relay landing/help, authentication and admin pages retain their
separate flows; they do not use the Devices/workspaces/account route frame.

Old `/relay-devices` links on PC use a replace redirect preserving the complete
query, including `tab`, `dir`, `shareDevice` and `action`. Signed-out visitors go to
the sign-in portal. Phone-width browsers (under 640px on document load) and mobile
hosts retain the original `/relay-devices` page because phones have no dedicated
device page. Resizing a mounted page does not change this routing decision.

The account usage list shows only devices with usage for the chosen metric, highest first, eight per page. A device without statistics for the range, because it is offline or its Supervisor is older than 0.13.42, is counted below the list and not shown as zero.

The hourly charts sum devices with valid hourly summaries independently. A legacy
summary cannot blank capable siblings. Only when none have hourly summaries does
the turns chart use the labelled runtime estimate; token/cost buckets remain
unknown. Both captions name excluded older devices through an Info mark; a known
empty range is not excluded. Offline/missing responses alone do not prove an old
version.

Device usage rows expand in place to model totals, then workspace totals. Models
use the same metric ordering, token coverage and lower-bound/unknown pricing as
the account table. Activity-only fallback devices explicitly have no model
breakdown; current thread models never supply historical attribution. The Web no
longer presents longest-thread rankings; the runtime summary API remains compatible.

Display labels unwrap serialized JSON array IDs to their last string element and
merge equal labels in both model tables. `<synthetic>` remains in totals as muted
“System generated”. Full model labels are available through Info, with
`MiddleName` retaining both ends of long labels. New turn admissions and usage-event ledger writes normalize
these labels without changing opaque ACP session settings or rewriting old rows.

Usage charts and rows share twelve `--usage-category-1` through
`--usage-category-12` CSS tokens, with light/dark defaults that theme packs may
override. Categories follow sorted IDs from the full account inventory, including
cached and all-time model summaries, independently of range, metric and filters.
Additional cycles mix the hue with foreground at different strengths. Tooltips
and row labels keep the data readable without relying on colour. A newly learned
or removed inventory ID can change its sorted position; unavailable historical
models cannot be discovered from an activity-only runtime.
