# Local mode: Pockymoe on one computer, without a relay

Local mode runs Pockymoe on your computer and serves its Web UI there. There is
no account and no relay: open the address in a browser on the same computer, or
on a phone or computer in the same network.

## Install

macOS and Linux:

```sh
curl -fsSL https://app.lnz-study.com/setup.sh | sh
```

Windows (PowerShell):

```powershell
irm https://app.lnz-study.com/setup.ps1 | iex
```

Then open the address it prints, normally <http://127.0.0.1:8787>. Run in a
terminal, setup asks one question: whether phones and other computers on your
network may open Pockymoe. Everything else uses defaults.

Setup installs the native runtime from GitHub Releases (checksum and version
verified) and starts it as a user service that starts again at login: systemd
`pockymoe-local.service` on Linux, launchd `com.pockymoe.local` on macOS. Without
a user service manager (some containers, Windows) it runs detached until the
next reboot. It keeps:

| What | Where |
| --- | --- |
| Settings of this install | `~/.remote-codex/local-supervisor.json` |
| Conversations and settings data | `~/.remote-codex/supervisor.sqlite` |
| Log | `~/.remote-codex/logs/local-supervisor.log` |

Running the same command again repairs the install or updates it to the latest
release. A database left by an older Remote Codex installation is upgraded the
same way as for relay devices ([device setup](device-setup.md)).

### Options

Add them after `sh -s --` (PowerShell: run `& ([scriptblock]::Create((irm https://app.lnz-study.com/setup.ps1))) -Lan`):

| SH | PowerShell | Effect |
| --- | --- | --- |
| `--port 9000` | `-Port 9000` | Use this port. By default 8787, or the next free port. |
| `--lan` | `-Lan` | Let devices on your network open Pockymoe, without asking. |
| `--no-lan` | `-NoLan` | Only this computer, without asking. |
| `--reset-database` | `-ResetDatabase` | Move an old database that cannot be upgraded aside instead of asking. |

```sh
curl -fsSL https://app.lnz-study.com/setup.sh | sh -s -- --lan --port 9000
```

## Phones and other computers

With network access, setup prints the address to use, such as
`http://192.168.1.20:8787`. Anyone who can reach that address can use Pockymoe
until you set a passcode:

1. Open **Settings → Device → Access passcode**.
2. Enter a passcode (at least 4 characters) and choose **Turn on**.

Other browsers then sign in once with the passcode; the browser that set it stays
signed in. Changing the passcode signs out every other browser. Forgot it? On
this computer run:

```sh
pockymoe passcode off
```

The passcode keeps others on the same network from walking in. On a home network
the connection is plain HTTP; to use Pockymoe from outside your network, use a
[relay](../README.md#quick-start), which also encrypts everything end to end.

## Updates

**Settings → Device → Check for updates** installs the latest release and restarts
Pockymoe, like on relay devices. Rerunning the setup command does the same.

## Together with a relay device

A computer can have both: a relay device (`remote-codex-supervisor` service,
`relay-supervisor.sqlite`, port 45679 by default) and a local install. They use
separate services, ports and databases, so their conversations are separate too.

## What needs a relay

Local mode has everything that runs on one computer: conversations with every
installed agent, terminals, files, automations, inbox and tasks, and Settings
(including updates and harness installation). These need a relay:

- accounts, sign-in with Google or GitHub, and the account key that protects devices;
- several devices in one place, and agents talking to threads on other devices;
- sharing threads or devices with other people, and public links;
- port previews, push notifications, and recent threads synced across devices;
- end-to-end encryption between the browser and the device.

## Running it by hand

`pockymoe start` runs a Supervisor in the foreground. An installed release serves
its own Web UI and keeps its data in `~/.remote-codex` without further settings.
`POCKYMOE_PORT` and `POCKYMOE_HOST` choose the address (`HOST=0.0.0.0` for the
network). The older `POCKYMOE_MODE=server` with a fixed administrator password
still works for existing deployments; for new ones use a relay or local mode with
a passcode ([connectivity modes](auth-and-connectivity-modes.md)).
