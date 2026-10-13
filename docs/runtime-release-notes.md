Harness processes are now released when nobody needs them, instead of staying
loaded until the Supervisor exits. Opening a thread loads its harness; leaving
it without sending a message releases it about 10 seconds later. A thread that
no page shows and that has no work in progress (running turn, pending approval
or question, queued input, background task, subagent, goal, Claude scheduled
loop or wakeup) is released after 10 idle minutes. The next message reloads it
as before. Releasing a Codex thread also drops its writer lock, so the Codex
desktop app no longer reports the thread as in use by Pockymoe.

Settings → Resources sets the idle time and optional limits on loaded threads
and harness memory. The device monitor's Memory row expands to the
Supervisor's total and each loaded thread's adapter, agent, MCP and tool
processes, with an End button. Only the owner and device-scoped grants can see
the list or change the settings; read-only viewers never keep a harness
loaded. A new thread that was released before its first message starts a fresh
session on that message. Explorer previews now show PDFs and linked Markdown
images on the static Web origin.

Validated with the runtime and Supervisor suites, Web unit tests, and a live
run against real Claude and Codex adapters on an isolated Supervisor
(release, view, idle, limit, delete, Codex writer lock, and the browser
monitor and settings). The mobile apps do not yet report which thread is on
screen, so a thread left open there is released after the idle time and
reloads on the next message. Windows Device Manager is unchanged.
