# Context inspection

Detailed context inspection is an optional device setting for Codex, Claude and DSH.
It is off by default. The ordinary context meter remains available for other
harnesses. Enabling or disabling inspection takes effect when a loaded harness
next resumes for a prompt; an active prompt is not interrupted to rebind its
process. The setting persists in the Supervisor database.

The context strip sits below the smaller model name and reasoning effort within
the existing fixed-height model control. Its separate button opens context
details; the strip uses a single color and the exact occupancy ratio. The model
control does not put a long context report in its hover title.

The context panel has three views, chosen by tabs under its title. The tabs
appear only where the host can load recorded steps.

- **Now** shows occupancy, category counts, observation time and the available
  tool, memory and skill details. Deferred tool definitions are outside the
  occupied window and are listed separately. Missing provider details are not
  fabricated as zeroes. Detail expansion reserves a stable scrollbar gutter, so
  opening child rows does not change content width or wrap paths again.
- **Trend** draws one bar per model request, or per turn, stacked by what the
  context held. **Total** shows the whole context of each request; **Change**
  shows what each request added or dropped since the one before. A strip under
  the bars shows how long the model and the tools took. The selected bar's
  figures are listed under the chart: context size and change, cache hit, output,
  model time, tool time, and each group with its change. Hovering, tapping or
  the arrow keys select a bar. A long thread scrolls sideways and opens at its
  newest request.
- **Statistics** shows the thread's cache hit rate with what it read, what came
  from cache, what it wrote to cache and what it produced; the makeup of
  everything the recorded requests read; and model time against tool and waiting
  time, with the output speed.

The footer contains only Compact context; it remains visible and disabled when no
supported action is available or the thread is busy. Claude's late `/compact`
advertisement is resolved from the current session instead of a provider-wide
capability cache.

### Categories and groups

A snapshot's used categories split the conversation by who wrote each part where
the harness allows it: `user_messages`, `assistant_messages`, `reasoning`,
`tool_calls`, `tool_results` and `injected` (text the harness or Pockymoe put
into the conversation: tagged blocks, attachments, compaction summaries). What
cannot be told apart stays in `messages`. A category may carry a `count` of the
items behind it.

Charts fold the categories into eight groups with one fixed color each: system
(system prompt, MCP instructions, runtime context, custom agents), tool
definitions, memory files, skills, user messages, model output (assistant
messages, reasoning, tool calls, unsplit messages), tool results and injected
content. The difference between the native total and the category sum is drawn
in neutral gray and never spread over the groups.

- Codex: the trace analyzer classifies every input item, so all six
  conversation categories are available.
- Claude: `messageBreakdown` gives user, assistant, tool-call and tool-result
  tokens. Its attachment and redirected-context figures overlap other
  categories, so `injected` is capped to what remains of Messages.
- DSH: retained nodes are split by their event type into user messages,
  assistant messages and tool results; instruction, skill and runtime nodes keep
  their own categories.

## Collection

Claude uses a thin extension of the installed `claude-agent-acp` adapter and
reads `getContextUsage()` on its existing Query. It does not create another
conversation or submit `/context` as a prompt. Summary readings use local
estimates; full readings can call the harness's token-count API. Opening the
Claude panel requests full counting automatically; each usage report during a prompt and prompt
completion obtain a summary in the background. The API bounds control requests to twenty seconds. Unsupported
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

## Steps

The Supervisor records one step for every model request it can tell apart: each
new per-request usage report inside a turn, for any harness and whether or not
inspection is enabled. A step holds
its turn, time, model, the tokens the request read (`inputTokens`, with
`cachedInputTokens` and `cacheWriteInputTokens` inside it), the tokens it wrote,
the time until the report arrived (`modelMs`) and the time spent before the
request started on tools and waiting (`toolMs`). A context snapshot whose total
agrees with a step, or that arrives before the next step, supplies the step's
`categories`; a step without one is drawn as an unsplit gray bar.

A composition needs inspection to be enabled. Codex steps take theirs from the
trace analyzer. Claude is asked for a summary reading after each usage report
during a turn, so its steps have one too. DSH steps take the bridge's snapshot.
Other harnesses, and any harness with inspection off, record steps without one.

Steps are diagnostic data. They live in `<database>.context-steps/<thread>.jsonl`
beside the database, not in it, so an older Supervisor ignores them. A thread
keeps its newest 1,500 steps, and deleting the thread deletes its file. Requests
made before this was recorded have no steps, and against a Supervisor too old
to have the route the Trend and Statistics views say it needs an update; the thread's statistics still come
from the turns' recorded usage.

The thread totals behind Statistics are summed from the turns' usage, not from
the steps, so they cover the whole thread. The cumulative makeup covers only
steps that have a composition, and the panel says how many those are. Because
every request reads the whole context again, cumulative figures are many times
the context size.

## HTTP and events

- `GET /api/threads/{id}/context/steps` returns `steps` (each with its `step`
  and `turn` numbers), `turnCount` and `totals` (`usageTurns`, `inputTokens`,
  `cachedInputTokens`, `cacheWriteInputTokens`, `outputTokens`,
  `reasoningOutputTokens`, `modelTimeMs`, `wallTimeMs`, `outputTimeMs`,
  `timedOutputTokens`). It is readable wherever the thread's context is.

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
