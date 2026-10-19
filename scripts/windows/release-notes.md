Pockymoe Windows Device Manager 0.2.1 fixes connections stuck at “Installing the verified runtime and Web UI…”.

- Connect starts the hidden Supervisor launcher directly and detaches it from the caller's Windows Job, avoiding WMI/CIM process creation.
- First-time setup detects an unconsumed launcher after five seconds and starts it after validating the activated runtime, device configuration and log paths. Registration is not repeated.
- Manager-initiated restart/update can also recover an unconsumed native worker after verifying its plan and executable checksum. The native worker continues to own task draining and restart verification.
- Recovery handles native restart payloads with System.Text.Json instead of Windows PowerShell's legacy JSON parser.
- Setup reports connection progress and has a three-minute deadline instead of waiting ten minutes at the installation stage.
- Device configuration, database history, account binding and transport identity remain in place. Runtime downloads retain GitHub release checksum and version verification.

Validation includes Windows builds, isolated parser/auth/checksum tests, real native setup with a local Relay and account binding, maintenance restart, tray reconnect and identity preservation, plus screenshots of four themes at 100%, 150% and 200% scale.

This independently versioned, self-contained Windows x64 executable is unsigned. Verify the accompanying SHA-256 checksum. This release does not change the runtime latest alias or update other devices.
