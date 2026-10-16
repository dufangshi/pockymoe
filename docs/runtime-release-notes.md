A Pockymoe Agent launched in Treer on a machine without the selected agent CLI
(for example `codex` or `claude`) no longer exits with only Treer's
`process_not_running`. It stays running, says which base agent is missing and how
to install it, and retries every 30 seconds, connecting on its own once the CLI
is available. The same applies when the provider is not enabled on the hub. The
hub now recognizes these causes even when a runtime layer adds context to the
error.

This release also opens plugin settings from the gear in the plugin list, makes
automation reports more compact, and adds a chronological public text summary
API for threads.

Validated with the Supervisor and CLI suites, including a new attach test for a
missing base agent. Windows Device Manager is unchanged.
