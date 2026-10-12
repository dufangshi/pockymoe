import type { ThemeManifest } from './manifest';

/** A built-in pack as the build publishes it (see build/themePacksPlugin.ts). */
export interface BuiltinThemePack {
  id: string;
  /** Content hash; every URL below carries it as `?v=`. */
  hash: string;
  manifest: ThemeManifest;
  /** Pack paths, `theme.json` included. */
  files: string[];
  /** URL prefix of the pack's files (`/themes/<id>/`). */
  base: string;
  /** URL of the compiled stylesheet. */
  css: string;
}
