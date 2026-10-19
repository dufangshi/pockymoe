Disk cleanup gets finer and stays responsive; the desktop's tabs are the
conversations you opened, under one bar; notifications follow a scope you choose
and the turns you subscribe to; plugins have a default for conversations.

- **Disk cleanup in the background**: cleaning up ran inside the request and
  froze Pockymoe for a minute on a large database. It is now a job with a
  progress bar that follows it after a reload, rewriting history in steps of a
  few milliseconds; on a copy of a real database the slowest of thousands of
  concurrent requests waited 63 ms.
- **What to clean up, under one age**: Settings → Resources → Disk cleans up
  command output, command text (kept, shortened to its first 512 characters, or
  removed so only "a command ran" stays) and file-change diffs (the files and
  line counts stay), all after the same number of days. Every scanned
  conversation has a menu (⋯, right click or long press) to clean up its output,
  diffs or commands now, with progress on its row. The scan reports diffs.
- **Commands stored once**: a command's detail repeated it as its input and
  preview. New commands keep it only as their text, and three minutes after the
  update a one-time pass removes the copies from history; on the measured
  database that was 50 MB, 37% of command bytes, in 8 seconds.
- **Desktop tabs and one bar**: the tabs are the conversations you opened, like
  an editor's files (✕ or a middle click closes one), kept in this browser and
  ordered by dragging. The tabs row is the only bar: search at its left
  (Ctrl/⌘+F), the thread tools menu with the thread's panels, notifications at
  the right; sidebar, Back and Forward moved to the rail. The sidebar's width
  follows a drag. A workspace's conversations keep a dragged order in the
  sidebar and on phones (hold 0.7 s on touch).
- **Notification scope**: Settings → Notifications chooses subscribed turns
  (the default: tap the bell on a working turn), all turns, or selected devices
  and workspaces, for Web Push, native push and the notification list alike.
  Accounts that were notified of every turn now need to subscribe or choose
  All. Interrupted turns send their own notice.
- **Plugin defaults**: each plugin has a default for conversations that follow
  the device; turning it off stops it there.
- **Reasoning effort on phones** shows under the model again, in the slider's
  colour.
- **Confirmations inside Settings** take focus on Cancel, and Esc closes only
  the confirmation, not Settings.

Validated with the runtime, supervisor, protocol and relay Rust tests, the Web
and shared UI tests, the storage jobs measured on a copy of a real database
under concurrent load, browser checks of the Disk settings and cleanup menu on
desktop and phone, and focused browser suites for tabs and notifications. The
relay was deployed before this release, as interrupted notices need it. Windows
Device Manager is unchanged. Installing this release is a separate device
update; publication does not restart an existing Supervisor.
