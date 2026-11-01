# Device turn statistics

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
empty), so clients can weight average durations across devices exactly.
Average and p50 are null with no durations. p50 averages
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

Migration 20 adds an idempotent `julianday(started_at)` SQLite expression index,
so offset timestamps and UTC timestamps use the same indexed range scan. The
summary joins only turn/thread/workspace rows and never reads history items.
HTTP computes in a blocking worker instead of blocking the async reactor.
The focused production latency benchmark runs with
`cargo test --release -p pockymoe-runtime stats_indexed_five_thousand_turns_under_150ms --lib -- --ignored --nocapture`;
it is intentionally ignored in debug test runs because unoptimized code is not
a production latency measurement.

This calendar summary complements [the thread map](thread-interaction.md#thread-map):
`/api/thread-activity` reports all-time per-thread totals and windowed runtime;
this endpoint reports turns started in local calendar dates, terminal statuses,
model/workspace groups and duration distribution. It reuses the same persisted
usage and pricing helper, including native per-request/subagent accounting.
Device hardware metrics and per-watch attribution remain separate endpoints.

The endpoint is owner-only, including when forwarded through a relay. Shared
browser grants and Treer thread-scoped viewers cannot read device-wide totals.

## Response type

```ts
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
