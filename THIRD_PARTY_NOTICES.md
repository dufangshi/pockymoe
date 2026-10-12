# CC Switch

Source: https://github.com/farion1231/cc-switch
Revision: `889b797d8aa252299221ed6569f992bda0a31a72`

Adapted components and provider preset data:
- `src/components/providers/ProviderCard.tsx`, `ProviderCardActions.tsx`,
  `ProviderList.tsx` and provider form sections in the corresponding Web components.
- Direct API preset settings from `src/config/{claude,codex,gemini}ProviderPresets.ts`
  in `apps/supervisor-web/src/components/upstreamPresets.ts` (189 presets).
- Claude, Codex and Gemini field lists from `src-tauri/src/live/floor.rs`
  in `crates/runtime/src/upstreams/cc_switch.rs`. Tool credentials and helper
  commands are preserved as global user settings at the integration boundary.
- Provider fragment capture/projection and outgoing live backfill from
  `src-tauri/src/live/project` and `src-tauri/src/services/provider/live.rs`
  in `upstreams/provider_config.rs` and `upstreams.rs`.
- Plan/stage/reread/rename conflict checks from `src-tauri/src/live/engine.rs`
  in `upstreams/write_engine.rs` and `upstreams.rs`.

Pockymoe retains its profile store, backups, private atomic writes, harness
session gates and HTTP API. CC Switch's Tauri database, desktop services, local
proxy, OAuth accounts and arbitrary supplier usage scripts are not copied.
DSH YAML projection is an original adapter against the local DSH provider schema.

MIT License

Copyright (c) 2025 Jason Young

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

# Fonts

The Web UI bundles these fonts under the SIL Open Font License 1.1. Each license
is next to its font: DM Sans in `apps/supervisor-web/public/fonts/`, Nunito and
Fredoka in the Plum Pocket theme pack (`themes/plum-pocket/assets/fonts/`).

- DM Sans (`dm-sans-OFL.txt`), https://github.com/googlefonts/dm-fonts
- Nunito (`nunito-OFL.txt`), https://github.com/googlefonts/nunito
- Fredoka (`fredoka-OFL.txt`), https://github.com/hafontia/Fredoka-One
