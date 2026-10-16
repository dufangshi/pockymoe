# Thread text summary API

`GET /api/threads/{id}/summary` returns a compact, chronological extraction of a
thread's **persisted public prose**. It is suitable as context for a future text
polishing model. No model runs: text is not paraphrased or summarized, and the
default request includes the entire stored public conversation without a hidden
turn, message, or character cap.

This is separate from Treer's transcript, the interaction CLI transcript, and
`GET /api/threads/{id}?view=summary`. Their behavior is unchanged.

## Response

```json
{
  "messages": [
    {"time": "2026-10-15T10:00:00Z", "role": "user", "text": "Check the document."},
    {"time": "2026-10-15T10:01:00Z", "role": "assistant", "text": "I found two issues."},
    {"time": "2026-10-15T10:02:00Z", "role": "user", "text": "Keep the examples."},
    {"time": "2026-10-15T10:03:00Z", "role": "assistant", "text": "Updated both sections."}
  ],
  "truncated": false
}
```

Each complete message has only `time`, `role` (`user` or `assistant`), and `text`.
Whitespace, Markdown, and Unicode are preserved. The response uses JSON and
`Cache-Control: no-store`. An existing empty thread returns
`{"messages":[],"truncated":false}`; a nonexistent thread returns HTTP 404.

Included text is the accepted initial user question, delivered user steer, every
separate intermediate assistant commentary, and every turn's final answer.
An initial question missing from legacy history items falls back to that turn's
saved display prompt, once. Empty message bodies are omitted.

Excluded content includes tools and their results, reasoning, private channels,
plans, operation records, images, system context, automation control prompts,
native task notifications, peer messages, and control reminders. Explicit origin
metadata and automation/peer ledgers identify control input; older Pockymoe control
envelopes and injected-context prefixes are also recognized when an explicit
user origin is absent. Newly accepted actual user prompts and steers record their
user origin, so quoting a control envelope does not suppress their prose. Assistant public
responses remain included even when their turn was started by an automation.
Queued input that has not been delivered is excluded. A delivered steer remains
present only once even while its queue record is retained. Native echoes of an
initial input or delivered steer are excluded when the canonical stored input
already contains the same text. Separate accepted user steers and assistant
messages are not deduplicated merely because they repeat the same words.

The endpoint reads only the Supervisor's database. It does not load a harness,
fetch native transcripts, inspect model context, expand tools, or apply the UI's
message grouping and user-message hoisting. Imported history is available only
to the extent it has already been persisted.

## Ordering and timestamps

Messages with recorded emission timestamps sort by the timestamp's actual instant,
including when stored timestamps have different UTC offsets. Returned timestamps
are normalized to UTC RFC3339, preserving the recorded instant with normalized
fractional seconds. Ties retain
turn ordinal and durable history insertion order; no message-kind sort promotes
user input ahead of earlier assistant commentary.

For a live assistant stream, the first observed emission time survives subsequent
snapshots and completion. Its accumulated text is returned as one message. User
steer uses its recorded harness submission time, rather than the queue creation
time or active turn's start. This is **message chronology**, not token-by-token
interleaving: if one assistant message streams across a steer, the complete
message stays at its first emission time. Separate commentary messages retain
their separate positions.

If an item's emission timestamp is missing or invalid, `time` is `null`. No
completion/import time or current time is presented as a precise emission time.
For ordering only, such an item uses the preceding recorded item's time in its
turn, then the turn start, then the preceding turn's available anchor. Unanchored
items retain turn/insertion order before timestamped items. A legacy display
prompt uses its saved turn start, or `null` when that is unknown. Historical
ordering is limited to this recorded evidence; the API cannot reconstruct an
unrecorded delivery time.

## Filters and bounds

All query parameters are optional and composable. Unknown parameters, duplicate
parameters, invalid dates, non-integers, and out-of-range bounds return HTTP 400.

| Parameter | Meaning | Valid values |
| --- | --- | --- |
| `since` | Include messages whose emission is at or after this instant | RFC3339 with `Z` or an explicit UTC offset |
| `recentSeconds` | Include messages emitted within this elapsed time before the request | Integer, 1–3155760000 |
| `turns` | Select the newest N stored execution turns, including an active turn | Integer, 1–1000000 |
| `maxChars` | Retain the newest text within this total character budget | Integer, 1–10000000 |

`since` and `recentSeconds` intersect: the later cutoff wins. A time filter
excludes messages with unknown emission time; an ordering anchor does not make
their date known. Turn selection is by saved turn ordinal before time filtering;
a turn is one execution request, and a steer belongs to its active turn.

The character cap applies last, across the selected messages' `text` fields, in
Unicode scalar values (Rust `char`), not UTF-8 bytes or grapheme clusters. Combining
marks count separately. The newest suffix is retained; the response stays in
ascending chronological order. `maxChars` counts text only, not JSON syntax,
timestamps, or roles. HTTP values are decimal integers; `20k` is not accepted.

If the cap removes any text, the envelope's `truncated` is `true`. If its boundary
falls inside a message, that message additionally has `"truncated":true`, meaning
**the beginning of that message was removed**. Complete messages omit that field.
No ellipsis is inserted into original prose. When the cap omits only whole older
messages, the envelope signals truncation and all retained messages remain complete.
Time/turn selection alone does not set `truncated`.

For example, `?turns=4&recentSeconds=3600&maxChars=20000` selects the last four
turns, retains messages emitted within the last hour, then keeps at most 20,000
characters of the newest text. Encode a positive timestamp offset's `+` as `%2B`
in a URL, or use an HTTP client's query encoder.

The default response is intentionally unbounded so it faithfully includes all
stored public context. Consumers needing a bounded standalone response should
set `maxChars`, optionally with time or turn filters. The server selects compact
message metadata first and reads only retained text slices; it does not construct
full thread DTOs or transfer tool detail into the API process. Metadata work still
scales with the selected history, and an uncapped response's memory and transfer
size scale with its public text. There is no implicit pagination or silent cap.

## Access

The endpoint uses existing Supervisor authentication and thread read permissions.
Owners can read it locally or through the relay. A read-only thread share can
read only its authorized thread group, using the same lineage rules as other
thread reads. Workspace/device grants keep their existing scope. It grants no
additional access to workspaces, harness sessions, other threads, or control
operations; the relay and Supervisor use the same route authorization rules.
