Sign in to Claude, Codex and the other harnesses from the Web, without SSH to the device.

**New**

- **Harness sign-in.** Settings → Harnesses has a Sign in button for each harness that has a login command. It opens a terminal on the device running that command (for example `claude auth login` or `codex login --device-auth`), with the same configuration directory the harness uses, so you finish the sign-in in the browser. When the command ends, Pockymoe checks again and shows whether the harness is signed in. Only the device owner can open it, and it runs only the harness's own login command.
- **Sign-in needed.** An agent that is not signed in now shows "Sign-in needed" in the new-thread picker, with a button to sign in, instead of a raw error.

Update a device from Settings → Check/Update.
