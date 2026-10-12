# Device setup and upstream management

On the relay Devices page, create a device and copy its setup command: SH for
macOS and Linux, PowerShell for Windows. The command downloads `/setup.sh` or
`/setup.ps1` from the Web origin (the relay itself unless the Web UI is
[hosted separately](self-host-relay.md#separate-web-origin)) and carries the
relay URL, the device's permanent token, the Web origin and the values that bind
the device to your account key (see below). It
installs the native runtime from GitHub Releases, verifies its checksum and
version, and starts the device; see
[native installation and releases](github-runtime.md) for the details,
migration of older npm installations and updates. Existing online Supervisors
update through their management API. No shell profile is changed. The existing
database, credentials and transport identity are preserved, and a different
device configuration is never overwritten.

Supported platforms are Apple Silicon macOS, glibc Linux on ARM64/x64 and
Windows x64. macOS uses a user LaunchAgent; Linux uses a systemd user service
when available. These start with the user session. Linux machines requiring
startup before login need user lingering configured by their administrator.
Without a user service manager (including some containers), setup starts a
detached process and reports that reboot startup is unavailable.

Linux units use `WorkingDirectory=%h`: systemd resolves the service user's home.
Unlike `ExecStart` arguments and `Environment` assignments, this directive must
not contain surrounding quotes. If an older setup left a failed unit with a
quoted home path, running a **fixed launcher** with the same relay/token/port
while the device is offline regenerates the unit, reloads systemd and starts it,
preserving the saved configuration and history. An already-online device is
left running; setup does not rewrite or restart its service just for this repair.

The parser regression can be run in an isolated Linux environment with Node and
systemd installed: `POCKYMOE_TEST_SYSTEMD=1 node --test scripts/setup.test.mjs`.
It verifies generated units without starting a Supervisor or contacting a relay.

## Binding to your account key

A bound device accepts only requests signed by browsers of your account, so the
relay cannot command it ([account key trust](account-key-trust.md)). Copying a
setup command needs your password if this browser has not unlocked the account key
in the last 10 minutes, and a sign-in or verification within the last 10 minutes
for the device token. The command passes `--account-key` (`-AccountKey` in
PowerShell) and a one-time enrollment secret in the `POCKYMOE_ENROLL_SECRET`
environment variable, never as an argument; setup binds the device before
starting it, and the Supervisor does not inherit the secret. The Web refuses to
build a command from a malformed device token. When the device first connects,
the browser that copied the command finishes enrollment by itself while it stays
open; the device then shows **Protected**. That browser keeps the enrollment for 7
days; after that, copy the setup command again and rerun it on the device. On an
installed device, rerunning a setup command binds the existing installation and
keeps its registration, provided the Web origin saved at installation matches
the command's.

Devices installed without the binding flags show **Not protected · Relay
trusted**. **Protect all devices** on the Devices page binds them from the browser
in one step and offers **Update and protect** for older runtimes; it trusts the
relay at the moment of binding ([one-click protection](account-key-trust.md#protect-all-devices)).
**Protect this device** offers the same for one device, or **Show local bind
command**, the most secure option, a command to run on the device:

```sh
POCKYMOE_ENROLL_SECRET='<secret>' pockymoe bind --account-key '<key>' --web-origin 'https://app.example.com'
```

The PowerShell form sets `$env:POCKYMOE_ENROLL_SECRET` and removes it after the
command. `pockymoe bind --enroll-stdin` reads the secret from standard input
instead.

It writes the Supervisor database directly, needs no network access and works
whether or not the Supervisor is running. `pockymoe bind --status` prints the
device identity fingerprint and the bound account fingerprint, and `pockymoe bind --reset` (`--yes` skips the prompt)
removes the binding and every browser registration, grant and browser-created
port mapping, restoring relay trust; removed browsers stay removed if the device
is bound to the same key again. Use it before binding to a new account key after
**Start over**, or before attaching the device to a different relay device; until
then a bound device that the relay greets with a different device ID retries with
backoff and reports `device_id_mismatch` in `/healthz` and `pockymoe bind
--status`. **Remove device** in the Web deletes the device's relay registration
but leaves its local binding in place.

A device whose identity file or certificate was lost shows **Certificate
missing**. **Repair device binding** gives a new bind command; after it runs, the
browser completes enrollment and, if the device's identity changed, asks you to
confirm the reinstalled device before trusting it.

## Settings and upstreams

Open a device's Settings to manage its Supervisor, harnesses and upstreams.
Only the device owner can change these settings. Install/Update jobs keep running
when the page closes; their progress is visible when reopening Settings. The
managed install catalog includes Codex, Claude Code, Gemini CLI, Grok Build,
Cursor Agent, GitHub Copilot and OpenCode. Codex and Claude also install the ACP
adapter. Updates preserve the detected installation owner. DeepSeek/custom
executables still need an externally managed installation.

Upstream profiles support OpenAI Codex Responses, Claude Agent Messages, Gemini
GenerateContent, Grok Responses/Chat Completions, and DSH DeepSeek official /
OpenAI-compatible APIs. Profiles, keys and exact
configuration backups are stored privately on the device. The browser receives
a `hasApiKey` flag and redacted provider fragments. Activation updates provider-owned fields while preserving
MCP, skills and other settings. It retires idle ACP processes; the next turn
reloads the configuration and resumes the session. A busy harness returns a
conflict; finish or stop its current tasks before switching. Restore recovers
the previous files. CLI configuration follows the Supervisor's OS user and
native config-home overrides; two Supervisors sharing those directories share
the live CLI configuration.

Model discovery preserves per-model reasoning capabilities from upstream
`reasoningEfforts` / `reasoning_efforts` or `capabilities.reasoning_effort`
metadata. Unknown effort names are ignored; defaults must belong to the advertised
options. An explicit unsupported/empty declaration takes precedence over ACP;
only missing metadata is supplemented by a fresh, bounded-cache ACP probe.
No common effort list is assigned to every model.

For Grok, discovered options are written to both the wire model ID and its legacy
managed alias. Refreshing the directory invalidates discovery/probe caches;
changed catalogs reload an idle session before its next settings change or prompt.
Harness Check/Update/restart also invalidates caches. Active turns are not killed
by directory refresh. Explicit effort changes use ACP `session/set_config_option`
with a string value and verify the returned setting; Auto selects the model's
advertised default. Original local model settings are retained for restoration.

Connection tests make a small authenticated model request and may incur charges.
Claude profiles support both API-key and bearer-token authentication; imported
profiles preserve that choice and connection tests use the same header as the CLI.
They validate the API response shape, without returning upstream response bodies.
Import accepts native provider TOML/JSON or a CC Switch provider's `settingsConfig`
object, not its database or arbitrary commands. OAuth-only providers still require
their vendor login flow; an API key template does not replace OAuth authorization.

## Templates

Import first shows a preview. Apply installs missing harnesses/adapters, then
activates the profiles. A failed step stops the job and reports completed steps;
successful installations are retained and configuration backups remain available.
Jobs run within the Supervisor, so do not restart it during a template apply.
Exports omit API keys; fill these before importing on a new device.

```json
{
  "schemaVersion": 1,
  "harnesses": ["codex", "claude"],
  "profiles": [
    {
      "name": "Team Codex",
      "harness": "codex",
      "baseUrl": "https://api.example.com/v1",
      "apiKey": "REPLACE_WITH_DEVICE_KEY",
      "apiType": "responses"
    }
  ]
}
```

The model field is optional. When omitted, applying the template probes the
upstream model catalog and selects the first generative model returned. The
schema accepts one active profile per harness and fixed catalog IDs only.
It never installs imported shell commands, arbitrary package names or URLs.

## Isolated acceptance

`scripts/device-setup-live.mjs` runs on an isolated Linux machine with a supplied
candidate native binary. It creates its own HOME, relay, registry fixture and
database, fetches official Node and harness packages, exercises device-scoped
management, and stops only the processes it created. Evidence remains in its
printed temporary directory. Restart/configuration ACP tests use deterministic
fixtures; no host credentials or model quota are required.

`scripts/upstream-models-live.mjs <candidate-runtime> <grok-binary>` is an opt-in
isolated Linux regression with real Grok and a synthetic upstream. It checks
new-model capabilities, actual request effort values, Auto, directory refresh,
session restoration, upstream switching and exact native-config restoration.
Run with `TEST_GROK_API_TYPE=chat_completions` as well as the default Responses
mode. It sends requests only to its loopback fixture, not a paid model provider.
