The context panel now shows how a thread's context grew and what it cost, not
only what it holds now.

- **Trend**: the context panel has a Trend view with one bar per model request,
  or per turn, stacked by what the context held: system prompt, tool
  definitions, memory files, skills, user messages, model output, tool results
  and injected content. Switch to Change to see what each request added or
  dropped. A strip under the bars shows model time and tool time, and the
  selected bar's figures are listed below the chart.
- **Statistics**: cache hit rate for the thread, tokens read, read from cache,
  written to cache and produced, the makeup of everything the requests read, and
  model time against tool and waiting time with the output speed.
- **Finer categories**: the conversation is no longer one Messages figure. Codex,
  Claude and DSH threads split it into user messages, assistant messages, tool
  calls, tool results and injected content where the harness can tell them
  apart, with item counts.
- **Recording**: the Supervisor records every model request of a thread from this
  version on, for any harness. The per-request makeup needs Detailed context
  analysis to be on (Settings > Resources) and is available for Codex, Claude
  and DSH.
  Requests made before the update are not in the trend; the thread's statistics
  still cover them.
- Claude threads now refresh their context reading after each model request
  during a turn, not only when the turn ends.
