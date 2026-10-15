Setup now upgrades a database left by an older Remote Codex (Node) installation
instead of stopping with "unsupported `threads` schema: required column
`agent_id` is missing". The database is first copied to
`relay-supervisor.pre-rust-0.12.sqlite`; the missing Node migrations and the
runtime migrations then run in transactions. A database that cannot be upgraded
is never deleted: setup asks on the terminal whether to move it aside as
`relay-supervisor.unusable-<time>.sqlite` and continue with an empty database.
Without a terminal it asks to rerun the command with `--reset-database`
(`-ResetDatabase` in PowerShell). Offline `pockymoe bind` upgrades such a
database the same way.

The SH bootstrap no longer stops on macOS with "setup_version?: unbound
variable": macOS `/bin/sh` (bash 3.2) read the ellipsis after a variable as part
of its name. The script is ASCII now, and a test keeps it that way. The Web
origin and the relay serve the fixed script independently of this release.

This release also includes the relay port-preview and Plum phone top-bar changes
already on main. Database migration, setup and bootstrap script regressions were
verified, including a pre-0028 Node database through the real `pockymoe bind`
path and the terminal prompt. Windows Device Manager is unchanged. Installing
this release is a separate device update; publication does not restart an
existing Supervisor.
