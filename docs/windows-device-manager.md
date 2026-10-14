# Windows Device Manager

The independently versioned Windows x64 tray app lives in
`apps/windows-device-manager`. Download its self-contained EXE and checksum from
the `windows-device-manager-vVERSION` GitHub release. Manager releases use
`--latest=false`; `/releases/latest` always remains the runtime release.

## Install and connect

Open the EXE, copy a Windows setup command from the public Web Devices page,
and paste it in the Manager. The app parses the command; it never evaluates
pasted PowerShell. Both current account-bound commands (including enrollment
secrets and setup codes) and legacy environment assignments are accepted.
Secrets remain masked and are not written to Manager settings or logs.

The Manager downloads the latest versioned native Rust runtime from GitHub,
verifies its checksum and reported version, and calls native setup to install
the matching Web bundle and establish account trust. It does not install Node,
the retired Pockymoe npm package, or a specific Agent. Install/configure Agents
in Web Settings. Classic and Plum themes each have light/dark variants; the
selection is saved per Windows user.

Closing the window hides it to the tray. Windows sign-in startup is optional.
Disconnect or the tray's offline/exit action stops this device. Check/Update
requires a running native device and uses its authenticated loopback management
API; the Supervisor drains/restarts and recovers tasks itself. The Manager does
not roll back a migrated database. It waits during maintenance instead of
starting a competing Supervisor. Reconnect reads `native/current.json` each
time, so it follows updates started in Web Settings too.

## Existing installations

The executable/assembly name, mutex/pipe, current-user startup registry value,
`%LOCALAPPDATA%\RemoteCodex` and `~/.remote-codex` paths remain compatible.
A Manager upgrade does not delete device configuration, SQLite history, Agent
credentials or transport identity. Config readers prefer `POCKYMOE_` keys and
accept `REMOTE_CODEX_` keys.

Native devices installed with PowerShell can be discovered by the Manager.
Windows runtimes older than 0.13.3 require a one-time offline migration because
their PowerShell 5.1 launchers stop on ordinary stderr and cannot reliably stop a
headless Supervisor during maintenance. Disconnect, then Connect; the Manager
reuses the saved registration and installs the current native release. It never
automatically restarts an affected older binary or stops a running old device.
For a retired npm device, explicitly disconnect the old Supervisor before
migrating and paste its current Windows setup command. The old device token,
Relay, local port and absolute database path must match the saved registration;
native setup refuses to overwrite a different registration. Running legacy
installations are not silently stopped or reinstalled during Manager startup.

## Validation and release

`dotnet build apps/windows-device-manager/RemoteCodex.DeviceManager.csproj`
can cross-compile on Linux. Windows validation is
`scripts/windows/build-device-manager.ps1`: self-contained publish, isolated
parser/checksum/auth/state tests, plus rendered screenshots for the four themes
at 100%, 150%, and 200% scale. `--native-self-test PATH_TO_VERIFIED_RUNTIME`
adds a real, isolated local Relay, account-bound native setup, authenticated
Check, maintenance restart, reconnect following current.json, and preservation
of legacy configuration and device identity. Tests require
`POCKYMOE_DEVICE_MANAGER_TEST_ROOT`; never point it at a real user profile.

The dedicated `windows-manager-release.yml` runs these gates and uploads the EXE,
checksum and screenshots. After reviewing artifacts, dispatch it on main with
`publish=true`. It creates an immutable Manager tag/release, without runtime or
npm publishing. The EXE is currently unsigned; users can verify its SHA-256.
