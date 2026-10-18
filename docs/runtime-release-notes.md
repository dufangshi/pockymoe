Pockymoe inside Treer gets its Settings back and can open itself full page.

- **Settings in Treer**: the gear returns in a Treer Agent tab with two tabs,
  Session (this thread's own model and harness settings) and Preferences
  (language, theme and theme packs, color mode, text size, phone dock, folding,
  status summaries). Built-in theme packs now load through the Treer tunnel
  instead of falling back to Classic. Tabs that administer the whole device stay
  hidden, and standalone Settings are unchanged.
- **Open in a new browser tab**: inside a Treer frame the workbench rail (phone:
  top bar) has a button that opens the same page full size. It needs a Treer
  whose frame allows popups (EvoEvolver/treer#17); in an older Treer the button
  explains Treer's own tab menu entry instead.
- **Consistent interface**: one copy button, path chip, status mark and
  confirmation dialog everywhere; menus work with arrow keys and long press;
  send intents, empty-send and queued-prompt editing behave alike in every
  theme; tool failures say so with Retry; New thread stays pinned in the tab
  strip and search marks what it found; phone hit areas are at least 40 px; the
  wording uses one name per concept.

Validated with the Treer crate and focused Web/shared UI tests, and a browser
check of the Treer tunnel: scoped Settings, a theme pack through the tunnel, and
the new-tab button in framed pages with and without popups. Windows Device
Manager is unchanged. Installing this release is a separate device update;
publication does not restart an existing Supervisor.
