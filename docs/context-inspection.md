# Context inspection

Detailed context inspection is an optional device setting for Codex, Claude and DSH.
It is off by default. The ordinary context meter remains available for other
harnesses. Enabling or disabling inspection takes effect when a loaded harness
next resumes for a prompt; an active prompt is not interrupted to rebind its
process. The setting persists in the Supervisor database.

The context strip sits below the smaller model name and reasoning effort within
the existing fixed-height model control. Its separate button opens context
details; the strip uses a single color and the exact occupancy ratio. The context panel shows
occupancy, category counts, observation time and the available tool, memory and
skill details. Deferred tool definitions are outside the occupied window and
are listed separately. Missing provider details are not fabricated as zeroes.
The model control no longer puts a long context report in its hover title.
Detail expansion reserves a stable scrollbar gutter, so opening child rows
does not change content width or wrap paths again. The footer contains only
Compact context; it remains visible and disabled when no supported action is
available or the thread is busy. Claude's late `/compact` advertisement is
resolved from the current session instead of a provider-wide capability cache.

## Collection

Claude uses a thin extension of the installed `claude-agent-acp` adapter and
reads `getContextUsage()` on its existing Query. It does not create another
conversation or submit `/context` as a prompt. Summary readings use local
estimates; full readings can call the harness's token-count API. Opening the
Claude panel requests full counting automatically; prompt completion obtains a summary in the
background. The API bounds control requests to twenty seconds. Unsupported
adapter layouts retain ordinary ACP behavior and report inspection unavailable.
The wrapper currently supports the adapter's ACP v1 builder export.

Claude reports category totals for system tools and system prompts, but its
optional `systemTools`, `systemPromptSections` and `deferredBuiltinTools`
diagnostics are not populated by current native producers. The panel only
expands details the producer actually supplies.

Codex enables its local rollout trace in the owned ACP process when inspection
is enabled. A background worker checks new inference events every 25ms while a
visible prompt is active. It counts request fragments with a bundled
`o200k_base` tokenizer and reuses hashes/counts across requests. This is an
estimate, not a promise that the tokenizer matches every upstream model.
Media bytes, encrypted state, item IDs and transport metadata are never
stringified for text counting. Only readable content blocks and tool-call
arguments are counted; images/audio and opaque reasoning remain in the native
unattributed difference. A request stays pending until its provider response is
correlated, so an uncalibrated estimate cannot briefly replace native occupancy.
If the local tokenizer still estimates more than Codex reports, the category sum
and signed discrepancy are displayed separately using `compositionTokens` and
`attributionDeltaTokens`, without rescaling the messages category. Codex total
occupancy is always the correlated native response total. The Web also discards
old malformed Codex category caches that exceed their native occupancy without
explicit separate-composition metadata. Refreshing cached Codex details does not
re-emit them as a new occupancy reading.
Responses are correlated with inference IDs before calibrating the input total;
unattributed overhead remains separate instead of being distributed across
categories. The response's native total updates occupancy after output.

The collector accepts complete request inputs. Incremental WebSocket requests
that reference `previous_response_id` are not treated as complete windows;
ordinary native occupancy remains the fallback. A new session may wait for its
first native window reading before detailed counts become available. Compaction
and model transitions may require a new observation before classifications are
current.

DSH reads `contextBreakdown`, `contextPressure` and `tokenMeter.measure()` from
the existing ACP-owned process through its bridge. Occupancy uses the native
`projectedTokens`: the next prompt estimate anchored to the latest provider input
(including cache reads/writes), adjusted for retained-surface changes. It updates
immediately after native `/compact`. Neither cumulative family billing nor the
meter's request-plus-output `totalTokens` is used as occupancy. Before the first
usage/capacity observation, ordinary ACP occupancy remains the fallback.

DSH's native composition uses roughly four characters per token plus framing;
it is not an exact tokenizer, particularly for CJK and JSON. Its category sum
can be above or below the projected total. Snapshots expose `compositionTokens`
and signed `attributionDeltaTokens` without rescaling categories or fabricating a
negative bucket. The panel draws total occupancy separately from composition,
and category shares use the composition sum. Free space uses the context window.
Tool item estimates have independent rounding and need not sum to the tools bucket.

Retained native node sequences provide project-instruction, skill and runtime
context attribution; these are removed from the messages bucket when split out.
If node prices do not match the native breakdown, the three native categories
remain the fallback. Tool names come from the current request header, including
native `mcp__` names. Multi-file instruction messages expose a combined count,
not invented per-file counts. Replaced historical nodes and child-agent windows
are excluded. The bridge walks new route events incrementally and uses native
`eventAt()` for retained nodes. `asOfSeq` and route metadata accompany snapshots;
a changed route is marked stale until that route reports usage. Summary and full
both read these same local native estimates; DSH has no separate full-count API.
The automatic-compaction marker follows the native configuration, including an
exact provider/model threshold override when present.

Only counts, names and provider-supplied metadata are exposed through HTTP and
WebSocket events. Raw Codex payloads stay in a private local trace directory,
are removed after aggregation, and the owned trace directory is removed when
its process is released. Request payloads above 16MiB are not parsed. Inspection
is diagnostic: inability to obtain it does not fail a prompt.

## HTTP and events

- `GET /api/context-inspection/settings` returns `{ "enabled": false }`.
- `PUT /api/context-inspection/settings` accepts `{ "enabled": true }`.
  This is a device-owner setting.
- `GET /api/threads/{id}/context?detail=summary` returns `enabled`, `supported`,
  `availability`, `snapshot` and an optional reason. `detail=full&refresh=true`
  requests a current full Claude reading. Reading the endpoint never loads an
  unloaded harness or acquires another native conversation writer.
- `thread.context.updated` may include `contextUsage.details` in addition to
  the existing total/window fields. A later native total preserves the last
  details and marks them stale when the totals no longer match.

Snapshots contain provider, model, source, precision, used/window tokens,
observation time, categories, and optional detail groups. Categories use
`used`, `free`, `buffer`, or `deferred`; deferred entries do not contribute to
occupancy percentages. A cached snapshot can remain visible with its timestamp
when a harness is unloaded or a refresh fails. None of these numbers are
cumulative billing totals, and cache reads are not an additional context slice.
