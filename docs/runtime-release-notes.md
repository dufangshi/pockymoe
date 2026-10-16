A thread map shows every thread on the account's devices at once (More on a
phone, the drawer, the rail, the workspace list). Devices hold workspaces and
workspaces hold threads; a tile grows with the thread's turns, its fill shows how
long it ran in the last 24 hours, and running threads breathe. Big tiles add the
agent and model, the latest prompt, turns, total and recent run time, tokens and
API price (with the usual breakdown), hourly bars and an age mark; small tiles
keep only what fits. A thread's agent threads open up inside its tile. Each
Supervisor reports this at the owner-only `GET /api/thread-activity`; devices on
older Supervisors still appear, unsized and uncolored.

On phones the workspace drawer is redesigned: All workspaces, the thread map,
shortcuts that can be reordered by dragging (the order is saved with the account
and the same on every device) and recent conversations. The thread sheet lists the
current thread's group first and no longer shows notifications, which stay on the
bell. The dock's thread button no longer switches threads with a sideways swipe.

The phone composer opens to its full height when a long prompt is focused, also
while a dictation or an IME candidate is still underlined, and stays open through
its own buttons, the dock and menus until the conversation is tapped. The
one-handed dock now stays visible above the keyboard while typing.

Folded and opened turns keep their state when a thread view mounts again, and a
search result no longer reopens its turn on every refresh of a running thread.
The devices page opens with devices as the relay lists them instead of briefly
showing them offline and unverified.

Relay deployment adds the shortcut order (`PUT /relay/account/workbench/shortcuts`).
Validated with the runtime, supervisor, relay and protocol tests touched here, the
Web and shared UI unit suites and phone and desktop browser checks. Windows Device
Manager is unchanged. Installing this release is a separate device update.
