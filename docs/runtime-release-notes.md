Local mode now installs with one command and no relay:

```sh
curl -fsSL https://app.lnz-study.com/setup.sh | sh
```

(Windows: `irm https://app.lnz-study.com/setup.ps1 | iex`.) Run without relay
arguments, setup installs the latest release, picks port 8787 or the next free
one, asks once whether phones and other computers on the network may connect,
and starts Pockymoe as its own user service (`pockymoe-local.service` /
`com.pockymoe.local`) next to any relay device. `--port`, `--lan`, `--no-lan`
and `--reset-database` skip the defaults and the question. See
[local mode](https://github.com/dufangshi/pockymoe/blob/main/docs/local-mode.md).

Local mode starts open. **Settings → Device → Access passcode** turns on a
passcode that other browsers enter once; changing it signs out every other
browser, and `pockymoe passcode off` on the computer removes it. Failed attempts
are rate-limited.

Installed releases now serve their own Web UI and keep their data in
`~/.remote-codex` for `pockymoe start` without further settings, and the CLI
finds a local Supervisor by itself. Server mode with a fixed administrator
password keeps working for existing deployments; new ones should use a relay or
local mode.

This release also includes the built-in content quality plugin already on main.
Relay devices, their services and databases are unchanged. Windows Device
Manager is unchanged. Installing this release is a separate device update;
publication does not restart an existing Supervisor.
