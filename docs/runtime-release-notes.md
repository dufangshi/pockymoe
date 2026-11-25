The Git panel and the isolated environments panel are redesigned, and a thread whose environment migration failed can be opened and recovered again.

**Fixes**

- **A thread stuck after a failed migration.** When an approved move into an isolated environment failed, the thread page said "Unable to resolve this thread" and the Retry and Restore buttons could not be reached. The thread now opens; everything that runs stays blocked until its owner retries or restores this device.
- **Stop or start interrupted by closing the page.** An environment could stay in "stopping" forever when the tab was closed mid-action. The action now finishes on the device.
- **Retry after a failed migration.** Retry stops the failed environment first instead of failing again.
- **Git diff under the wrong file.** An untracked binary file made the next file's changes appear under its name. Each file's changes now show under its own name, and binary and empty files say so in words.

**Changes**

- **Environments start when you create or switch to them.** Before, the environment stayed stopped and prompts waited without a word.
- **Git panel.** One line shows where you are (branch, short commit, working-copy switch), one line switches between Unstaged, Staged, Compare and Commits, and the rest is the file list with the changes under it. Paths and details open on request.
- **Isolated environments panel.** One list shows where the thread runs and what it can switch to, with the current place first: its state, image and limits. A line of text says why something cannot be changed, for example "Needs a Dokploy connection first", with a button to Settings. An environment you left running says it still uses the device's budget and offers Stop.
- A steer sent into a running wake turn shows at once while the wake is folded, and "Show less" sits at the left with its arrow pointing up.

Update a device from Settings → Check/Update.
