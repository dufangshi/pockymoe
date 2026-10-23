# pockymoe guide devices

Threads, files and mail on your other devices.

## Other devices

Devices must belong to the same relay owner, and the target must accept requests:
peer access on and not isolated. This device's own peer access and isolation only
close the way in; they never stop you from reaching other devices. The
CLI still connects to your local Supervisor; it routes encrypted requests through
the relay. Use a relay device ID or a unique device name (case insensitive).
`device list` works before opting in and reports this device's `peerAccess`.

An isolated device answers only `device list`/`info` (with `isolated: true`); an
isolated workspace is missing from `device workspaces` and listings, and requests
for its threads or files are refused. The owner sets this on the Devices and
Workspaces pages; do not work around it.

A device shared with people who can control its agents (`sharedControl: true` in
`device list`) cannot reach other devices (`peer_source_shared`); report it, do not work around it.

```bash
pockymoe device list
pockymoe device access                    # inspect; managed threads can view
pockymoe device access on                 # local machine credential required
pockymoe device workspaces DEVICE
pockymoe thread list --device DEVICE --workspace WORKSPACE_ID
pockymoe thread backends --device DEVICE
pockymoe thread models --device DEVICE --workspace WORKSPACE_ID
pockymoe thread create --device DEVICE --workspace WORKSPACE_ID --title helper
pockymoe thread status DEVICE/THREAD_UUID
pockymoe transcript DEVICE/THREAD_UUID --view summary
pockymoe transcript DEVICE/THREAD_UUID --limit 1
pockymoe thread send DEVICE/THREAD_UUID --text 'Please inspect these files' \
  --attach ./report.txt --attach ./sources
pockymoe fs ls DEVICE --workspace WORKSPACE_ID
pockymoe fs get DEVICE --workspace WORKSPACE_ID path/to/file --out ./copy
pockymoe outbox
```

The delivery policy is the same across devices: queue needs `--kind task`, and
direct/steer need `--interrupt-reason`. Remote creation requires an existing target workspace and creates no local lineage;
it does not inherit your approval mode. Remote send supports inbox/direct/queue/steer
and `--notify-on-complete`; results return to your local passive inbox. Reply to
cross-device mail using its `replyTo` (`DEVICE/THREAD_UUID`). Remote wait, wake, tree,
task, close, delete and inbox operations are unavailable; inspect status/transcript
or wait for local inbox results instead. `DEVICE/THREAD_UUID` with this device's own
ID is a local thread, and every command accepts it.

Attachments copy up to 20 local paths; directories become zip files. The receipt's
`attachments` and the delivered message both name where each file landed on the target. `fs` reads only within a target
workspace. Downloads default to the caller's `.temp/threads/THREAD/downloads/`
directory (or workspace `.temp/downloads/` without a caller).

Retryable relay/offline/timeout failures save only inbox/queue sends in this device's
outbox (`delivery: "outboxed"`); direct/steer and create fail immediately. Outboxed
mail retries for seven days, then reports failure to the local sender's inbox.
Identity changes stop delivery. Verify the peer's `pockymoe relay-fingerprint`
before `pockymoe device trust DEVICE --reset` with a local machine credential.
