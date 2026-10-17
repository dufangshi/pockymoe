# Upstream connections

An API connection is saved independently of its consumers. Harnesses apply a
connection explicitly; plugins select a saved local connection without applying
it to a harness. Settings, harness pages, and plugin settings share one connection
form. Saving, model discovery, connection testing, and synchronization never write
harness configuration or restart a harness.

In Settings > Upstreams, manage saved connections first. Adding a connection
does not require selecting or installing a harness. Harness use is a separate
step: choose a harness and a compatible saved connection, then explicitly apply
it. Plugins select from the same saved connections in their own settings.
Connection tests request only a brief answer but allow 1,024 output tokens for
models that use part of that budget for reasoning. Incomplete responses still
fail the test.

## Local storage and migration

Connections are stored privately beside the Supervisor database in
`<database>.upstreams/connections.json`. API keys, custom header values, and
provider-owned native fragments remain on the device. List and save responses
omit API keys and mask custom header values. Empty keys preserve an existing key
only while the endpoint and protocol stay unchanged. Edits require the current
opaque `revision`; successful edits create a new random revision. Omitting
`headers` preserves the saved private header map on an unchanged endpoint and
protocol; an explicit empty object removes custom headers.

Existing saved CC Switch-derived profiles migrate with their IDs intact. Migration
retains provider-owned native options, custom headers, authentication mode, and
context windows, and records existing active bindings. It never changes native
files. Profiles with the same name remain separate connections. Removing a local
copy does not cause its legacy profile to be imported again automatically.

Supported protocols are `openai_chat`, `openai_responses`, `anthropic`, and
`gemini`. Protocol describes the wire API, not the model brand. For example,
Gemini models behind an OpenAI-compatible proxy use `openai_chat`. OAuth sessions
are not synchronized.

Harness compatibility follows the native adapters:

| Harness | Connection protocols |
| --- | --- |
| Codex | `openai_responses` |
| Claude | `anthropic` |
| Gemini CLI | `gemini` |
| Grok | `openai_chat`, `openai_responses` |
| DeepSeek Harness | `openai_chat`, `anthropic` |

Grok's native adapter does not support custom HTTP headers. Applying a connection
that requires them to Grok returns an error instead of silently dropping them;
the connection remains usable by plugins and supported harnesses.

Applying validates the protocol and selected model, takes the normal harness and
maintenance gates, and writes through the existing native configuration backup
and rollback engine. Busy harnesses return a conflict; the saved connection stays
available. Bindings track the applied connection revision separately from its
latest saved revision. Native backup restoration is reflected in the binding list.
Each backup also retains its previous binding, including when the same connection
was reapplied with a different model or revision. Restoring it does not revert the
saved connection; applying its newer revision remains an explicit choice.

## Harness-local sign-in and native configuration

Each harness's selector also offers **Account sign-in** or **Native config** when
the device detects a usable native option. Choose it and click **Apply** to switch
back from a saved API connection. Selecting an option alone changes nothing.
An already active native option is selected automatically; applying it again is
unnecessary. The native option remains selectable even without a compatible saved
API connection. Older Supervisors that do not report native availability do not
show this option.

The native baseline belongs to one harness on one device. It is not a saved API
connection: it is excluded from the account directory, synchronization, and plugin
connection pickers. The Supervisor preserves the baseline before applying a
managed connection, so switching between several API connections does not replace
the original baseline with the previous API connection. Restoration updates only
provider-owned configuration and necessary authentication fields, keeping unrelated
settings such as hooks and MCP servers. Grok also restores the native context
window and removes generated model entries that have no remaining user fields.
OAuth credentials that stay in the
harness's own credential store are not copied into a generic connection.

Availability is a local credential/configuration check, not a remote provider
authentication test. Expired, unrefreshable credentials do not count as usable.
The apply operation rechecks availability and the selected native revision under
the usual maintenance and harness gates. Conflicting edits or a busy harness
leave the current selection unchanged and return an error.

Detection reads the harness's supported local configuration and credential files.
Claude's default macOS configuration also supports its Keychain entry, with a
bounded lookup. Codex credentials stored only in an OS keyring are not detected;
its `auth.json` credentials are supported. A configured credential helper is not
executed just to populate this list. Local checks cannot establish whether an
opaque refresh token has been revoked by the provider.

## Account directory and explicit synchronization

Devices publish only `{id, name, protocol, revision}` over their authenticated
relay tunnel. The relay persists these advertisements per device and returns only
the authenticated owner's directory, adding device names and online status. It
does not persist endpoints, models, credentials, custom headers, native fragments,
or encrypted connection payloads. Offline advertisements remain discoverable;
they are the device's last reported inventory, not proof that a copy is available.

The connection list merges replicas by ID, with each source's opaque revision.
Different revisions are shown as a conflict; they are not ordered using device
clocks and are never silently merged. Names are display metadata, not identity
or authority. A missing or older relay leaves local connections available and
reports account-catalog unavailability.

**Sync** copies a connection to the selected device. **Apply** is a separate
harness operation. A successful sync is sufficient for a plugin to select the
connection. After copying, the destination can use its local credentials while
the source is offline. Changes are pulled explicitly, not pushed automatically.

Credential synchronization requires both devices to be bound to the same account
key, to have valid device certificates, and to have device-to-device access
enabled. Browsing the directory does not enable peer access. A peer certificate
alone never authorizes exporting an upstream credential:

1. The owner's browser sends an encrypted, signed request to the source for a
   short-lived transfer authorization, scoped to the target device, connection ID,
   and source revision. The response contains a random token, not the API key.
2. The browser passes the token to the destination's synchronization endpoint.
3. The destination uses the existing certified HPKE peer transport to request the
   exact connection. The source verifies the target identity, token, expiry,
   revision, and the issuing browser's continuing owner authority.
4. The source consumes the token and returns the configuration inside the encrypted
   peer response. The destination validates it and atomically saves its local copy.
   The browser receives only the redacted connection summary.

Authorizations expire after two minutes and are single-use. The source stores
only a token hash and bounded authorization metadata. Retry after an interrupted
transfer requests a new authorization. Updating a differing local revision also
requires its `expectedRevision`; concurrent local edits are not overwritten.
Same-revision imports are idempotent. A source revision change requires refreshing
and authorizing the new version. The relay can withhold or misrepresent directory
metadata, but cannot replace certified device identities or authorize an export.

Removing a local copy withdraws that device's advertisement and leaves other
copies intact. Connections referenced by a harness or Content Quality cannot be
removed until those references are changed. Removing copies does not revoke a
provider-issued API key. Local-only operation works without a relay or account;
only cross-device discovery and synchronization require the relay.

## HTTP interface

These management routes use the existing owner access boundary. Shared users
cannot list connections, apply them, or authorize transfers. Responses use
`Cache-Control: no-store`.

| Method and path | Purpose |
| --- | --- |
| `GET /api/management/connections` | Local connections, bindings, harness-local native availability, and account discovery metadata |
| `POST /api/management/connections` | Save connection; never apply |
| `DELETE /api/management/connections/{id}` | Remove this device's unreferenced copy |
| `POST /api/management/connections/models` | Discover models using an unsaved draft |
| `POST /api/management/connections/test` | Test an unsaved draft with a small text request |
| `POST /api/management/connections/{id}/models` | Discover using saved credentials |
| `POST /api/management/connections/{id}/test` | Test saved connection, optionally overriding `model` |
| `POST /api/management/connections/{id}/apply` | Apply `{harness, model?}` explicitly |
| `POST /api/management/upstreams/native/{harness}/activate` | Restore this harness's native option; body `{revision?}` |
| `POST /api/management/connections/{id}/transfer-authorizations` | Source owner authorizes `{targetDeviceId, revision}` |
| `POST /api/management/connections/{id}/sync` | Destination imports `{sourceDeviceId, revision, token, expectedRevision?}` |
| `POST /api/peer/connections/{id}/export` | Certified encrypted peer consumes `{token, revision}` |

The connection list's optional `native` object is keyed by harness ID. Each entry
contains `available`, `active`, `kind` (`oauth` or `config`), and an opaque
`revision` when a native baseline exists. No credentials or native configuration
are returned. Clients omit the native choice when availability is false or the
field is absent, and send its revision when applying the choice.

The save body contains `name`, `protocol`, `baseUrl`, `apiKey`, optional `model`,
`authType`, and `headers`; edits also supply `id` and `revision`. Native fragments
are retained internally and cannot be injected through a new generic connection.
Model identifiers can be entered manually if discovery is unavailable. Tests use
the selected wire protocol and may incur provider usage. Errors never return raw
provider response bodies or transport errors containing credentials.
