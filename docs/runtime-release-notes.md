Preview links explain themselves, delegates can be started without belonging to their creator, and the device pages are one place per device.

**New**

- **Device center.** Each device has one page with Overview, Settings and Access tabs; its Harness, upstream, resource, environment, Supervisor and workspace settings live there. A device switcher heads the sidebars, devices shared with you included. Phones use the same page.
- **One menu for devices and workspaces.** Rename, share, isolate, settings and remove open in place from the same ⋯ menu everywhere, and phones can rename a device.
- **`pockymoe thread create --detach`.** Starts a thread that is not the creator's delegate: it is its own root, outside the creator's limits and control, starts guarded, and records who created it (at most 10 an hour per lineage).

**Fixes**

- **Preview links.** When the service behind a preview is not running, its link now shows at once what happened and what to do, instead of waiting and ending in a Cloudflare 502. A stopped preview's address says so too. Closing or deleting a thread whose processes serve a preview is refused unless `--stop-preview-servers` is given, so a link is no longer left pointing at a service that was just ended.
- **Steer in one click.** A late queue update no longer puts a message you just steered back in the queue.
- **Zombie processes.** A thread's host worker now cleans up the processes it adopts while it runs, instead of only when the thread is released.
- **Settings act on the device you are looking at**, and name it.
- **Mac shortcuts.** Only ⌘ opens search and the other Mod shortcuts; Ctrl+F and Ctrl+K stay with the text field and the terminal.
- **Thread creation is bound to its caller.** An agent can no longer create threads in another thread's lineage or outside its own limits.

Update a device from Settings → Check/Update.
