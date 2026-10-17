# Turn output token speed

`tokenUsage.generationSpeed` is optional; historical turns without trustworthy
timing do not receive an invented wall-clock speed. Runtime events start the LLM
clock at turn admission and pause it while any tools are executing (including
parallel tool waits). They resume it only once every outstanding tool completes.
This includes model response latency and reasoning, but excludes tool execution
and explicit user-input/permission waits.

The live footer displays the **latest confirmed response interval**: its actual
output token delta divided by the LLM time accumulated since the previous report.
Output includes reasoning and tool arguments, matching billing. Response latency,
including time to first output, is included; tool lifecycles and user waits are
excluded. Tool boundaries follow the harness notifications: adapters which omit
an execution-start event may include some streamed arguments in the tool phase,
so this cannot promise exact decoder timing. The tooltip
shows the interval duration and when it was measured. While a new response is
still waiting to report tokens, the last confirmed rate stays visible rather than
dividing old tokens by that new wait. Before the first actual usage report, the
UI displays `— tok/s`, never a character-count estimate.

Finished turns use total output divided by confirmed response time. Unreported
idle tails after the last counter cannot dilute this value. Historical timing and
the legacy trailing 60-second fields remain supported by the UI. Total tokens,
price and speed stay inline; input/output/cache breakdowns live in price details.

Usage counters arrive at API-request boundaries, not necessarily per streamed
token. This is an interval average, not instantaneous decoder throughput. Stream
chunks can be batched and do not time hidden reasoning reliably. Claude ACP may
only report context occupancy during a turn, so the runtime also reads native
assistant JSONL usage as each response completes, including the response leading
to the first tool call. Repeated content blocks sharing a native message ID are
snapshots, never additive token charges. Old turns, sidechains and unrelated
sessions are excluded. Late ACP summaries cannot overwrite native turn totals.
If native usage is unavailable, ACP usage remains the fallback. Codex retains
its native rollout reader. Final timing is stored with turn usage and survives reload
and restart. Tools never alter billable token counts or price categories.

The per-request split (`splitMethod: "streamedRequests"`) counts every request that
produced output (`llmRequests`) and times only those whose output streamed for at
least 300 ms. A reply delivered in one piece (hidden thinking, then the tool call)
lands tens of milliseconds apart; it is counted in `burstRequests` and left out of
first token, prefill and decode. An agent message or thought opened before its first
token is not output. Decode divides a request's own `last` usage report by the time
its output streamed: from the first thought to the last chunk, all tokens, when the
thinking streamed (local models); otherwise the answer's (text and tool calls) time,
reasoning tokens left out. The turn total is not used, since late reports carry
earlier requests' tokens. A report that arrives after a quick tool, before the next
request outputs anything, still belongs to the request that wrote it. Snapshots
without `splitMethod` timed bursts as streams and are shown as the combined speed only.

Harness notices (ACP `notice` updates, advertised with `clientCapabilities.session.notices`)
become their own `generic` items with `origin: "sessionNotice"` and a `severity`:
Codex's warning about an unknown (local) model no longer arrives as reply text, so
it neither joins the answer nor counts as the first token.

Local-model accuracy check (opt-in): run `e2e/fixtures/fake-local-responses.mjs`, a
Responses API server with known prefill and decode rates that streams reasoning and
answer token by token and logs each request's truth. Point an isolated `CODEX_HOME`
at it (`wire_api = "responses"`, `env_key`), start an isolated Supervisor without the
fake runtime, prompt a Codex thread and compare `generationSpeed` with the log. At
3000/40, 800/150 and 20000/15 tok/s (prefill/decode) decode stayed within 1%; prefill
within 2%, except about 10% at 20000 tok/s, where the harness's ~40 ms request
overhead is a tenth of the wait.

Targeted checks:

```sh
cargo test -p pockymoe-runtime service::generation --lib
cargo test -p pockymoe-runtime gpt_61_sol --lib
cargo test -p pockymoe-runtime --test generation_speed
cargo test -p pockymoe-runtime acp::claude_usage --lib
```

A real Claude check uses an independent database/workspace/session and the
advertised Haiku model. It verifies positive speed during the first tool wait,
then checks the final persisted usage. It never restarts the host Supervisor:

```sh
cargo test -p pockymoe-runtime --test claude_usage_live -- --ignored --nocapture
```

The real-token browser acceptance is opt-in. Copy credentials/config into a new
isolated Codex home with owner-only permissions; do not print or commit them.
Use independent database/workspace paths and ports, then run only:

```sh
E2E_REAL_CODEX=1 E2E_CODEX_HOME=/absolute/isolated/codex-home \
E2E_API_PORT=19987 E2E_WEB_PORT=16273 \
E2E_DATABASE_URL=.local/token-speed.sqlite \
E2E_WORKSPACE_ROOT=.local/token-speed-workspaces \
pnpm exec playwright test e2e/token-speed.spec.ts --project=desktop-chromium
```

The scenario uses exactly `gpt-6.1-sol`, checks the live footer during a real tool
wait, verifies all price categories and completed speed, and reloads at desktop
and phone widths. It never touches an active device Supervisor or relay.
