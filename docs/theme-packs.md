# Theme packs

A theme pack restyles the Web UI: tokens, layout, fonts and agent art. Settings →
Preferences → Appearance lists Classic (the built-in default, which needs no
pack), the built-in packs and the packs imported into this browser. Each card has
Export (every pack) and Remove (imported packs only); **Import…** takes a
`.pockytheme` file. The choice and the imported packs are stored per browser.
The built-in themes are described in [theme presets](theme-presets.md).

A pack keeps the meanings set by the [UI and interaction principles](ui-principles.md):
it may redraw status marks, icons and small controls, but every status stays
recognizable by shape alone, menus, windows and copy behave the same, and text
stays aligned on the shared baseline in the pack's fonts.

Packs hold no JavaScript. Their CSS is checked and rewritten so that it cannot
reach the network, but a pack can still hide or restyle any part of the page, so
import only packs you trust.

## Format

A `.pockytheme` file is a zip archive (stored or deflated entries). A zip of a
single folder that holds `theme.json` also works. Built-in packs use the same
layout as folders under `themes/<id>/` in this repository.

```
theme.json        manifest (required)
styles/*.css      stylesheets, applied in manifest order
assets/**         images (webp, png, jpg, gif, avif, svg) and fonts (woff2, woff)
preview.webp      optional card image; the card shows the swatches otherwise
LICENSE, *.txt    optional licence texts (fonts keep their own)
```

`theme.json`, schema 1:

```json
{
  "schema": 1,
  "id": "plum-pocket",
  "name": { "en": "Plum Pocket", "zh-CN": "梅子口袋" },
  "description": { "en": "…", "zh-CN": "…" },
  "version": "1.0.0",
  "author": "Pockymoe",
  "license": "MIT; fonts OFL-1.1",
  "engine": ">=0.12.89",
  "modes": ["light", "dark"],
  "swatches": ["#2f1a3b", "#fbf4e8", "#e8a93a", "#f0e0f4"],
  "chrome": { "light": "#2f1a3b", "dark": "#170d1d" },
  "styles": ["styles/tokens.css", "styles/shell.css"],
  "fonts": [{ "family": "Nunito", "src": "assets/fonts/nunito-latin.woff2", "weight": "400 800" }],
  "agents": {
    "default": { "avatar": "assets/agents/agent.webp" },
    "claude": { "avatar": "assets/agents/claude.webp", "peek": "assets/agents/composer-peek-claude.webp" }
  },
  "features": ["composer.richMenus", "transcript.agentByline"],
  "preview": "preview.webp"
}
```

| Field | Rules |
| --- | --- |
| `schema` | `1` |
| `id` | lowercase words joined by `-`; `classic`, `default`, `none`, `system`, `light`, `dark` and built-in ids are taken. Importing a pack with the id of an imported pack replaces it. |
| `name`, `description` | a string or a map of locales (`en`, `zh-CN`, …); the UI picks the locale, its language, then English |
| `version` | `1.2.3` (optionally `-pre` or `+build`) |
| `author`, `license`, `engine` | optional text. `engine` is the Pockymoe range the pack was made for; it is shown on the card and not enforced. |
| `modes` | `light` and/or `dark` (default both). A pack with one mode always shows in it; see [color modes](#color-modes). |
| `swatches` | up to 6 hex colors for the card |
| `chrome` | hex `<meta name="theme-color">` per mode |
| `styles` | 1–32 stylesheets, applied in this order |
| `fonts` | up to 16 `@font-face` rules: `family`, `src` (woff2/woff), optional `weight` (`400` or `"400 700"`) and `style` |
| `agents` | art per [agent key](#agent-art): `avatar` and `peek` images; `default` covers every other agent |
| `features` | [capability flags](#capability-flags); unknown flags are kept and ignored |

Unknown top-level fields are ignored, so a newer pack still loads.

### Color modes

A pack that lists both modes follows the color mode setting. A pack that lists
one mode fixes the page to that mode while it is applied, whatever the setting:
`data-theme-effective`, `color-scheme`, the theme-color meta (from `chrome`) and
the terminal, editor and code block palettes all use the pack's mode, so its
stylesheets only need rules for that mode. The setting itself is kept and comes
back with a pack that has both modes. Settings shows the color mode control
disabled with "Light only" (or "Dark only"), and the pack's card shows a small sun
or moon.

### Limits

64 files, 8 MB uncompressed in total, 10 MB archive, 1 MB per stylesheet, 64 KB
`theme.json`, 200-character paths. Paths are relative with forward slashes: no
`..`, `.` or empty segments, absolute paths, drive letters, backslashes, `?`, `#`,
`%`, quotes or control characters, and no two names that differ only in case.
Images and fonts must start with the bytes of their type. Encrypted, zip64 and
multi-disk archives are refused, and a deflated entry that grows past its
declared size stops at once.

## Stylesheets

Every top-level selector starts from `:theme`, the pack's root. The loader
rewrites it to `:root[data-theme-preset="<id>"]`, so rules apply only while the
pack is selected. Mode variants use `:theme[data-theme-effective='dark']` (or
`'light'`). Add `:root` to raise specificity, for example `:root:root:theme`.

```css
:root:root:theme { --theme-accent-solid: #e8a93a; }
:theme[data-theme-effective='dark'] .thread-ui-shell { --thread-gc-bg: #1d1124; }
:theme .matter-thread-avatar { display: block; background: var(--theme-agent-avatar) center / cover; }
:theme .product-title::before { background: url("asset:assets/pockymoe-icon.png") center / cover; }
@keyframes hop { to { transform: translateY(-2px); } }
@media (max-width: 640px) { :theme .matter-topbar { height: 48px; } }
```

- `url("asset:<path>")` names a pack file. Built-in packs resolve it to
  `/themes/<id>/<path>?v=<hash>`; imported packs to a `blob:` URL (images) or a
  `data:` URL (fonts, because the relay's content security policy allows
  `font-src data:` and not `blob:`). Strings in `image-set()` follow the same rule.
- `data:` URLs are allowed for `image/*` and `font/*`.
- `@keyframes` names and `@font-face` families, including the manifest `fonts`,
  are prefixed with `<id>--`. References in `animation`, `animation-name`,
  `font`, `font-family` and custom properties are rewritten, so write the plain
  names (`font-family: 'Nunito'`, `animation: hop 1s`).
- Allowed at-rules: `@media`, `@supports`, `@container` (also nested), and at the
  top level `@keyframes` and `@font-face`. `@charset` is dropped.
- Rejected, with the file and line in the error: `@import`, `@namespace` and any
  other at-rule; a `url()` or `src()` that is not `asset:` or an image/font
  `data:` URL (remote, protocol-relative, host-absolute and relative URLs
  included); `expression()`, `javascript:` and `vbscript:`; `behavior` and
  `-moz-binding`; escaped function, at-rule and property names; `url()` in
  selectors; blocks nested deeper than 24.

The compiler parses the CSS (tokens after CSS Syntax Level 3, escapes resolved)
rather than matching text, so escape tricks such as `u\72l(` are caught.

## Stable contract

A pack may rely on these. Selectors outside them (internal class names, lucide
icon classes, `data-slot`) work but can change between releases.

**Tokens.** `--theme-*` and `--app-*` (main app), `--matter-*` and
`--thread-gc-*` (shared UI, on `.thread-ui-shell` and `.thread-graph-dialog`).
Palettes painted by JavaScript read optional hex colors from CSS and re-read them
when the theme changes: `--terminal-bg`, `--terminal-fg`, `--terminal-cursor`,
`--terminal-selection` on `.terminal-panel`; `--editor-*` on an ancestor of the
Monaco editor and diff view; `--shiki-*` for code blocks with the
`code.cssSyntaxPalette` flag.

**Scrollbars.** Every scrolling box draws the same scrollbar from these tokens,
set on `:root` per mode: `--scrollbar-thumb-idle` (the box is not pointed at),
`--scrollbar-thumb` (it is), `--scrollbar-thumb-hover`, `--scrollbar-thumb-active`;
the shape `--scrollbar-size` (gutter, default 8px), `--scrollbar-inset` and
`--scrollbar-inset-hover` (space around the thumb, 2px and 1px), `--scrollbar-radius`; and an
optional rail shown while the box is pointed at, `--scrollbar-rail-y` and
`--scrollbar-rail-x` (a `background` value). Do not set `scrollbar-width` or
`scrollbar-color` except `scrollbar-width: none`: Chromium then draws the system
bar instead. Touch-only devices keep the platform's overlay indicator.

**Document attributes.** `<html>` carries `data-theme-preset` (the pack id or
`classic`), `data-theme-features` (the pack's flags, space separated) and
`data-theme-effective` (`light` or `dark`).

**Neutral hooks** that Classic ignores:

- outer pages: `data-page` on the page root (`workspaces`, `devices`,
  `relay-home`, `relay-portal`, `relay-guide`, `login`, `account`, `admin`,
  `thread-import`, `port-preview`);
- settings: `settings-hint` on help text, `settings-choice-group`,
  `settings-harness-picker`, `settings-harness-switch`, `settings-harness-row`;
  the theme cards are `theme-preset-card`, `theme-preset-option`,
  `theme-preset-name`, `theme-preset-meta`;
- workbench: `.matter-thread-avatar[data-agent]` on thread rows and tabs (hidden
  by default), `matter-agent-threads`, `matter-agent-threads-summary`,
  `matter-agent-threads-menu`, `matter-agent-threads-item`, `matter-drawer-tools`,
  and `--load` (0–100) inline on each device-monitor reading;
- composer: the form's `data-agent` and `data-composer-busy`, `data-send-intent`
  on the send button (`send`, `queue`, `goal`), `data-toolbox-action` on toolbox
  rows, `data-effort`, `data-variant`, `thread-composer-usage-tag`;
- transcript: `data-turn-status` on turns, `.thread-usage-details` and
  `.thread-usage-details-total` for the token and price breakdown;
- Explorer: `workspace-node-icon is-<kind>`, `workspace-file-alert`;
- thread map: `thread-map-dialog`, `thread-map-section` (`data-kind` `device` or
  `workspace`), `thread-map-tile`, both with `data-heat` (0–4) and `data-running`;
  `thread-map-agent[data-agent]` (shows `--theme-agent-avatar` when the pack has
  art for that agent); `thread-map-usage` (tokens and the price button),
  `thread-map-children` (the agent-thread button) and `thread-map-group` (a thread
  opened up into its agent threads); `--thread-map-heat` (default the accent),
  `--thread-map-tile` (the unheated fill), `--thread-map-spark` (the hourly bars)
  and `--thread-map-age-0` … `--thread-map-age-4` (the age pie, fresh to old);
- phone one-handed layout: the workbench's `has-phone-dock`, `is-left-handed`
  and `is-typing`; `.matter-topbar.is-status-only` (its readings keep their own
  size under a pack's top bar button rules); `matter-phone-dock`,
  `matter-phone-thread`, `matter-phone-panes`, `matter-phone-more`,
  `matter-phone-pins`, `matter-phone-sheet`; the turn strip `thread-jump-nav`
  (also `thread-jump-latest-badge`, its old name) with `data-following`; the
  drawer's `data-dragging` while it follows a finger; the composer-and-dock panel paints
  `--matter-dock-surface` and `--matter-dock-edge` (default `--matter-chrome`
  and `--theme-border`);
- icons: `data-icon="<name>"` on the icon `<svg>` names what it stands for, so a
  pack can draw its own glyph as a mask over `currentColor` (see below).

**Icon names.** The same name means the same thing wherever it appears; the
button around the icon keeps its label and state (`aria-pressed`,
`aria-expanded`), and a filled shortcut star keeps `fill="currentColor"`.

| Area | Names |
| --- | --- |
| Rail and top bar | `sidebar`, `back`, `forward`, `home`, `search`, `chat`, `terminal`, `explorer`, `thread-map`, `harness-panel`, `settings`, `notifications`, `help`, `close`; device monitor `cpu`, `memory`, `gpu` |
| Thread list and tabs | `shortcuts` (section), `shortcut` (thread star), `new-thread`, `thread-tools`, `agents`, `automations`, `split`, `restore`; thread actions `ports`, `share-link`, `sharing`, `download`; thread menu `more`, `open`, `copy`, `rename`, `delete` |
| Composer | `attach`, attach menu `photo`, `file`; toolbox rows `plan`, `goal`, `fast`, `compact`, `fork`, `skills`, `mcp`, `hooks`, `harness`, `prompt`, `unsupported` (the send button uses `data-send-intent`) |
| Transcript | `think`, `wait`, `wake`, `copy`, `copied`; timeline `previous-turn`, `latest`, `next-turn` (step icons use the `is-<tone>` classes) |
| Explorer and files | nodes `folder`, `folder-open`, `folder-link`, `archive`, `image`, `code`, `file`; header `new-file`, `filter`, `collapse-folders`, `refresh`, `more`, `upload`, `delete`, `expand-viewer`, `collapse-explorer`; rows `copy-path`, `copy-absolute-path`, `more`, `download`, `rename`, `delete`, `properties`; tabs `code`, `close`, `unsaved`; preview `back`, `forward`, `source`, `preview`, `edit`, `save`, `close`, `more`, `expand-explorer`, `collapse-viewer` |
| Outer pages | navigation `menu`, `close`, `devices`, `settings`, `help`; workspaces `thread-map`, `import`, `add`, `refresh`, `pin`, `more`, `view-path`, `rename`, `delete` |
| Dialogs | `close` on every dialog's close button |

A pack replaces a glyph like this; the lucide paths inside stay in the markup:

```css
:theme svg[data-icon='back'] { background: currentColor; mask: var(--my-back) center / contain no-repeat; }
:theme svg[data-icon='back'] > * { display: none; }
```

### Agent art

Every agent maps to one canonical key (`themeAgentKey` in the shared UI): thread
agent ids, providers and harness labels such as `claude-code` and "Claude Code"
all become `claude`; `dsh` becomes `deepseek`; the families are claude, opencode,
codex, deepseek, copilot, gemini, grok, cursor, qwen and kimi. Any other agent
uses its id as a slug. The loader turns `agents` into
`--theme-agent-avatar` and `--theme-agent-peek` on `[data-agent="<key>"]`
elements (`default` everywhere else), so a pack's CSS only uses the two
variables.

### Capability flags

Some looks need different markup, which CSS cannot add. The shared UI renders it
when the applied pack lists the flag (`useThemeFeature(flag)`):

| Flag | Effect |
| --- | --- |
| `workbench.phoneDrawerTools` | With the phone dock switched off, Home and Chat move from the top bar into the thread drawer |
| `composer.modelAvatar` | The model chip shows the agent avatar and a menu chevron |
| `composer.richMenus` | Icon rows with descriptions in the slash toolbox and attach menu, a `/` glyph and a paperclip trigger |
| `composer.intentIcons` | A paper plane for "Send" (every theme already shows distinct Send, "Send after this turn" and "Set goal" icons) |
| `transcript.agentByline` | Each reply opens with the agent's avatar and name |
| `explorer.shortMenuLabels` | No longer needed: Explorer row-menu labels are always short, with the node name in the accessible name |
| `code.cssSyntaxPalette` | Code blocks use the CSS-variable Shiki theme (`--shiki-*`) |

## How it works

- `apps/supervisor-web/src/lib/themePacks/` holds the archive reader and writer
  (`zip.ts`), limits and paths (`limits.ts`), the manifest validator
  (`manifest.ts`), the CSS tokenizer and compiler (`cssTokenizer.ts`, `css.ts`),
  pack validation and compilation (`pack.ts`), IndexedDB storage
  (`storage.ts`, database `pockymoe-theme-packs`) and the registry (`registry.ts`).
- The Vite plugin `apps/supervisor-web/build/themePacksPlugin.ts` validates and
  compiles `themes/*` at build time. It publishes `/themes/<id>.css`, the pack's
  files under `/themes/<id>/` and `/themes/index.json`, and gives the app the
  manifests through `virtual:pockymoe-theme-packs`. The dev server serves the same
  paths from the source folders and reloads when they change. A Classic user
  downloads no theme CSS.
- Applying a pack fills the single `<style data-theme-pack>` that `main.tsx`
  creates before the app renders (after the host's stylesheets, before lazily
  loaded ones), then sets `data-theme-preset` and `data-theme-features` and the
  theme-color meta. Imported packs are validated and compiled again when applied.
- The registry's `fixedMode` is the applied pack's only mode (or null);
  `app.tsx` passes the setting through `effectiveColorMode` and gives the shells
  the pack's mode as `data-theme-mode` (their palettes key on it), and the registry sets
  `data-theme-effective` and `color-scheme` together with a single-mode pack's
  stylesheet so it never shows with the other mode's tokens.
- `main.tsx` waits for the stored pack (at most 1.5 s) before the first render.
  An unknown, missing or broken pack falls back to Classic. Other tabs follow
  through `storage` events and a `BroadcastChannel`.
- A transcript export inlines the document's stylesheets with their assets, which
  include only the applied pack, and keeps its `data-theme-preset` and flags.

## Making and testing a pack

1. Export Plum Pocket from Settings (or copy `themes/plum-pocket/`) and change
   `id`, `name` and the styles.
2. Zip the folder's contents (or the folder) and rename it to `.pockytheme`, then
   use Import…. Errors name the file and line.
3. Check each color mode the pack lists on desktop and phone: dialogs, menus, the composer,
   code blocks, the terminal, the editor and the outer pages.

A built-in pack is a folder under `themes/` whose name equals its `id`; the build
fails when it does not validate. Tests: `apps/supervisor-web/src/lib/themePacks/*.test.ts`
(validator, archives, CSS rules, registry) and `e2e/theme-packs.spec.ts`
(export, import, apply, remove, single-mode and malicious packs).
