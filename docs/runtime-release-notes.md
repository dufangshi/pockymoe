Claude native subagents stopped when a session ends now appear as interrupted
instead of remaining running indefinitely. SDK notifications are matched by
native task ID even when no tool-call ID is present, and the runtime counter
stops at the notification time. Older child transcript records cannot undo a
newer stop/completion notification; actual resumed work still updates normally.
Live background-agent tracking also recognizes the native `stopped` status.

Private port previews reuse fully consumed HTTP connections within the exact
account/session/mapping authorization, and disable Nagle on Relay and device
data sockets. The published Web entry can open an already authorized CLI preview
without first loading the entire workbench. These Relay/Web improvements are
already deployed and work with older Supervisors; device-side TCP_NODELAY takes
effect after a normal runtime update.

Targeted native-agent runtime and Supervisor HTTP regressions cover stopped
notifications, old-record replay, transcript replacement, resumption, scoped
access and unchanged accounting. Port preview regressions and isolated HTTP,
SSE and WebSocket benchmarks were also verified. Windows Device Manager is
unchanged. Installing this release is a separate device update; publication
does not restart an existing Supervisor.
