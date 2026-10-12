# Model prices and subscription usage

Settings → Global → Model pricing lists USD per million tokens for this device.
Add a model or edit its rates and comma-separated display-name aliases. Matching
ignores case, spaces and punctuation and also uses names advertised by the harness.
Overrides persist in the supervisor database and apply to historical estimates on
reload. Reset restores a bundled model or removes a custom model.

The cost tooltip separates uncached input, cache reads, cache writes, output and
reported reasoning tokens. Each row shows tokens and USD. Reasoning is part of
output billing, so the two displayed rows split that charge instead of adding it
twice. Prices estimate API token charges, even for subscription sessions; they do
not represent subscription invoices, tool charges or cache-storage charges.

Claude turn totals include native subagents under the owning session. The runtime
tails their usage records without loading execution history, counts each native
request ID once, and keeps the latest snapshot when its output grows. Parent and
child requests are priced using their own reported models, context sizes and
observation times. These request snapshots persist for historical repricing;
child activity does not change the parent model or context-occupancy display.
Older turns without these snapshots retain their previously recorded totals.

## Bundled rates

The catalog is `config/codex-model-pricing.json`, with source links and verification
dates per added model. Sources checked on 2026-09-05:

- [Claude pricing](https://platform.claude.com/docs/en/about-claude/pricing):
  Opus 5, Fable 5/5.1, Sonnet 5 and Haiku 4.5. Cache-write estimates use the default
  five-minute rate for writes without a reported TTL. Native Claude reports preserve the one-hour write subset and price it at the separate one-hour rate; mixed-duration writes are charged once at their respective rates. Existing records without TTL information retain the five-minute fallback.
- [xAI pricing](https://docs.x.ai/developers/pricing): Grok 4.5/4.6, including the
  long-context multiplier.
- [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing): text rates for
  3.1 Pro, 3.7/3.8 Flash and 3.1/3.5 Flash-Lite. The catalog includes context
  thresholds and the published end of promotional Flash rates.
- [DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing/): V4 Flash
  and Pro, with weekday UTC peak periods and the off-peak discount.
- [GLM pricing](https://docs.z.ai/guides/overview/pricing): 5.1, 5 and 4.7.
  GLM 5.3 is listed without rates because its price was not verifiable on this
  page. Missing rates show unavailable, never a fabricated zero-dollar estimate.

Live usage accumulates per-report prices. Repricing historical totals after a
custom rate change cannot reconstruct missing per-request context sizes or times;
it uses the saved last input size and turn start time.

Grok Build response deltas and final turn snapshots feed the same cost display.
The final snapshot replaces the streamed total rather than adding it again.
Older turns without token usage are backfilled from completed local Grok session
updates when those logs are still available. See
[ACP slash commands, forks and Grok usage](acp-slash-fork-and-usage.md).

## OAuth allowance

The small composer badge shows actual available windows, with remaining allowance,
reset time and observation time on hover/tap. Opening a thread reads a shared
device/harness cache; successful readings refresh about every five minutes.
In Plum Pocket, the meter sits inside the composer's bottom seam. Its opaque
detail panel opens above the composer in the browser's top layer, retaining the
current theme; tap outside or press Escape to dismiss it.
Switching between parent/child threads, turn completion and WebSocket reconnects
do not bypass this cache. The Supervisor also shares a five-minute provider cache
across viewers and serializes refreshes to avoid concurrent upstream requests.

Temporary failures preserve the last successful observation for at most thirty
minutes, dimmed and marked `~` / `last known`, without changing its timestamp.
Retries back off from five to ten, twenty and thirty minutes; a longer upstream
`Retry-After` is respected and `Retry-After: 0` never causes an immediate retry.
Expired windows remain hidden. API-key authentication, missing subscription
credentials or rejected OAuth credentials clear the cache. The response includes
`unavailable: true` for these definitive cases; older runtimes' ambiguous
`usage: null` responses are treated as temporary failures by the browser.

- Codex: read-only `account/read` and `account/rateLimits/read` on an independent
  app-server process. No thread is opened and no conversation writer is acquired.
- Grok: read-only ACP `_x.ai/billing`; a weekly allowance is shown as 7d, without
  inventing a 5h limit.
- Claude: verify OAuth authentication with `claude auth status`, then read the
  account usage endpoint using the existing local OAuth access token. An expired
  token hides the badge until the harness refreshes its credentials. Pockymoe
  does not rotate credentials or log tokens.

ACP threads select the account adapter using their agent ID. Usage reflects the
local harness account; custom wrappers or separate credential stores must expose
compatible account commands to be supported. Account usage is not available to
shared-thread viewers. OAuth credentials are never sent to the relay or browser.
