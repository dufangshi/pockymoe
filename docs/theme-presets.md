# Theme presets

Settings → Preferences → Appearance offers a **Theme** (preset) and a **Color mode**
(Light, Dark, System). Both are stored per browser, and every preset supports both
modes.

| Preset | Look |
| --- | --- |
| Classic (default) | Quiet green-neutral workbench |
| Plum Pocket | Plum chrome, mustard felt accents and cream content, rounded fonts, agent mascot avatars and the pocket sticker |

## How it works

- `apps/supervisor-web/src/lib/themePreset.ts` stores the preset in
  `localStorage['pockymoe-theme-preset']` and sets `<html data-theme-preset>` at
  startup, on change and when another tab changes it. The color mode stays on
  `data-theme-effective`.
- **Classic** is the base styling: `apps/supervisor-web/src/matter-theme.css` for the
  outer pages and the shared UI's `styles/matter-workbench.css` for the workbench and
  dialogs.
- A preset is a stylesheet (or a folder of them) under `apps/supervisor-web/src/theme-presets/`, imported
  from `index.css`. It overrides the same tokens one step more specifically:
  - main app tokens (`--app-*`, `--theme-*`) on `:root:root:root[data-theme-preset='…']`,
    with a `[data-theme-effective='light']` variant;
  - shared UI tokens (`--thread-gc-*`, `--matter-*`) on `.thread-ui-shell` and
    `.thread-graph-dialog` under the preset attribute, with a dark variant.
- Some shared UI rules hard-code Classic's slate neutrals. A preset that changes the
  neutrals overrides those rules too; `plum-pocket/tokens.css` has the current list.
- The terminal (xterm) and the file editor and diff view (Monaco) paint with
  JavaScript palettes. They read optional hex colors from CSS, `--terminal-bg`,
  `--terminal-fg`, `--terminal-cursor`, `--terminal-selection` on `.terminal-panel`
  and `--editor-bg`, `--editor-fg` and related `--editor-*` variables on an
  ancestor, and re-read them when `data-theme-preset` changes
  (`components/themeHooks.ts` in the shared UI). Without them they keep the Classic
  palette.
- Thread rows and tabs carry `<span class="matter-thread-avatar" data-agent="…">`,
  hidden by default. A preset can show it with a background image per agent.
  Plum Pocket's images are in `apps/supervisor-web/public/theme/plum-pocket/`.
- Agent-thread groups carry neutral class hooks (`matter-agent-threads`,
  `matter-agent-threads-summary`, `matter-agent-threads-menu`,
  `matter-agent-threads-item`) that only a preset styles.
- On phones, Plum Pocket keeps the workbench top bar to 40px-tall controls and moves
  Home and Chat into the thread drawer (`matter-drawer-tools`). The device monitor
  stays in the top bar as a compact CPU/memory meter. The workbench checks the
  preset with `useDocumentThemePreset()`; Classic keeps all of them in the top bar.
- Each live device-monitor reading carries `--load` (0–100) as an inline style.
  Plum Pocket shades the value from green through amber to red; Classic ignores it.
- The thread tools open as a second line of the tab row (`.matter-breadcrumb`), so a
  preset must let `.matter-tabs-row` grow and fix the height of the tab strip instead.
- The chat composer form carries `data-agent` (derived from the harness label) and
  `data-composer-busy` while a turn runs, and the toolbox rows carry
  `data-toolbox-action`. Classic ignores them. Plum Pocket uses them for the
  mascot that peeks over the composer (`composer-peek-*.webp`), the busy seam and
  the toolbox icons; its extra composer markup (mascot on the model chip, menu
  icons, per-intent send icons) renders only when `useDocumentThemePreset()` is
  `plum-pocket`. Under Plum Pocket the 5h / 7d usage meter moves from the row
  below the composer into the shell (`thread-composer-usage-tag`). The send button carries
  `data-send-intent` (`send`, `queue` while a turn runs, `goal`); Plum Pocket
  labels the queue state "Send after this turn".
- Settings help text carries the neutral class `settings-hint`, and a few settings
  groups carry `settings-choice-group`, `settings-harness-picker`,
  `settings-harness-switch` and `settings-harness-row`. Plum Pocket
  (`plum-pocket/controls.css`) hides hints visually but keeps them for screen
  readers, gives each settings tab an icon (icons only on phones), and styles the
  shared buttons, inputs, switches, segmented controls, dialogs and menus.
- Transcript turns carry `data-turn-status`, and under Plum Pocket each reply opens
  with the replying agent's avatar and name (`graph-chat/TranscriptAgent.tsx`).
- Outer pages (workspaces, devices, relay home, sign-in, account, admin, guide,
  import, port preview) carry a neutral `data-page` attribute on their root.
  `plum-pocket/pages.css` uses it to hide help paragraphs and eyebrows, turn boxed
  lists into plain rows and size buttons so their labels never wrap.
- Plum Pocket redraws the thread tab bar's tools (thread tools, subagents,
  automations, split) as CSS masks over the existing buttons, keyed on the lucide
  icon classes. Labels, titles and badges stay as they are.

## Adding a preset

1. Add its id to `THEME_PRESETS`, a card in `components/ThemePresetSettings.tsx`, a
   `themeChromeColor` entry and the `files.themePreset…` strings in the shared UI
   translations.
2. Add `theme-presets/<id>.css` and import it from `index.css`.
3. Check both color modes on desktop and phone, including dialogs, menus, the
   composer, code blocks and the outer pages. `e2e/settings-appearance.spec.ts`
   covers switching and persistence.
