# Built-in feature plugins

Built-in feature plugins are trusted modules compiled into the Rust runtime and
the shared Web UI. They ship with the application. Terminal, DeepSeek Harness
and [Content Quality](content-quality.md) use this registry. Third-party
plugins installed from git share the same registry, settings and enablement;
see [Installed plugins](external-plugins.md).

## Backend registration

Implement `runtime::plugins::BuiltinPlugin` and register the module in
`PluginHost::default`, or use `Supervisor::with_builtin_plugins` before sharing
the Supervisor. Plugin IDs are unique and stable. Existing `remote-codex.*` IDs
remain unchanged for compatibility.

The trait supplies the descriptor, default enabled state, default settings,
settings validation, declared actions, input validation and asynchronous action
handler. Typed validation belongs to the plugin. Descriptors report availability;
unavailable plugins cannot be enabled or invoked.

Settings have device defaults and optional per-thread overrides. They must not
contain credentials: reference a saved local connection instead. Account and
workspace settings are not generic scopes in this API.

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

## Thread scope convention

Built-in descriptors advertise `capabilities.scopes: ["device", "thread"]`.
The device enabled flag is the master switch. Beside it, each plugin has a
default for conversations (`defaultEnabled`, on unless the owner turns it off):
what a conversation that follows the device gets. A thread's enabled override is
`null` (follow the default), `true` or `false`; it cannot enable an unavailable
or globally disabled module, but can turn on an enabled plugin whose default is
off. Missing overrides follow the default.

```text
PATCH /api/plugins/{plugin}   {"enabled":true} or {"defaultEnabled":false}
```

Turning the default off cancels unclaimed actions in the conversations it turns
off and refreshes their managed hooks, like a thread override would. Settings are either
`null` (inherit current device values) or a full validated object snapshot.
Changing an unrelated device default does not patch a thread's custom snapshot.

```text
GET /api/threads/{thread}/plugins
GET /api/threads/{thread}/plugins/{plugin}/settings
PUT /api/threads/{thread}/plugins/{plugin}/settings
    {"revision":0,"enabled":null,"values":null}
```

The list flattens `PluginDto`, with its `enabled` field resolved for the thread,
and adds `globalEnabled`, `defaultEnabled`, `enabledOverride` and
`settingsOverridden`. The thread choice labels its follow option with what it
resolves to: Default (on) or Default (off). Settings
return `{revision, globalRevision, enabled, values, effective:{enabled,values}}`.
`effective.values` is always an object. Writes compare the thread revision and
increment it, including reset-to-inherit writes, so stale requests cannot cross
a reset. HTTP 409 requires a reload. `globalRevision` identifies the current
device settings; it is separate from the thread's concurrency token.
Deleting a thread also removes its plugin overrides, including direct-child
deletion; other threads and device defaults remain intact.

Thread-read access can fetch the plugin list for rendering. Settings reads and
writes require thread control or device-owner access, because effective values
can include private reading preferences or legacy local paths. Existing manual
action authorization is unchanged. Shared access remains limited to its thread.

The same resolved state controls panels, actions and managed hooks, not just the
checkbox display. Updating a thread refreshes only its managed hooks. Device
updates refresh affected threads without discarding overrides. Disabling a
scope cancels unclaimed actions; claimed/running work may finish with the
settings snapshot recorded when dispatch committed. Restoring inheritance does
not replay completed turns.

The shared UI offers one thread plugin button and a list of effective states.
A plugin's existing settings contribution is reused with a scoped load/save
adapter, rather than a second plugin-specific form. Scope keys include the
device and thread; global management stays attached to the device provider.
Settings controls never silently turn inherited effective values into custom
snapshots. Historical derivative replies and action reports remain readable
when future plugin processing is disabled. Read-only views cannot change scope.

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

The host exposes a settings button on each contributing plugin's row under
Settings > Plugins. It opens a separate plugin settings dialog; plugin sections
do not add entries to the main settings navigation. Sections stay available
while a plugin is disabled so it can be configured before enabling; unavailable
or unregistered device modules do not contribute settings. Settings storage
remains on the device, not in the component or browser local storage.

`browserSettingsSections` are the exception for per-browser preferences: they
render their own content, appear only while the plugin is enabled and available,
and keep nothing on the device. The Terminal settings button under Settings >
Plugins uses one for its clipboard behaviour (see
[thread interaction](thread-interaction.md#terminal-clipboard)).

Frontend modules and Rust modules release with their respective applications.
The public Web build consumes the shared UI commit pinned by `thread-ui.sha`;
updating a device alone does not deliver new Web components.

Modules can also provide `presentMessage` for an alternate reply body and
`renderActionOutput` for readable execution reports. The message host retains
its Markdown rendering, copy controls and original-body toggle; asynchronous
validation can reject a derivative whose source no longer matches. Message
presentation can remain available for historical stored derivatives after
disabling a plugin. Historical action reports also remain readable. `PluginActionOutput` falls back to bounded JSON
for unknown output formats and retains raw output details for inspection.
