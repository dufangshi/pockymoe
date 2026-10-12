# Device-local conversation search

The workbench search switches between **Current conversation**, **This workspace**, and **This device**. Workspace/device scopes search persisted thread titles and `userMessage` / `agentMessage` text across threads on the selected device. Results contain bounded excerpts, thread/workspace names and IDs, timestamps, and original turn/item IDs. The UI identifies the device using the current device route and opens message results with `searchTurn` / `searchItem` URL parameters, loading only that turn. Title results open the thread. Existing current-conversation search and live-message matching remain available.

This is a control-plane capability over the device journal, independent of harness-native search. It does not search tool output, reasoning, attachments, files, unsaved native harness sessions, or every offline device. Native history becomes searchable when imported/persisted. Account-wide or multiple-device aggregation is not implemented; each device is explicitly selected.

## API and access

- `GET /api/search?q=...&limit=50&offset=0` searches this device.
- `GET /api/workspaces/{id}/search?q=...&limit=50&offset=0` filters by workspace in SQL.
- `GET /api/threads/{id}/search` retains the existing thread-only contract.

Queries are literal case-insensitive substrings, 1–200 Unicode characters after trimming. Chinese, one/two-character queries, emoji, quotes, `%`, `_`, and FTS operators are data rather than query syntax. Limits are 1–100, offsets 0–10000. The response includes `matches`, `hasMore`, and `nextOffset`; results sort newest first with a deterministic document-ID tie break. Excerpts retain the original Unicode text, with at most 100 characters of leading and 240 characters of trailing context. Pagination reflects the current journal rather than a frozen snapshot.

The Supervisor's existing authenticated HTTP boundary protects local requests. The relay authorizes explicit paths before forwarding: global/workspace conversation search requires device ownership or a device-scoped thread read/control grant. A thread share can use only that thread's search endpoint. Workspace filesystem access, including access attached to a thread share, cannot enumerate workspace conversations. UI scope options follow the same permission boundary, but server ACLs remain authoritative. Device conversation readers may obtain the signed device encryption descriptor through the workspace transport-key alias even without file access; each data request still has its own ACL.

Browser requests use the existing encrypted device transport: query parameters and result bodies are encrypted between browser and Supervisor; the relay sees the routing path and authorization metadata. No relay index or central plaintext conversation database is created. Search fails visibly on an unavailable endpoint/device; global search never falls back to loading histories into the browser.

## Index maintenance

Runtime migration 11 (`device_conversation_search`) transactionally creates a compact `search_documents` projection and an external-content FTS5 trigram index, then backfills existing titles and conversational journal items. Migration failure rolls back the schema, backfill, and ledger update. This is a one-time startup cost proportional to persisted conversational text; subsequent clean starts do not rebuild history.

SQLite triggers maintain documents and FTS in the same transaction as title edits, new/streaming message writes, message/turn/thread deletions, and fork history copies. A thread source change reprojects its messages. Rust-registered deterministic SQL functions provide Unicode lowercasing and the existing imported-Codex user-text sanitizer; injected context is excluded and imported duplicate visible messages resolve to the earliest original item. Do not write this journal through another SQLite connection lacking these functions.

Queries of at least three Unicode characters use quoted FTS trigram candidates followed by exact folded substring verification. Shorter queries scan only the device-side conversational projection with bounded returned pages; they never decode tool payloads or transfer full histories. FTS corruption can be checked/rebuilt from the projection using SQLite FTS5 `integrity-check` / `rebuild` through the owning runtime connection. The projection is derived data; the journal remains authoritative.

## UI integration

Shared UI exports `ConversationSearchScopePicker` and `ConversationSearchExcerpt`. Search copy lives in paired `src/i18n/search.en.ts` / `search.zh-CN.ts` resources, consumed through the unified `useI18n` store. Rebuild the shared UI source repository and refresh the Web file dependency before checking or publishing a combined version. The public Web is deployed to GitHub Pages from `main` with the shared UI commit pinned in `thread-ui.sha`.

The new scopes require the updated device Supervisor, including runtime migration 11. Publishing the Web or relay alone does not update connected devices. A workspace/device endpoint returning 404 shows localized guidance to update the device runtime, verify the workspace still exists, or explicitly choose Current conversation. The scope stays selected; no fallback runs automatically. Network failures and permission denials retain their own errors. Current-conversation search keeps its older-Supervisor compatibility path.
