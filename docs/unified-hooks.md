# Device hooks / automations

`pockymoe automation` (aliases `hooks`, `hook`), REST and the thread's
**Automations** button use one Rust Supervisor registry and SQLite execution
ledger. This works with Codex and ACP harnesses as well as Claude. Native Claude
watches stay a separate read-only projection: they are never imported or duplicated.
Recorded active/unconfirmed native watches block registering a Supervisor prompt
against that same session until a successful cancellation is recorded.

An automation belongs to its target thread. Its source can be every subsequent
complete turn of another local thread, a precise turn, lineage task or controlled
command. Only a registered
`prompt` action wakes a thread; results/reminders and command execution reports
always enter passive inbox, both when idle and during a turn. Reading an inbox
never starts execution. Existing queue/inbox/task/wake rules remain in force.

## CLI examples

Read `pockymoe skill` and use `thread self` / `POCKYMOE_THREAD_ID` for the
Pockymoe thread ID. The following create schedules only when explicitly run.
They require an existing local device connection and an open target thread.
Use disposable threads/workspaces when experimenting.

```sh
# Every hour, starting an hour after registration. Busy ticks merge into one pending.
pockymoe automation create --thread self --request-id hourly-check --json '{
  "name":"Hourly check",
  "trigger":{"kind":"interval","everySeconds":3600},
  "action":{"kind":"prompt","text":"Check the authorized project and report findings."}
}'

# One UTC/offset date. Choose a future date explicitly.
pockymoe hooks create --thread self --json '{
  "name":"One reminder",
  "trigger":{"kind":"at","at":"2030-01-01T12:00:00Z"},
  "action":{"kind":"notifyInbox","subject":"Reminder","text":"Review the report.","messageKind":"status"}
}'

# Listen to another local thread, without knowing its current or future turn IDs.
# Set SOURCE_THREAD_ID to its Pockymoe thread UUID first.
pockymoe hooks create --thread self --request-id source-thread-results --json "{
  \"name\":\"Other thread results\",
  \"trigger\":{\"kind\":\"threadEnded\",\"sourceThreadId\":\"$SOURCE_THREAD_ID\"},
  \"condition\":{\"kind\":\"statusIn\",\"values\":[\"completed\",\"failed\",\"interrupted\"]},
  \"action\":{\"kind\":\"notifyInbox\",\"subject\":\"Source thread ended\",\"text\":\"A complete source turn ended.\",\"includeClosingMessage\":true}
}"

# Full JSON files and typed predicate combinations use the same server validator.
# Fill SOURCE_THREAD_ID and TURN_ID from a real thread/turn before running this block.
cat > turn-hook.json <<EOF_JSON
{
  "name":"Build finished",
  "trigger":{"kind":"turnEnded","sourceThreadId":"$SOURCE_THREAD_ID","turnId":"$TURN_ID"},
  "condition":{"kind":"all","conditions":[
    {"kind":"statusIn","values":["completed"]},
    {"kind":"not","condition":{"kind":"statusIn","values":["interrupted","failed"]}}
  ]},
  "action":{"kind":"notifyInbox","subject":"Build result","text":"The selected turn finished.","includeClosingMessage":true}
}
EOF_JSON
pockymoe automation preview --thread self --file turn-hook.json
pockymoe automation create --thread self --file turn-hook.json --request-id build-reminder

# Successful lineage task #4: replace ROOT_ID with its root Pockymoe thread ID.
pockymoe automation create --thread self --json "{
  \"name\":\"Task result\",
  \"trigger\":{\"kind\":\"taskEnded\",\"rootThreadId\":\"$ROOT_ID\",\"taskNumber\":4},
  \"condition\":{\"kind\":\"statusIn\",\"values\":[\"completed\"]},
  \"action\":{\"kind\":\"notifyInbox\",\"subject\":\"Task complete\",\"text\":\"\",\"includeClosingMessage\":true}
}"
```

Immutable turn/task/command-ID subscriptions reject already ended sources with
`sourceAlreadyEnded`. Set `replayExisting: true` to explicitly run once immediately
instead. Register against the actual immutable turn ID, not "the next turn".
`commandKey` subscriptions match each new controlled wrapper execution of that key.

`threadEnded {sourceThreadId}` subscribes continuously to the source's complete
turns ending **after registration**, including a turn already running at registration.
Each turn produces at most one occurrence, for `completed`, `failed` or `interrupted`;
typed `statusIn` can filter these statuses. Tools/batches, idle state and closing or
deleting a thread do not produce completion occurrences. Registration never replays
history; `replayExisting: true` is rejected with `replayUnsupported` for this trigger.
At the next scheduler check, source closure/deletion pauses this subscription with
`sourceUnavailable`, removing its unexecuted actions. A closed source must reopen
before explicit resume; paused events are discarded, and reopening alone never
resumes the subscription.

```sh
# Register a script after this thread's controlled `focused-build` wrapper succeeds.
# This example uses fixed argv, explicit cwd and a bounded timeout.
cat > command-hook.json <<EOF_JSON
{
  "name":"After build",
  "trigger":{"kind":"commandEnded","sourceThreadId":"$POCKYMOE_THREAD_ID","commandKey":"focused-build"},
  "condition":{"kind":"exitCodeEquals","value":0},
  "action":{"kind":"runScript","argv":["/bin/sh","scripts/report-build.sh"],"cwd":".","timeoutSeconds":60}
}
EOF_JSON
pockymoe hooks create --file command-hook.json --request-id after-focused-build
pockymoe command run --thread self --command-key focused-build --request-id build-1 \
  --cwd . --timeout-seconds 120 -- cargo check -p pockymoe-runtime
# Inspect the commandId returned above; output and exit status persist.
pockymoe command show --thread self COMMAND_ID

# Time and precise completion events can execute scripts too; explicit shell is supported.
pockymoe automation create --json '{
  "name":"Hourly script",
  "trigger":{"kind":"interval","everySeconds":3600},
  "action":{"kind":"runScript","shell":"./scripts/check.sh > check-result.txt","cwd":".","timeoutSeconds":60}
}'

pockymoe automation list --thread self
pockymoe automation show --thread self AUTOMATION_ID
pockymoe automation runs --thread self AUTOMATION_ID --limit 20
pockymoe automation pause --thread self AUTOMATION_ID
pockymoe automation resume --thread self AUTOMATION_ID
pockymoe automation cancel --thread self AUTOMATION_ID
pockymoe inbox list --kind result --kind status
```

`argv` and `shell` are mutually exclusive. On Unix shell uses `/bin/sh -c`; on
Windows it uses `cmd.exe /C`. Relative cwd resolves against the target workspace;
absolute cwd is accepted under existing device access permissions. These are
ordinary processes under the Supervisor's OS user, not a separate sandbox. No
new grants, script hashes or trust approval workflow is required.

Commands receive a cleared environment with PATH, command ID and (on Windows)
standard OS/temp paths. Connection tokens, CLI config paths, HOME and upstream
keys are not automatically copied into scripts. Stdout and stderr each retain
at most 64 KiB while draining the pipe; excess is discarded. Timeout is 1–300
seconds. The device allows at most four commands simultaneously. Unix timeout
kills the owned process group; Windows uses taskkill /T /F (Windows execution is
implemented but was not part of Linux validation).

## Definition and reliability

Triggers: `interval {everySeconds, anchorAt?}`, `at {at}`, `threadEnded {sourceThreadId}`, `turnEnded
{sourceThreadId, turnId}`, `taskEnded {rootThreadId, taskNumber}`, `commandEnded
{sourceThreadId, commandId?, commandKey?}`. UTC interval anchors do not drift with
execution duration or DST. Dates require RFC3339 with an offset. There is no cron,
DST wall-clock scheduler, cross-device schedule or arbitrary PTY command observer.

Typed conditions: `all/any {conditions}`, `not {condition}`, `statusIn {values}`,
`exitCodeEquals {value}`, `workspaceId {value}`, `commandId {value}`,
`replyLengthAtLeast {chars}`. The reply condition counts Unicode characters in
the closing reply's prose, excluding fenced and inline code; it is false without
a closing reply. Conditions
never run shell/JavaScript. Exit-code equality requires a known exit code and a
`completed` or `failed` status, so a normal nonzero exit can match its exact code;
unknown exits/timeouts never match. Condition nesting is capped
at eight levels. Script argv/output are available only with existing control access.

Trusted built-in plugins can declare `pluginAction {pluginId, action, input,
timeoutSeconds}` actions. The Supervisor validates the registered action, plugin
availability and enabled state before acceptance. Execution waits for an idle
target, uses an immutable settings snapshot, and records bounded output in the
same run ledger. Settings snapshots remain private. Plugin actions have a device
limit of four concurrent invocations and one per plugin, with a timeout of 1–300
seconds. Disable cancels pending invocations; accepted running invocations may
finish. Restart marks unfinished invocations `uncertain` without retrying them.
See [built-in plugins](builtin-plugins.md) for extension and manual-action APIs,
and [Content Quality](content-quality.md) for the reply and document editor.

Default `missedRunPolicy` is `coalesceLatest`, `maxLatenessSeconds` is 86400.
Optional `skip` skips accumulated missed ticks. Busy/downtime intervals advance
nextRunAt while preserving at most one unexecuted occurrence per schedule; merged
extra ticks increment missedCount. One running action may also have one pending
occurrence. Scripts wait for the target's entire turn and existing continuation
queue to finish. A queued prompt uses the normal whole-turn queue admission.

Events commit alongside turn/task/command terminal state. Event keys and occurrence
keys are unique. Action intent is saved in the run's immutable definition snapshot.
Inbox/continuation acceptance and the run receipt commit in one local transaction:
the run row is the local durable outbox. The database's existing exclusive process
ownership plus transactional state guards avoids multiple dispatchers; no network
lease or separate daemon is involved. Stable CLI request IDs return the original
receipt/execution; conflicting definitions are rejected. A command request is never
re-spawned when retried, even if the first request lost its response.

`queued` means accepted, not successful execution. Prompt admission binds
run → pendingSteerId → turnId atomically; turn terminal state records execution
completion. Script intent is durable before spawn and real wait records exit/output.
Restart changes starting/running commands without saved completion to `uncertain`
and emits a passive result. They never auto-retry. A child may continue after an
abrupt OS/process crash; inspect external effects and process state before deciding
to create a new execution. External side effects cannot be exactly-once guaranteed.

Command and turn events carry automation ancestry; repeated automation IDs or
ancestry depth ≥3 are skipped. Script completions have no wrapper commandKey, so
ordinary named wrapper hooks cannot recursively match their own script executions.
No PTY/transcript text is interpreted as an executable event.

Pause/cancel atomically remove only this automation's unexecuted queue entries.
Running work is allowed to finish. Cancel is permanent; resume starts from future
ticks and discards paused events. Resume on an already enabled definition is a
no-op: retries preserve its event cursor, nextRunAt and anchor. The UI preserves
the create request ID across lost-response retries of the same definition and
target thread, clearing it after success. User Stop pauses that thread's prompt automations,
so they do not secretly wake it again. Closed/missing targets and deleted sources
pause schedules with a visible reason. Recovering threads defer actions and never
silently reconnect or reopen. Native historical imports do not generate script events.

Definitions, event journal and execution history are retained in the database;
automatic history retention/pruning and editable definition revisions are future
work. Create a replacement definition explicitly when changing a hook.

## HTTP

All public DTO fields are camelCase. Existing device/relay control permissions apply.

```text
GET/POST /api/threads/{threadId}/automations
POST     /api/threads/{threadId}/automations/preview
GET      /api/threads/{threadId}/automations/{automationId}
POST     /api/threads/{threadId}/automations/{automationId}/pause|resume|cancel
GET      /api/threads/{threadId}/automations/{automationId}/runs?limit=20
POST     /api/threads/{threadId}/commands
GET      /api/threads/{threadId}/commands/{commandId}
```

Create body: `{ "definition": { ... }, "clientRequestId": "stable-key" }`.
Preview body: the definition directly. Command body: `{ "argv": ["..."],
"cwd": ".", "timeoutSeconds": 60, "commandKey": "build",
"clientRequestId": "build-1" }`. CLI operations use existing `/api/cli` and call
these same runtime services. Read-only shares can inspect definitions/history;
control is required to create/control hooks or access command execution/output.

For example, POST to `/api/threads/TARGET_A/automations` to monitor source B:

```json
{
  "clientRequestId": "listen-b-v1",
  "definition": {
    "name": "Source B completed turns",
    "trigger": { "kind": "threadEnded", "sourceThreadId": "SOURCE_B" },
    "condition": { "kind": "statusIn", "values": ["completed"] },
    "action": {
      "kind": "notifyInbox",
      "subject": "Source B result",
      "text": "A complete source turn ended.",
      "includeClosingMessage": true
    }
  }
}
```

Replace TARGET_A/SOURCE_B with actual local Pockymoe thread IDs. The target
is selected by the URL; no turnId is required. The same trigger supports explicit
prompt or runScript actions through the existing action and busy-turn rules.

Web changes require the independent shared UI commit; the public Web picks it up
when `thread-ui.sha` on `main` is set to that full SHA, which deploys GitHub Pages.
A device Supervisor restart alone does not publish UI.
