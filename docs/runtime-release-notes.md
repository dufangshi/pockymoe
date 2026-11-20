Harnesses work right after their first install, and accounts created with Google or GitHub can sign in with their password.

**Fixes**

- **First harness install.** On a machine without a suitable Node, a newly installed harness failed with "ACP stdout closed" until the Supervisor restarted. This affected every npm-based harness (Codex, Claude, Gemini, OpenCode, DeepSeek Harness and others). Harnesses, Pockymoe terminals, automation commands and plugins now find the managed tools and Node at once, so `codex` and `claude` also work in a Pockymoe terminal.
- **Clearer harness errors.** When a harness stops before answering, the thread error names its exit status and its last error lines, for example `env: node: No such file or directory`.
- **Password sign-in for Google and GitHub accounts.** The password such an account sets now also signs in, with its email or username. Accounts that set one earlier get this the next time they unlock in a browser after signing in with Google or GitHub.

**Interface**

- Deleting a thread closes its tab at once, in every window of the browser; a thread deleted on another device or by an agent leaves the tabs too.
- File paths in messages no longer blink while you type.
- A message's time, copy and fork buttons sit on a translucent chip, so the text under them stays readable.
- Tab titles stay on one line; the unsent-draft mark shows only on tabs that are not open.

Update a device from Settings → Check/Update. The Web app and the relay are already updated.
