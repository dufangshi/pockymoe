# Installed plugins

A third-party plugin is a git repository with `pockymoe-plugin.json` at its
root. The Supervisor clones it, runs each action as a child process and runs
the manifest's schedules at device scope. Installed plugins appear in the same
registry as [built-in plugins](builtin-plugins.md): `/api/plugins`, device
enablement and device settings work the same way.

Plugins are trusted local code. They run as the Supervisor's OS user, like a
VS Code extension; permissions only decide what Pockymoe hands the process.
Install only repositories you trust.

## Manifest

```json
{
  "schemaVersion": 1,
  "id": "owner.calendar-agent",
  "name": "Calendar agent",
  "version": "0.1.0",
  "description": "Plans events and reminders into iCloud.",
  "entry": ["uv", "run", "--project", ".", "calendar-agent", "plugin"],
  "setup": ["uv", "sync"],
  "actions": [
    { "name": "tick", "timeoutSeconds": 120 },
    { "name": "status", "description": "Connection and queue summary" }
  ],
  "schedules": [{ "action": "tick", "everySeconds": 60 }],
  "permissions": ["pockymoeCli"],
  "defaultSettings": {}
}
```

- `id` is a dotted name; `pockymoe.` and `remote-codex.` are reserved. The
  install directory is named after it, and an update may not change it.
- `entry` is the argv for every action. A first element containing `/` is
  relative to the plugin directory; otherwise it is looked up on PATH.
- `setup` runs once in the plugin directory after install and after each
  update (for example `uv sync` or `npm ci`), with a 300 second limit. A failed
  setup removes a fresh install.
- `actions`: at most 32, names of letters, digits, `-` and `_`;
  `timeoutSeconds` 1–300 (default 60).
- `schedules`: at most 8, each naming an action, `everySeconds` from 10 to one
  year.
- `defaultSettings` is the settings object before the owner saves any. Device
  settings reject credential-looking keys, as for built-in plugins.

## Lifecycle

```bash
pockymoe plugin install https://github.com/owner/calendar-agent.git [--ref main]
pockymoe plugin list                 # read capabilities.permissions first
pockymoe plugin enable owner.calendar-agent
pockymoe plugin run owner.calendar-agent status --input '{}'
pockymoe plugin runs owner.calendar-agent
pockymoe plugin update owner.calendar-agent
pockymoe plugin disable owner.calendar-agent
pockymoe plugin remove owner.calendar-agent [--purge-data]
```

Installing never enables a plugin or runs an action; only `setup` runs. These
commands need the machine's own CLI credential: a thread's scoped token or a
peer device is refused. The HTTP equivalents are owner-only routes:

| Route | Purpose |
| --- | --- |
| `POST /api/plugins/install` `{source, ref?}` | Clone, set up, register disabled |
| `POST /api/plugins/{id}/update` | `git pull --ff-only`, rerun setup |
| `DELETE /api/plugins/{id}?purgeData=true` | Uninstall; built-ins answer 409 |
| `POST /api/plugins/{id}/actions/{action}` `{input}` | Run at device scope, returns `{output}` |
| `GET /api/plugins/{id}/runs` | Last 20 device-scope runs |

Installed code lives in `<database>.plugins/<id>/`. Its private data directory
`<database>.plugin-data/<id>/` (mode 0700) survives updates and uninstall
unless purged. Removing a plugin deletes its settings and cancels its pending
plugin actions. The install record (`source`, `ref`, `commit`, times) is in the
descriptor's `capabilities.install`.

## Action protocol

Each action spawns `entry` in the plugin directory. The request is one JSON
object on stdin:

```json
{
  "action": "tick",
  "scope": "device",
  "trigger": "schedule",
  "runId": "…",
  "input": {},
  "settings": {}
}
```

`scope` is `device` for schedules, `pockymoe plugin run` and the device action
route (`trigger` is `schedule` or `manual`). An action can also run on one
thread through an ordinary `pluginAction` hook or
`POST /api/threads/{thread}/plugins/{id}/actions/{action}`; then `scope` is
`thread` and the request carries `threadId` and `event` as for built-in plugins.

The process writes one JSON value to stdout (empty output is `null`) and exits
0. A nonzero exit fails the run with the last 2000 characters of stderr. Stdout
is limited to 256 KiB and stderr to 64 KiB. A timeout kills the process group
and records a failure; external effects may be incomplete. One device-scope run
per plugin is active at a time; a schedule that comes due while one runs waits
for the next tick. A schedule records its start before running, so a crash
skips a tick rather than repeating it. Make actions idempotent.

The environment is cleared like a [hook script](unified-hooks.md) (PATH,
HOME, locale and similar basics only), plus:

| Variable | Value |
| --- | --- |
| `POCKYMOE_PLUGIN_ID` | The plugin id |
| `POCKYMOE_PLUGIN_DIR` | The installed repository |
| `POCKYMOE_PLUGIN_DATA_DIR` | Private data directory |
| `POCKYMOE_URL`, `POCKYMOE_TOKEN` | Only with the `pockymoeCli` permission |

PATH starts with the Supervisor's own `pockymoe` link, so the CLI works as is.

## Permissions

`pockymoeCli` hands the plugin the device owner's CLI credential. With it the
plugin can create and message threads (for example a DeepSeek Harness thread
the plugin keeps for conversations), wait for results, register hooks and call
`modelComplete`:

```json
{"operation":"modelComplete","connectionId":"…","model":"","system":"…","user":"…","maxTokens":1024}
```

`modelComplete` sends one prompt through a saved upstream connection
(Settings → Upstreams, any `openai_chat`, `openai_responses`, `anthropic` or
`gemini` connection) and returns `{"text": …}`; an empty model uses the
connection's default. The plugin never sees the connection's key. Store the
connection id, not a key, in the plugin's settings.

`workspaceEnsure` `{"absPath": "/abs/dir", "label": "…"}` creates the directory
if needed and returns the workspace registered there, adding it once. A plugin
uses it for the threads it keeps, for example under `POCKYMOE_PLUGIN_DATA_DIR`.

A plugin without permissions only receives its request and directories.

## Not yet supported

- Plugin-provided Web UI. Installed plugins have no frontend module; the
  shared UI lists them with the generic plugin entry.
- Long-running plugin processes. Each action is a separate process.
