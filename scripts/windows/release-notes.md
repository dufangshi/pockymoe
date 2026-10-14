The Windows Device Manager now follows Pockymoe's native GitHub release installation and maintenance model.

- Rebuilt the desktop interface with Classic and Plum, each in light and dark mode. Connection details are collapsible; common actions stay visible. The product icon matches the Web app.
- Supports current PowerShell setup commands, account enrollment, one-use setup codes and legacy REMOTE_CODEX configuration. Pasted commands are parsed locally, never executed as shell code.
- Downloads a versioned Rust runtime from GitHub, verifies its SHA-256 and actual version, and lets native setup install the matching Web bundle. No Pockymoe npm package, Node or Agent is installed by the Manager.
- Requires native runtime 0.13.3 or newer, which fixes Windows PowerShell 5.1 background startup and maintenance stop. For an older native runtime, Disconnect then Connect performs a one-time migration with the saved registration.
- Uses authenticated local Supervisor Check/Update APIs. The Supervisor owns update restart and task recovery. Tray reconnect resolves native/current.json on each launch and waits during maintenance.
- Preserves the existing Manager install location, startup entry, device configuration, database and encryption identity. A running legacy npm device must be disconnected before the Manager migrates it; paste the current Windows setup command from the Devices page. Agents are managed in Web Settings.

Validation includes Windows builds, isolated parser/auth/checksum tests, real native setup with a local Relay and account binding, maintenance restart, tray reconnect and identity preservation, plus screenshots of four themes at 100%, 150% and 200% scale.

This independently versioned, self-contained Windows x64 executable is unsigned. Verify the accompanying SHA-256 checksum. This release does not change the runtime latest alias or update other devices.
