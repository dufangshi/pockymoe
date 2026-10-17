Harness selectors now offer a device-local Account sign-in or Native config
option when usable authentication/configuration is detected. Switching to a saved
API connection preserves that baseline; selecting the native option and clicking
Apply restores it without replacing unrelated hooks, MCP, or other user settings.
Grok restores its native context window and removes empty generated model slots.
The option stays with its harness and device and is excluded from shared saved
connections, plugin pickers, and cross-device synchronization.

Restoration rechecks availability and revision under the existing maintenance and
harness gates. Busy harnesses and conflicting edits return an error without
changing the selected upstream. The UI keeps selection separate from application
and supports restoring a native option even without a compatible API connection.

Content Quality also saves its default context limits explicitly when the device
declares support. The defaults remain the latest 5 execution turns and 10,000
Unicode body characters; both are configurable globally or for one thread.

Validated with focused native upstream and connection regressions, component
checks, and a representative phone browser switch-and-restore flow using isolated
fixtures. Windows Device Manager is unchanged.
