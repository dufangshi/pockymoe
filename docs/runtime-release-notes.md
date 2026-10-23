Sharing grows up: its own tab, safety codes the recipient shows instead of
hunts for, leaving a share, and clear reasons when sharing blocks something.
Agents can ask for any reasoning effort, and the conversation, tabs and menus
are steadier.

- **Sharing tab**: the Devices page has Devices and Sharing tabs. Sharing lists
  what is shared with you and what you share as compact rows, viewed as a list
  (filters, search), by device or by person; a row opens in place with its
  details and actions. Shares that need you are marked and counted.
- **Safety codes**: with "Require safety code check" on (Account settings,
  default on), a new share waits until you confirm the recipient's code. The
  recipient sees their code on a waiting page and sends it to you; you type it
  in the share and approve. Existing shares keep working.
- **Leave a share**: a recipient can give up a thread, workspace or device
  shared with them, after a confirmation.
- **Clear share states**: a share the protected device has not signed now says
  the recipient cannot open it and offers the one fix; the details no longer
  blink while the page refreshes.
- **Why a shared device is blocked**: a device shared for control tells its
  agents, and you, which shares keep it from your other devices.
- **Reasoning effort for agents**: `thread create --reasoning-effort` and role
  efforts are accepted when model discovery has not yet learned a model's
  efforts; the harness applies or refuses them at session start.
- **Conversation**: a prompt sent as a turn ends starts a new turn; a steer
  shows in its turn at once; earlier turns load without twitching; tabs drag
  beside the conversation to split, and a tab on another device switches
  without blanking the page.
- **Quieter failures**: failures and unavailable reasons show as small marks.
- **iOS**: a tap in a menu never goes through to what is under it.

The relay and Web parts (Sharing tab, safety codes, leaving, the relay admin
console) are already live; this release brings the device side: the agent
message for blocked devices, the reasoning effort fix and the conversation
changes. Validated with the runtime, Supervisor and relay suites, the Web and
shared UI tests, and focused end-to-end runs against a real relay and protected
device. Installing this release is a separate device update.
