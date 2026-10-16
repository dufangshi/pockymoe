Phone navigation, the thread map and failed turns read more clearly.

- **Running threads**: the top bar's count is the account's running threads,
  shown with a threads mark. Tapping it opens only those threads, each with how
  long it has run, then finished threads that were opened before and not read
  since. It counts running threads on every device the navigation reads.
- **Thread map zooms**: tapping a device or workspace fills the map with it, and
  Back returns. On touch, pinching in goes up a level, spreading opens the block
  under the fingers and dragging down closes the map. On a phone the map fills
  the screen and its legend keeps clear of the bottom edge.
- **More (⋯)**: the thread map, workspace drawer, Home, Back and Settings sit in a
  row fixed at the bottom of the sheet, so Settings shows without scrolling.
- **Files prompt**: the prompt under the phone file browser edits the same draft
  as the chat composer, so switching views keeps the text, and it sends the
  composer's attachments too.
- **Prompt time and Copy** sit beside the prompt's text, or under its right end,
  instead of covering it (including the Terminal pack's ❯ mark).
- **Failed turns**: the prompt carries a red mark that opens the error under it.
  The red bar above the thread and the reply that only repeated the error are
  gone, and Claude's `<synthetic>` model name no longer replaces the model that
  ran.

Validated with the shared UI and Web test suites, the runtime Claude usage
tests, and phone screenshots in the default, Plum and Terminal themes. Relay and
Windows Device Manager are unchanged. Installing this release is a separate
device update; publication does not restart an existing Supervisor.
