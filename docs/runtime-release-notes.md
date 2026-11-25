Terminals scroll again in full-screen programs such as Codex and Claude Code, and Codex forks no longer fail on long conversations.

**Fixes**

- **Scrolling Codex and Claude Code in a terminal.** After a long session, reopening the page or switching back to the thread drew a full-screen program in the normal buffer, and the mouse wheel no longer reached it. The terminal now restores the program's screen and mouse modes when it is reopened, and no longer starts its replay inside an escape sequence.
- **Scrolling on a phone.** A finger drag in a full-screen program (Codex, Claude Code, less, vim) now scrolls the program, as the mouse wheel does on a computer. At a shell prompt it still scrolls the terminal's history.
- **Forking a long Codex conversation.** Forking a Codex thread with a long history could fail because the native fork answer carried every inherited turn and exceeded the transport limit. The fork now asks only for the new thread.

Update a device from Settings → Check/Update.
