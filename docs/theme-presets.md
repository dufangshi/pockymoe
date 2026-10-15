# Theme presets

Settings → Preferences → Appearance offers a **Theme** and a **Color mode**
(Light, Dark, System). Both are stored per browser. Classic and Plum Pocket
support both modes; an imported pack may support only one, and then the color
mode is fixed while it is applied (see [theme packs](theme-packs.md#color-modes)).

| Theme | Look |
| --- | --- |
| Classic (default) | Quiet green-neutral workbench |
| Plum Pocket | Plum chrome, mustard felt accents and cream content, rounded fonts, agent mascot avatars and the pocket sticker |

Every theme other than Classic is a [theme pack](theme-packs.md): Plum Pocket is
the built-in pack in `themes/plum-pocket/`, and more packs can be imported in
Settings. The choice is stored in `localStorage['pockymoe-theme-preset']`.

## Classic

Classic is the base styling and needs no pack: `apps/supervisor-web/src/matter-theme.css`
for the outer pages and the shared UI's `styles/matter-workbench.css` for the
workbench and dialogs. Packs override the same tokens one step more specifically.
Some shared UI rules hard-code Classic's slate neutrals; a pack that changes the
neutrals overrides those rules too (`themes/plum-pocket/styles/tokens.css` has
the current list).

## Plum Pocket

Its stylesheets are split by area: `tokens.css`, `shell.css`, `transcript.css`,
`workspace.css`, `composer.css`, `pages.css` and `controls.css`. It declares
every [capability flag](theme-packs.md#capability-flags).

- Nunito and Fredoka come from the pack's `fonts`; their OFL texts are next to
  them in `assets/fonts/`.
- Agent art (`assets/agents/`): an avatar and a composer mascot
  (`composer-peek-<agent>.webp`) for claude, codex, deepseek, gemini, grok, cursor,
  copilot and opencode; other agents get `agent.webp`. A new harness needs both
  images and an `agents` entry in `theme.json`.
- Thread rows, tabs and replies show the avatar; the mascot peeks over the
  composer, and the composer's busy seam and toolbox icons key on
  `data-composer-busy` and `data-toolbox-action`. The queue state of the send
  button reads "Send after this turn", and the 5h / 7d usage meter sits in the
  composer shell.
- Transcript replies open with the agent's avatar and name. Expanded step
  batches keep their child tools and subgroups behind a vertical guide in both
  themes; Plum Pocket uses contiguous 24px detail rows and 28px subgroup headers,
  and mixed-operation batch headers are 32px on phones. The outer work disclosure
  keeps its 44px touch target. The token and price breakdown is a small receipt.
- On phones the workbench top bar keeps 40px-tall controls, Home and Chat move
  into the thread drawer, and the device monitor stays as a compact CPU/memory
  meter whose values shade from green through amber to red with `--load`. Icon
  slots and the meter shrink when a thread adds tools or the connection badge
  appears (below 400px the meter shows only percentages), while the sidebar,
  settings and notifications buttons keep their size; an empty healthy
  connection slot takes no room.
- The thread tools open as a second line of the tab row (`.matter-breadcrumb`), so
  the theme lets `.matter-tabs-row` grow and fixes the height of the tab strip.
  The tab bar's tools (thread tools, subagents, automations, split) are redrawn as
  CSS masks over the existing buttons; labels, titles and badges stay.
- Outer pages (`data-page`) hide help paragraphs and eyebrows, turn boxed lists
  into plain rows and size buttons so their labels never wrap.
- Settings hide help text visually (`settings-hint`, kept for screen readers),
  give each settings tab an icon (icons only on phones), and restyle the shared
  buttons, inputs, switches, segmented controls, dialogs and menus.
- The terminal, the file editor and code blocks use the pack's `--terminal-*`,
  `--editor-*` and `--shiki-*` palettes.

## Checking a theme

Check both color modes on desktop and phone, including dialogs, menus, the
composer, code blocks and the outer pages. `e2e/settings-appearance.spec.ts`
covers switching and persistence, the `plum-*` and `theme-plum-composer` specs
cover Plum Pocket, and `e2e/theme-packs.spec.ts` covers import, export,
removal and single-mode packs.
