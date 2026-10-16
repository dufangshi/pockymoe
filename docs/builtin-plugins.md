# Built-in feature plugins

Pockymoe feature plugins are trusted modules compiled into the Rust runtime and
the shared Web UI. They ship with the application; manifest imports, dynamic
code loading and independent installation are not supported. Terminal, DeepSeek
Harness and [Content Quality](content-quality.md) use this registry.

## Backend registration

Implement `runtime::plugins::BuiltinPlugin` and register the module in
`PluginHost::default`, or use `Supervisor::with_builtin_plugins` before sharing
the Supervisor. Plugin IDs are unique and stable. Existing `remote-codex.*` IDs
remain unchanged for compatibility.

The trait supplies the descriptor, default enabled state, default settings,
settings validation, declared actions, input validation and asynchronous action
handler. Typed validation belongs to the plugin. Descriptors report availability;
unavailable plugins cannot be enabled or invoked.

Settings are device-local objects, stored under the plugin's own key. They must
not contain credentials: use a reference to a private connection file instead.
Personal/account and workspace configuration are not generic scopes in this API.

```text
GET   /api/plugins
GET   /api/plugins/{id}
PATCH /api/plugins/{id}                  {"enabled":true}
GET   /api/plugins/{id}/settings
PUT   /api/plugins/{id}/settings         {"revision":0,"values":{...}}
```

Settings reads and writes return `{revision, values}` with `Cache-Control:
no-store`. A write replaces the object, validates it and increments the revision.
An outdated revision returns HTTP 409; reload before saving again. Both settings
routes use existing device owner authorization, not shared-thread access.

## Actions and hooks

An automation can call a declared plugin action:

```json
{
  "kind": "pluginAction",
  "pluginId": "pockymoe.content-quality",
  "action": "rewriteReply",
  "input": {},
  "timeoutSeconds": 120
}
```

The handler receives its execution ID, target thread, triggering event, input,
settings snapshot and Supervisor. Thread-end actions can read `event.turnId`.
The `replyLengthAtLeast` condition counts Unicode prose characters, excluding
fenced and inline code; it tests the source event's closing message.

Manual actions use the same execution ledger:

```text
POST /api/threads/{thread}/plugins/{plugin}/actions/{action}
     {"input":{...},"clientRequestId":"unique-key","timeoutSeconds":120}
```

The response is an automation receipt, not the completed action. Inspect its
normal `/api/threads/{thread}/automations/{id}/runs` endpoint. Reusing the request
ID with the same action returns the original receipt; different input conflicts.
Action output appears in `deliveryReceipt.output`. Private settings snapshots
are omitted from the public receipt.

Actions wait for the target's active turn and continuation queue to finish. At
most four plugin actions run on a device, and one per plugin. Execution intent
commits before invocation; output and errors persist afterward. Disabling a
plugin cancels unstarted work and rejects new invocations; accepted running work
may finish. A timeout or panic becomes a failed execution. After a Supervisor
restart, an in-flight invocation becomes `uncertain` and is not automatically
retried because its external effects may already have occurred. Plugin handlers
must make their own repeated requests safe and respect cancellation when
launching child work. Settings and inputs are limited to 64 KiB, output to 256
KiB, and timeouts to 1–300 seconds.

A plugin can provide a pure `turn_automation` definition. The host creates or
updates one managed hook per plugin/thread during turn settlement. The callback
must not perform I/O or model calls. Settings changes refresh existing managed
hooks; disabling automatic behavior cancels their pending work. Explicitly
paused or cancelled hooks are not silently resumed. Hook registration and the
turn-end event share the terminal-state transaction. Plugin configuration errors
are logged without undoing the agent's completed turn.

## Frontend settings

Register a `FrontendPluginModule` with `PluginProvider`. A module's
`settingsSections` contributions supply labels and React content. Their context
provides namespaced `load()` and revision-checked `save()` methods through the
host adapter. IDs become `plugin:{pluginId}:{sectionId}`.

The host adds these sections to its ordinary settings navigation. Sections stay
available while a plugin is disabled so it can be configured before enabling;
unavailable or unregistered device modules do not contribute settings. Settings
storage remains on the device, not in the component or browser local storage.

`browserSettingsSections` are the exception for per-browser preferences: they
render their own content, appear only while the plugin is enabled and available,
and keep nothing on the device. The terminal's Settings > Terminal uses one for
its clipboard behaviour (see [thread interaction](thread-interaction.md#terminal-clipboard)).

Frontend modules and Rust modules release with their respective applications.
The public Web build consumes the shared UI commit pinned by `thread-ui.sha`;
updating a device alone does not deliver new Web components.

Modules can also provide `presentMessage` for an alternate reply body and
`renderActionOutput` for readable execution reports. The message host retains
its Markdown rendering, copy controls and original-body toggle; asynchronous
validation can reject a derivative whose source no longer matches. Message
presentation requires an enabled plugin. Historical action reports remain
readable after disabling one. `PluginActionOutput` falls back to bounded JSON
for unknown output formats and retains raw output details for inspection.
