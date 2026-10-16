Save an API connection once, then choose it from a harness or a plugin.

- **Shared connections**: Settings, harness pages and Content Quality use the same
  add/edit form. Saving a connection never changes a harness. Apply is an explicit
  action, with a model choice and a native configuration backup; later edits are
  marked until they are applied again.
- **Account directory**: connections on your other devices appear in the same
  list. Sync copies an API-key connection over the existing certified, encrypted
  device channel. It remains separate from Apply, so a synced connection can be
  used directly by a plugin. Relay stores discovery metadata only; credentials
  stay on the devices. OAuth transfer is not included.
- **Compact settings**: shorter action labels, less repeated status text, smaller
  headings and tighter spacing on phones. Long names no longer push actions
  outside the dialog. Content Quality selects a saved connection directly; its
  existing private-file configuration remains supported.
- **Conversation display**: a compact turn head opens its details on one line;
  wide screens show the full heading, prompt photos use smaller previews, files
  open on request with window controls, map levels respond to clicks, and hover
  actions float above the text. Folded work opens in place and minimized files
  dock on the right.

Validated with focused Rust connection and plugin tests, encrypted two-device
integration, a real editorial-model request, and component and browser checks for
desktop and narrow phone settings. The release workflow also verifies the
workspace, native updates, packaging and all four native platforms.

Web and Relay deploy separately. Existing Supervisors need the device management
update to use the new connection APIs. Windows Device Manager is unchanged.
