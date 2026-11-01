# Context inspection

Detailed context inspection is an optional device setting for Codex and Claude.
It is off by default. The ordinary context meter remains available for other
harnesses. Enabling or disabling inspection takes effect when a loaded harness
next resumes for a prompt; an active prompt is not interrupted to rebind its
process. The setting persists in the Supervisor database.

The composer has separate model and context controls. The context panel shows
occupancy, category counts, observation time and the available tool, memory and
skill details. Deferred tool definitions are outside the occupied window and
are listed separately. Missing provider details are not fabricated as zeroes.
The model control no longer puts a long context report in its hover title.

## Collection

Claude uses a thin extension of the installed `claude-agent-acp` adapter and
reads `getContextUsage()` on its existing Query. It does not create another
conversation or submit `/context` as a prompt. Summary readings use local
estimates; full refreshes can call the harness's token-count API. Full counting
is requested by the panel, while prompt completion obtains a summary in the
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
Responses are correlated with inference IDs before calibrating the input total;
unattributed overhead remains separate instead of being distributed across
categories. The response's native total updates occupancy after output.

The collector accepts complete request inputs. Incremental WebSocket requests
that reference `previous_response_id` are not treated as complete windows;
ordinary native occupancy remains the fallback. A new session may wait for its
first native window reading before detailed counts become available. Compaction
and model transitions may require a new observation before classifications are
current.

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
