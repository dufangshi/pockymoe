Each turn now opens with the facts that used to sit under its answer, and the
speed explains itself.

- **Turn head**: a line above each turn's prompt shows when the turn started, the
  model and reasoning effort, the API cost and the combined output speed. While
  the turn runs it shows a pulsing dot and the time so far. Tapping it shows the
  start and end, the time taken, the tokens and how many prompts were added while
  it worked. The receipt under the answer is gone, so a turn's numbers no longer
  sit against the next prompt.
- **Added prompts**: a prompt sent while the turn was working says
  "Added · 17:12" above it.
- **Speed breakdown**: tapping the speed opens a breakdown, like the cost: the
  time to first token, the prefill speed over uncached input and the decode
  speed. The Supervisor records these per model request from this version on;
  older turns show the combined speed only.
- **Compact live marks**: waiting for a wake and running background agents show
  as a small turning hourglass and a bot with its count instead of phrases.
- **Reasoning slider**: the level names under the slider now take presses and
  drags too, so a drag that starts slightly off the thin track still moves it.

Validated with the shared UI and Web test suites, the runtime generation-speed
and Claude usage tests, and phone screenshots in the default, Plum and Terminal
themes. Relay and Windows Device Manager are unchanged. Installing this release
is a separate device update; publication does not restart an existing Supervisor.
