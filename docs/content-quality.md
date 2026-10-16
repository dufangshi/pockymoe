# Content Quality

`pockymoe.content-quality` is a built-in, optional second-pass writing plugin.
It is disabled by default. It uses an explicitly configured OpenAI-compatible
`chat/completions` endpoint and does not change the coding harness or its model.
A successful plugin run means editing completed; it does not mean the underlying
task achieved its goal.

## Settings and private connection

Settings use camelCase, reject unknown fields, and have these defaults:

```json
{
  "connectionFile": "",
  "model": "",
  "language": "zh-CN",
  "preset": "clear",
  "detail": "balanced",
  "readerBackground": "",
  "minReplyChars": 800,
  "autoReplies": true,
  "autoDocuments": false,
  "documentMode": "review"
}
```

Presets are `concise`, `clear`, `technical`, `beginner`, and `copyedit`.
`detail` accepts `short`, `balanced`, or `detailed`; `documentMode` accepts
`review` or `rewrite`. Language is a nonempty string up to 64 bytes; reader
background is limited to 2,000 bytes. `minReplyChars` is an integer from 0 to
65,536. The model can be selected in settings or in the connection file.

The private local connection file contains `baseUrl`, `apiKey`, and an optional
`model`. Keep it outside tracked source and restrict its filesystem permissions
to the Supervisor account. Never put its contents in plugin settings, action
input, thread messages, or documents. An empty `connectionFile` resolves to
`Supervisor.config.database_url.with_extension("polish-upstream.json")`.
Explicit relative connection paths resolve from the Supervisor process working
directory. A model is required in settings or the file. The endpoint can be a
base path such as `/v1`, or a complete `/chat/completions` URL. Embedded URL
credentials, queries, fragments and redirects are rejected. Prefer HTTPS;
HTTP is supported for local or private upstreams.

Only the path and nonsecret writing preferences are saved in plugin settings.
The API key is read locally and used only in the upstream Authorization header.
Errors report bounded generic messages or HTTP status codes, without returning
raw upstream bodies or transport errors. Requests have a 10-second connection
timeout, a 90-second total HTTP timeout and a 512 KiB response-body limit.

## Replies

With `autoReplies` enabled, the plugin supplies a standard managed automation
for completed thread turns meeting the configured reply-length threshold.
The hook uses the normal automation ledger, settings snapshots and receipts.
It runs after turn completion. Editing takes a separate upstream request and
can fail independently of the original turn.

`rewriteReply` accepts `{"turnId":"..."}`. Automatic hooks pass `{}` and
resolve the turn from their event, requiring its `sourceThreadId` to match
the target thread. Only stored terminal turns are eligible; manual actions
can also polish failed or interrupted turns while retaining their limitations.
The final-phase nonempty `agentMessage` is preferred, with the last nonempty
agent message as fallback for unrecognized phases. The reply must meet the
configured minimum and be no larger than 64 KiB.

The upstream receives the original reply, bounded public user prompt, turn
status, and a bounded summary of operation kinds and statuses. Reasoning items,
provider detail payloads, command output and tool result bodies are excluded.
The original message `text` remains unchanged. A successful edit adds flattened
message metadata:

```text
polishedReply: {text, preset, language, model, sourceHash, createdAt, requestHash}
```

`sourceHash` is SHA-256 of the original UTF-8 reply. `requestHash` fingerprints
the source, public settings and effective model for duplicate suppression;
it excludes the connection path and credential. Identical requests skip the
upstream call. Before saving, the plugin rechecks terminal status and the
original text in a database transaction. It emits `thread.item.completed`
after the durable update so subscribers can refresh the edited presentation.
Action receipts contain identifiers and status, rather than the full reply.

The editor is instructed to retain uncertainty, limitations and negative
results and to avoid inventing work, tests, citations or success. Protected
code/link content and a numeric/technical token inventory are checked before
accepting the result. These checks are conservative and can reject otherwise
reasonable edits; they do not prove semantic equivalence. The original answer
remains available for comparison.

## Documents

`reviewDocument` accepts `{"path":"guide.md","mode":"review"}`. `mode` is
optional and defaults to `documentMode`. Review returns findings, a proposed
full rewrite, the source hash, and a bounded unified diff without changing the
file. An explicit `mode:"rewrite"` or saved `documentMode:"rewrite"` permits
writing the proposal. The action operates on one explicit path in the thread's
workspace or worktree. This explicit action does not discover documents, execute
commands or create commits.

Paths must be workspace-relative and contain no traversal. Files must exist,
be regular strict UTF-8 text, and be no larger than 64 KiB. Accepted extensions
are `.md`, `.mdx`, `.rst`, and `.txt`. Every symlink path component is rejected;
canonical paths must stay inside the workspace. Hidden paths, vendor/build/
generated directories, generated-file headers and `AGENTS.md`, `CLAUDE.md`,
`SKILL.md` are excluded.

Markdown fenced and indented code, inline code, tables, frontmatter, complete inline links/images
(including their labels), and URLs are replaced by unique markers for the editor and restored
verbatim. Missing, reordered or duplicated markers and changed bracket counts reject the
proposal. Balanced parentheses in link targets are preserved; nested/escaped
labels and escaped or quoted targets are conservatively rejected.
Unclosed fences, inline code and frontmatter are rejected. MDX files, JSX,
HTML, brace expressions and RST directives/literal blocks are conservatively
skipped as unsupported markup. Numeric facts and recognizable technical tokens
must preserve their counts and exact spelling. This may reject translation
of technical terminology; the plugin never silently accepts a changed count.

Rewrites stage a temporary file in the same directory, preserve permissions,
flush it, recheck the source hash and atomically replace the original. If the
source changed during the upstream call, the action fails and preserves those
edits. Plugin writes are serialized. The filesystem does not offer a portable
compare-and-swap rename against unrelated editors: avoid concurrent external
writes during the final replacement. The diff is capped at 12,000 characters
and includes `diffTruncated`; the proposal remains available in the receipt.

## Automatic document review after a turn

`autoDocuments` is a separate, opt-in boolean, disabled by default. When enabled,
the managed successful-turn hook calls `afterTurn` instead of the reply-only
action. This hook does not use the reply-length condition: short or disabled
replies can still be followed by document review. Reply editing independently
honors `autoReplies` and `minReplyChars`.

`afterTurn` accepts the same optional `turnId` and event ownership rules as
`rewriteReply`, and requires a completed stored turn. It prioritizes up to five supported human documents recorded in the completed
turn's normalized `fileChange` items, then fills remaining slots from bounded,
read-only Git status results for modified, staged or untracked documents. The same containment, symlink,
generated, instruction-file, format and size restrictions apply. Selection is
alphabetical; `documentsTruncated` indicates additional eligible files. Recorded absolute paths are canonicalized and must remain inside the workspace;
relative paths use the workspace root. Failed and deleted change records are
excluded. Recorded paths take priority over dirty-worktree candidates. A nested
workspace excludes sibling repository files. If Git is unavailable, explicit
recorded paths remain reviewable and `documentScanError` reports the scan warning.
Shell-only committed edits without normalized `fileChange` paths cannot be
discovered automatically.

Automatic document processing always uses **review mode**, even when saved
`documentMode` is `rewrite`; automatic review never writes files. Explicit
`reviewDocument` remains the route for applying a rewrite. A file's review
failure is recorded individually and does not stop the remaining reviews.
The aggregate receipt includes reply status and per-document findings, source
hashes and a diff capped at 2,000 characters per file, under a 240 KiB output
limit. The full proposal remains available through explicit `reviewDocument`.
Git subprocesses clear the inherited environment, retain only the executable
path and standard temporary/system paths, and explicitly disable fsmonitor and
the untracked cache. Git scans are limited to 64 KiB and five seconds per command; the reply has a
65-second budget and each document review a 40-second budget, within a
300-second hook action budget.

The scan reflects the workspace's current dirty files, including changes made
before the completed turn; it does not claim turn-level authorship or provide
a pre-commit check. It runs after a successful turn and creates no commits.

## Connectivity and verification

`testConnection` accepts `{}` and sends a small connectivity request. Its
receipt exposes only connectivity status and the selected model. It does not
return the upstream reply, endpoint or credential.

Focused backend tests, including mock HTTP and database/event integration:

```sh
cargo test -p pockymoe-runtime --lib plugins::content_quality --locked
```

An ignored live protocol check requires explicit local authorization and an
existing private connection file. Set its absolute path in
`POCKYMOE_CONTENT_QUALITY_LIVE_CONNECTION`. If the private file omits a model,
set `POCKYMOE_CONTENT_QUALITY_LIVE_MODEL` to the chosen upstream model. Then run:

```sh
cargo test -p pockymoe-runtime --lib \
  plugins::content_quality::tests::live_connection_and_editorial_protocol \
  --locked -- --ignored
```

The ignored `live_reply_and_document_actions` test additionally exercises reply
persistence, duplicate suppression, document review and explicit document
rewrite against that upstream, using a temporary workspace only.

The ignored `live_after_turn_reviews_git_document_without_writing` test verifies
the opt-in Git document scan and automatic review against the live upstream
without modifying its temporary document.
