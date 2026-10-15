# Architecture

Pockymoe is a self-hosted control plane for personal coding-agent sessions.

```text
Browser / mobile WebView / native macOS app
  -> Supervisor (Axum, this repo)
      -> Runtime trait
           -> ACP stdio + thin harness adapters
           -> deterministic fake runtime (tests)
      -> SQLite journal (threads, turns, history items)
  -> optional Relay (outbound device tunnel)
```

## Ownership

| Crate | Owns |
| --- | --- |
| `crates/protocol` | JSON DTOs shared with the React client |
| `crates/runtime` | SQLite journal, workspace files, ACP catalog, fake runtime, thread service |
| `crates/supervisor` | HTTP + WebSocket + relay tunnel client, device side of [account key trust](account-key-trust.md) |
| `crates/relay` | Public accounts and encrypted account vaults, devices, shares |
| `crates/cli` | `pockymoe` binary |

## Module layout

Harness differences stay under `crates/runtime/src/acp/` — `catalog.rs` holds the
command catalog, `capabilities.rs` the capability overlays, and per-harness
adapters (`grok.rs`, `codex_bridge.rs`, `deepseek.rs`) sit alongside them.
Supervisor code does not special-case Codex vs Grok.

Large modules follow a `foo.rs` + sibling `foo/` convention: the parent file
keeps the public surface and the directory holds cohesive internals. Existing
examples are `runtime/src/service.rs` + `service/{reliability,update}.rs`,
`runtime/src/upstreams.rs` + `upstreams/discovery.rs`,
`supervisor/src/secure_transport.rs` + `secure_transport/streams.rs`, and
`relay/src/auth_api.rs` + `auth_api/passkeys.rs`. Prefer extending that pattern
over growing a single file; see [code-structure.md](code-structure.md) for the
current size inventory and the modules that most need it.

## Clients

Native Android/iOS/Windows clients are not duplicated in this rewrite. They keep
talking to the same HTTP/WS contract from `main`. The React thread surface stays
in `pockymoe-thread-ui`, consumed by `apps/supervisor-web` as
`@pockymoe/thread-ui` and `@pockymoe/shared`.

The native macOS/iOS/Android client apps live in a separate repository
(`remote-codex-app`), not in this tree — `apps/ios` and `apps/android` here hold
only built web bundles and packaging inputs. `apps/windows-device-manager` is an
independently released bootstrap; see AGENTS.md for its release boundary.

## History payloads and compression

Thread history is served in three shapes: `GET /api/threads/{id}?view=summary`
(user messages, the final reply and background anchors per completed turn),
`?view=full` and `GET /api/threads/{id}/turns/{turnId}/detail`. Every shape
defers `commandExecution` and `fileChange` output: the item carries
`hasDeferredDetail: true` instead of `detailText`, and the UI loads it from
`GET /api/threads/{id}/items/{itemId}/detail` (`{id, kind, title, text}`) when
the item is opened. WebSocket item events use the same deferral.

A summary turn whose operations are deferred also carries `activity`: counts of
`commands`, `fileChanges`, `changedFiles`, `addedLines`, `removedLines`, `reads`,
`searches`, `tools`, `agents`, `reasoning` and `failed` (zero counts omitted), and
`files`, up to eight changed paths with the latest edit's `itemId` and its line
counts (a multi-file patch credits its counts to its first path). The transcript
uses it for the folded turn's work line ("Worked for 51s · edited 2 files +64 −3 ·
6 commands") and its changed-file names, whose diffs load on demand through the
item detail endpoint. `segments` repeats the counts for each stretch of operations
with `before`, the id of the conversation item that follows it (null at the end),
and the stretch's `startedAt` and `endedAt`.

`narration=1` on the summary view keeps every reply with text in summary turns.
The Web sends it when Settings → Preferences → Thread timeline auto-collapse is
off: completed turns then show the agent's narration with each stretch of work as
a folded run, and opening a run loads the turn detail.

Clients that send `compact=1` on those three endpoints get compact items
(`history::compact_turn`):

- `detailText` is deferred for every tool kind (`commandExecution`,
  `fileChange`, `fileRead`, `webSearch`, `toolCall`, `agentToolCall`,
  `skillToolCall`); other kinds (reasoning, images, notices) keep it;
- `previewText` is omitted when it equals `text`, except for `fileChange`,
  whose summary badges read `previewText`; readers fall back to `text`;
- `sourceTurnId` is omitted when it equals the parent turn's `id`; readers
  fall back to the turn. Live items outside a turn keep it.

The Web sends `compact=1`; native apps, the CLI and older Web builds do not and
keep the full encoding. Older Supervisors ignore the parameter.

HTTP responses are compressed (gzip or brotli, by `Accept-Encoding`) when the
body is at least 1 KB of text, JSON, JavaScript, XML or SVG. SSE, WebSocket
upgrades, images, archives, other binary types and range responses stay
identity. Relay requests are dispatched in-process without `Accept-Encoding`;
their bodies are compressed inside the encrypted envelope instead, see
[relay-security-operations.zh.md](relay-security-operations.zh.md).
