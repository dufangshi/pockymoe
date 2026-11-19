A full pass over the interface (about 140 fixes across the Web, phones and the desktop app), backend hardening, and three behaviour changes.

**Behaviour changes**

- **Claude threads start with full access.** A Claude thread with the default full-access policy now runs in Claude's `bypassPermissions` mode, not its own `auto` mode. Where Claude does not offer bypass (root without a sandbox, or disabled in its settings) the thread keeps Claude's mode and Pockymoe approves each request as before.
- **Sending empties the composer at once.** The prompt and its files show only in the transcript while they upload and send, and the send key spins. A failed or cancelled send puts them back in the composer.
- **"Thread" everywhere.** The interface uses one name for a Pockymoe thread; "session" is kept for a harness's own session and for sign-in.

**Interface**

- **Transcript**:
  - A search result scrolls to the matching message and stays there.
  - Running dots use the theme colour and say how long nothing has arrived, instead of turning red.
  - Long commands can be read in full.
  - Work-line counts are labelled and can be tapped on touch screens.
  - Durations and token counts use one format: "39 分 24 秒", K/M/B.
  - A failed turn explains itself and offers Retry.
  - Earlier turns load ten at a time.
  - Sub-thread wake reports are one line per sub-thread until opened, at body text size, and the agent's reply after a wake shows its first lines outside the fold.
- **Composer**:
  - Menus work from the keyboard and no longer cover the line being typed.
  - Goal and Fast modes stay visible.
  - Unsent drafts survive a reload.
  - Stop and Send are 9 px apart.
  - Upload progress starts at once.
  - The new-thread form takes a first prompt.
  - Send is disabled, with the reason, while the device is unreachable.
- **Navigation**:
  - Threads never opened in this browser are no longer marked unread.
  - A lost connection is shown in words, not as a running ring.
  - Device readings are labelled and sit at the foot of the sidebar.
  - The thread map draws at once.
  - Tabs have close actions.
  - Thread menus are real menus.
  - Chinese text uses Chinese fonts in every theme.
- **Settings**:
  - Phone section tabs show that more follow.
  - The context window keeps its height between tabs.
  - Plum Pocket shows setting descriptions again.
  - Dialog buttons follow one order.
  - Long names and URLs truncate in the middle.
- **Files, terminal and Git**:
  - Ending a terminal with a running command asks first.
  - Draw.io diagrams fit their pane when opened, without the false "update device" warning.
  - Filtering finds files in collapsed folders.
  - The explorer no longer opens a file on its own.
  - Images in a Markdown preview open in place and page through the document.
  - Untracked files and single commits show their changes.
- **Phones**:
  - Settings, notifications and other windows rise from the bottom, clear of the notch and home bar.
  - Landscape keeps controls out of the side notch.
  - Touch targets are at least 40–44 px.
  - Terminal keys have no dead gaps.
  - Left-hand mode moves row actions.

**Backend**

- Shares and grants save their verification state atomically, and Relay and Supervisor validate scopes the same way.
- Loopback requests bypass environment proxies.
- Command failures are recorded; helper pipes and ACP writes have deadlines.
- Interrupted updates and Treer handshakes recover.
- Exports and public links load only the history they show.
- Search, idle automation checks and terminal output are cheaper.
- Database migration 25.

Update a device from Settings → Check/Update. Desktop and phone app changes ship separately in the client release.
