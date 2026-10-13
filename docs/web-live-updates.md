# Web live updates and request budget

An open thread page learns about changes from the Supervisor WebSocket (`/ws`,
or `/relay/devices/<id>/ws` through a relay). HTTP reads follow the events that
announce a change; timers are only a slow fallback, and nothing is polled while
the tab is hidden. Requests a hidden tab would have made run when it is visible
again. `e2e/network-budget.spec.ts` counts the requests of an open thread while
idle, while a turn runs and while hidden, and fails when a change exceeds them.

## What refreshes what

| Data | Refreshed by | Fallback |
| --- | --- | --- |
| Thread summary (`/api/threads/<id>?view=summary`) | Current-thread `thread.turn.started`, `thread.updated`, goal and request events; a reconnected socket; a turn end without a following `thread.updated` (older Supervisors) | Every 3 s while a turn runs and no socket is open |
| Thread list (`/api/threads?includeAgentThreads=true`) | Other threads' `thread.turn.*` and `thread.updated` events, at most every 2 s; a reconnect | 60 s (10 s while the socket is down) |
| Capability snapshot (`/api/threads/<id>/capabilities`) | Current-thread turn, `thread.updated`, `thread.harness.ready` and collaboration events; 1.5 s and 6 s after the thread connects while the page is open, because ACP agents announce commands after a session opens; a reconnect | 60 s |
| `/healthz` | Every 2 s, only while the socket is not open | none |
| Device metrics (`/api/device/metrics`) | Opening the monitor dialog | 15 s for the topbar meter, 2 s while the dialog is open |
| Harness processes (`/api/device/processes`) | Expanding the monitor's memory row; `thread.runtime.loaded` / `thread.runtime.released` events | 3 s while the row is expanded |

The page loads the summary once on open. The first socket open re-reads it only
when the thread is running, since a running turn's events may predate the
subscription. A failed first load retries with backoff (3 s doubling to 60 s).
The agent status, models and capability bundle loads once per provider.

The thread list is the single source for the page, its workspace tabs and the
workbench navigation. In relay mode the navigation reuses it for the current
device, and re-reads other devices' lists every 30 s and on return to the tab.

Thread renames and deletions made elsewhere emit no event; they appear on the
list fallback or on return to the tab. A rename in this browser updates locally.

Socket pings run every 2 s; a socket without a pong for 5.5 s is replaced.

The page also tells its socket which threads it displays, so the Supervisor
keeps their harnesses loaded: `{"type":"thread.view","threadIds":[...]}` with the
full set, sent when the set changes, when a socket opens, and when the tab
becomes visible again. The set is the primary thread plus a split pane showing a
conversation; a split on another device reports on that device's socket. The
split's files and collaboration views, hidden panes and view-only shares do not
count, and the split reads capabilities with `?load=false` so it never starts a
harness. A tab hidden for 60 s sends an empty set.

## First-paint bundle

`apps/supervisor-web/vite.config.ts` groups vendor packages into chunks but
leaves the shared UI (`@pockymoe/thread-ui`) and all stylesheets to Rollup's
default chunking. The shared UI's lazy imports (Monaco editor and diff, the
workspace panel with the graph view, xterm, Mermaid) therefore stay separate
chunks loaded on use. Assigning the shared UI, or a stylesheet such as
`xterm.css`, to a manual chunk turns that chunk into a static dependency of the
entry and puts it back into first paint. `lucide-react` resolves to its ES module
build so unused icons are tree-shaken.
