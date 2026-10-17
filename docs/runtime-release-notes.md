Speed details now say how a turn's model time was really spent. Tapping a turn's
tok/s lists how many LLM requests it made, then the time to first token, prefill
and decode, measured only on requests whose output actually streamed. Replies that
arrive in one piece (hidden thinking, then the tool call) no longer produce decode
speeds in the tens of thousands of tokens per second; the details say how many such
requests were left out. Decode uses each request's own usage report instead of the
turn total, so late reports no longer inflate it, and thinking that streams (as it
does with local models) is timed with the answer. Against a local model server with
known rates, through real Codex, decode came within 1% and prefill within 2%.
Turns measured by earlier versions show only the combined speed.

Harness notices are their own rows. Codex's warning about an unknown model (every
local model triggers it) used to arrive as reply text before the answer; it now
appears as a small warning row and is not counted as the model's first token.
Claude's model-fallback notices are shown the same way.

Agent threads stay inside their conversations. The running count counts
conversations, and the running sheet, unread list and notifications show an agent
thread under its conversation's title rather than on its own. The tab bar lists the
conversations started in the workspace with all their agent threads.

On phones, windows opened from More (native subagents, automations, split view)
rise as their own sheet instead of being squeezed into the tool grid.

Validated with the runtime, Web and shared UI suites, a mapper test for notices,
tracker tests for bursts, late reports and streamed thinking, and the local model
check described in docs/turn-token-speed.md. Windows Device Manager is unchanged.
Installing this release is a separate device update; publication does not restart
an existing Supervisor.
