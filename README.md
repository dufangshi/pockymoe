<p align="center">
  <a href="README.md"><img alt="English" src="https://img.shields.io/badge/English-current-7b3f8f?style=for-the-badge"></a>
  <a href="README.zh-CN.md"><img alt="简体中文" src="https://img.shields.io/badge/%E7%AE%80%E4%BD%93%E4%B8%AD%E6%96%87-switch-555?style=for-the-badge"></a>
</p>

<p align="center">
  <img src="docs/assets/readme/hero.jpg" alt="Pockymoe: three little agents in a pocket, connected to a laptop, a desktop and a cloud server" width="100%">
</p>

<h1 align="center">
  <img src="apps/supervisor-web/public/icon-192.png" alt="" width="44" align="center">
  Pockymoe
</h1>

<p align="center"><b>Your coding agents, in your pocket.</b></p>

<p align="center">
  Run Codex, Claude Code, Gemini, Grok and more on your own computers,<br>
  then keep them working from your phone or any browser. Private, end-to-end encrypted, open source.
</p>

<p align="center">
  <a href="https://github.com/dufangshi/pockymoe/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/dufangshi/pockymoe?color=7b3f8f"></a>
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-e0a526"></a>
  <img alt="macOS, Linux, Windows" src="https://img.shields.io/badge/runs%20on-macOS%20%7C%20Linux%20%7C%20Windows-555">
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#what-you-can-do">Features</a> ·
  <a href="#works-with-the-agents-you-already-use">Agents</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#faq">FAQ</a>
</p>

<p align="center">
  <img src="docs/assets/readme/en/thread.png" alt="A Pockymoe conversation: the agent read files, ran commands, changed two files and summarized the result" width="92%">
</p>

## Why Pockymoe

Coding agents are great at long tasks, but they live in a terminal on one computer.
Walk away from the desk and you lose sight of them.

Pockymoe gives every agent on every one of your machines a home you can reach from
anywhere:

- **Start a task on your workstation, check on it from the couch.** Conversations,
  terminals and files follow you to your phone.
- **Keep your own setup.** Agents run on your computers with your files, tools and
  logins. Nothing is uploaded to run them.
- **Nobody else can read it.** Your browser and your computer talk end to end
  encrypted. The relay in the middle only passes the envelopes along.

## Quick start

Three steps, no command-line knowledge needed beyond pasting one line.

### 1. Open a relay

A relay is the website you sign in to. It connects your browser to your computers.
Use one that a friend, your team or a community already runs, or
[host your own](docs/self-host-relay.md) on any small Linux server.

### 2. Sign in

Open the relay's address, then **Continue with Google** or create an account with
an email and password.

<p align="center"><img src="docs/assets/readme/en/step-1-sign-in.png" alt="The sign-in page with Continue with Google" width="360"></p>

### 3. Add your computer

Go to **Devices → Add device**, give it a name, and copy the command it shows.
Paste it into a terminal on the computer you want to use (macOS, Linux or Windows).

<p align="center"><img src="docs/assets/readme/en/step-2-add-device.png" alt="A new device with its one-line setup command" width="92%"></p>

The command installs Pockymoe as a background service and connects it to the relay.
A moment later the device shows **Online**. Click **Connect**, choose a project
folder and start a conversation with any installed agent. That's it.

> [!TIP]
> Missing an agent? Install or update Codex, Claude Code, Gemini CLI, Grok Build,
> Cursor, Copilot and OpenCode from **Settings → Harnesses**, right in the browser.

### Only this computer?

Skip the relay: run one line, then open <http://127.0.0.1:8787>.

```sh
curl -fsSL https://github.com/dufangshi/pockymoe/releases/latest/download/setup.sh | sh
```

Windows, phones on your network, a passcode and other options:
[Local mode](docs/local-mode.md).

## What you can do

### A real workbench, not just a chat

Put two conversations side by side, open a terminal under them, browse and edit
files, and search across all your conversations.

<p align="center"><img src="docs/assets/readme/en/workbench.png" alt="Two conversations side by side with a terminal docked below" width="92%"></p>

### Made for your phone

<table>
  <tr>
    <td width="38%"><img src="docs/assets/readme/en/mobile.png" alt="Pockymoe on a phone in dark mode"></td>
    <td>
      <p>Every screen is designed for touch, not squeezed from the desktop.</p>
      <ul>
        <li>Follow a long task as it happens, step by step.</li>
        <li>Reply, queue a follow-up, or steer an agent mid-task.</li>
        <li>Open a terminal with a touch keyboard bar.</li>
        <li>Get a notification when an agent finishes.</li>
        <li>Add it to your home screen like an app. Light and dark themes, English and Chinese.</li>
      </ul>
    </td>
  </tr>
</table>

### Private by design

<p align="center"><img src="docs/assets/readme/en/encrypted.png" alt="The device list showing an encrypted connection and its fingerprint" width="92%"></p>

- Content is encrypted between your browser and each computer. The relay sees only
  routing information, and you can compare the device fingerprint yourself.
- Your computer connects *out* to the relay, so there is no port forwarding, no
  public IP and no VPN to set up.
- Accounts support two-step verification, passkeys and trusted browsers.

### Built for long tasks

- **Nothing gets lost.** Close the browser, lose signal, switch devices: the agent
  keeps working and the conversation is waiting when you come back.
- **Updates don't interrupt work.** Pockymoe updates itself from Settings, then
  resumes exactly the tasks that were running.
- **Agents work as a team.** An agent can hand work to helper threads, message other
  threads, share a task board and even reach agents on your other computers.
- **Automations.** Run a prompt or a script when something happens, on a schedule
  or when a task completes.

### Everything in one place

- All your computers, projects and conversations in one list.
- Pick up sessions you started in the terminal.
- See token usage and an estimated cost for every turn.
- Switch an agent between API providers from **Settings → Upstreams**.
- Share a conversation or a whole device with another account.

## Works with the agents you already use

| Agent | Install from Settings |
| --- | :---: |
| OpenAI Codex | ✓ |
| Claude Code | ✓ |
| Gemini CLI | ✓ |
| Grok Build | ✓ |
| Cursor Agent | ✓ |
| GitHub Copilot CLI | ✓ |
| OpenCode | ✓ |
| DeepSeek Harness | |
| Any agent that speaks [ACP](https://agentclientprotocol.com) | |

Each agent uses its own login or your own API keys. Pockymoe doesn't sell or proxy
model access.

## How it works

```mermaid
flowchart LR
    phone["📱 Phone / browser"] <-- "encrypted" --> relay["🌐 Relay<br/>(sign-in, routing)"]
    relay <-- "encrypted, outbound" --> mac["💻 Laptop<br/>Claude Code, Codex"]
    relay <-- "encrypted, outbound" --> tower["🖥️ Workstation<br/>Codex, Gemini"]
    relay <-- "encrypted, outbound" --> cloud["☁️ Cloud server<br/>Grok, OpenCode"]
```

- **Device.** A small background service on each computer runs the agents, keeps
  the history and serves the files and terminals.
- **Relay.** A website that handles accounts and connects browsers to devices. It
  stores no conversations.
- **You.** Any modern browser: phone, tablet or another computer.

## FAQ

<details>
<summary><b>Is my code uploaded anywhere?</b></summary>

No. Agents run on your computers. Conversations, files and terminal output are
encrypted between your browser and the device; the relay can't read them. The
agents themselves still talk to their model providers as they normally do.
</details>

<details>
<summary><b>Does my computer need to stay on?</b></summary>

Yes, an agent runs where its device runs. Pockymoe starts with your user session,
reconnects by itself, and resumes interrupted work after restarts and updates.
</details>

<details>
<summary><b>Which systems are supported?</b></summary>

Devices: macOS on Apple Silicon, Linux x64 and ARM64 (glibc 2.28+), and Windows x64.
The Web UI works in any modern browser.
</details>

<details>
<summary><b>What does it cost?</b></summary>

Pockymoe is free and open source under the MIT license. You pay only for what your
agents use with their providers, and for a server if you host your own relay.
</details>

<details>
<summary><b>Can I run my own relay?</b></summary>

Yes, it's one executable plus a reverse proxy for HTTPS. See
[Self-hosting a relay](docs/self-host-relay.md).
</details>

## For developers and AI agents

The details live in `docs/`. If you are an agent setting up or changing Pockymoe,
start with these:

- [Self-hosting a relay](docs/self-host-relay.md): download, configure, HTTPS, Docker, backups
- [Native installation and releases](docs/github-runtime.md): what the setup command does, updates, npm migration
- [Device setup, harnesses and upstreams](docs/device-setup.md)
- [Development](docs/development.md): repository layout, running locally, tests, the update API
- [Architecture](docs/architecture.md) and [AGENTS.md](AGENTS.md): project rules for contributors and coding agents

Pockymoe was formerly called Remote Codex; existing installations keep updating.
See [the rename notes](docs/rename-pockymoe.md).

## License

[MIT](LICENSE). Third-party notices are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
