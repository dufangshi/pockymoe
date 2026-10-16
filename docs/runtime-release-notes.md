The conversation view is redesigned as a compact log. A finished turn folds into
one work line ("Worked for 2m · ✓12 ✕1 · edited 3 files +40 −8") with its answer
below; opening it shows the narration with each run of operations folded, and a
run lists its commands, edits and tool calls. Command output loads and opens only
when clicked, outputs and diffs open in a floating viewer, and toggles stay under
the pointer while what they open changes size. The Supervisor now sends each
summarized turn's operation counts, so folded turns show them without loading
their items.

On phones the controls move to the bottom for one-handed use (Settings →
Preferences, on by default): the top bar shows only device load, usage windows,
running threads and notifications; the composer and a row with the thread, Chat,
Files, Terminal and More form one bottom panel. Swipe up on it for the thread
list, drag a sheet down to close it, swipe right from the left part of the page
for the workspace drawer, and choose a left-handed layout that mirrors the panel.
A split pins its two conversations in that row.

The terminal copies and pastes like a desktop terminal: Ctrl+C copies a selection
and interrupts without one, Ctrl+V pastes at a prompt and stays a key in vim or
less, selecting copies, right-click opens a menu, and a multi-line paste that a
shell would run line by line asks first (Settings → Terminal). On phones a long
press selects text in the terminal and the key bar has Paste and Select.

Sharing a protected device now reviews the grant in plain words (recipient,
scope, access, expiry, safety code, changed fields) with the signed content
folded below. Moving between threads and pages no longer flashes: recently
opened threads appear at once, Home stays in the app, and reloads paint the
chosen theme from the first frame.

Relay devices, their services and databases are unchanged. Windows Device Manager
is unchanged. Installing this release is a separate device update; publication
does not restart an existing Supervisor.
