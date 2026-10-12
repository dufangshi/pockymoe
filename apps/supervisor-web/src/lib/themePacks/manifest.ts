/* `theme.json`, schema 1. Hand-written validation: the manifest is small and the
   rules are specific (paths, colors, the agent key format). Unknown top-level keys
   are ignored so a newer pack still loads; unknown feature flags are kept and
   ignored by the shared UI. */

import { packError } from './errors';
import { isFontPath, isImagePath, normalizePackPath, packFileKind } from './limits';

export type LocalizedText = string | Record<string, string>;
export type ThemeMode = 'light' | 'dark';

export interface ThemeFont {
  family: string;
  src: string;
  weight?: string;
  style?: 'normal' | 'italic';
}

export interface ThemeAgentArt {
  avatar?: string;
  peek?: string;
}

export interface ThemeManifest {
  schema: 1;
  id: string;
  name: LocalizedText;
  description?: LocalizedText;
  version: string;
  author?: string;
  license?: string;
  engine?: string;
  modes: ThemeMode[];
  swatches: string[];
  chrome?: Partial<Record<ThemeMode, string>>;
  styles: string[];
  fonts: ThemeFont[];
  agents: Record<string, ThemeAgentArt>;
  features: string[];
  preview?: string;
}

/** Ids that packs may not take. */
export const RESERVED_THEME_IDS = new Set(['classic', 'default', 'none', 'system', 'light', 'dark']);
export const THEME_ID_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
/** The same pattern as the shared UI's `THEME_AGENT_KEY_PATTERN` (kept DOM-free here
 *  for the build plugin; a test checks they match). */
export const AGENT_KEY_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const FEATURE_PATTERN = /^[a-z][a-zA-Z0-9]*(?:\.[a-z][a-zA-Z0-9]*)+$/;
const HEX_COLOR = /^#(?:[0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
const VERSION = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;
const LOCALE = /^[a-z]{2,3}(?:-[A-Za-z0-9]{2,8})*$/;
const FAMILY = /^[A-Za-z0-9][A-Za-z0-9 _-]{0,63}$/;
const WEIGHT = /^[1-9]00(?: [1-9]00)?$/;

function fail(detail: string): never {
  packError('manifest', detail);
}
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function text(value: unknown, field: string, max: number, optional = false): string | undefined {
  if (value === undefined && optional) return undefined;
  if (typeof value !== 'string' || !value.trim()) fail(`${field} must be a non-empty string`);
  // eslint-disable-next-line no-control-regex
  if (value.length > max || /[\u0000-\u001f\u007f]/.test(value)) fail(`${field} must be at most ${max} plain characters`);
  return value.trim();
}
function localized(value: unknown, field: string, max: number, optional = false): LocalizedText | undefined {
  if (value === undefined && optional) return undefined;
  if (typeof value === 'string') return text(value, field, max);
  if (!isRecord(value) || !Object.keys(value).length || Object.keys(value).length > 16) fail(`${field} must be a string or a map of locales`);
  const out: Record<string, string> = {};
  for (const [locale, entry] of Object.entries(value)) {
    if (!LOCALE.test(locale)) fail(`${field}: "${locale}" is not a locale`);
    out[locale] = text(entry, `${field}.${locale}`, max)!;
  }
  return out;
}
function list(value: unknown, field: string, max: number, optional = true): unknown[] {
  if (value === undefined && optional) return [];
  if (!Array.isArray(value) || value.length > max) fail(`${field} must be a list of at most ${max}`);
  return value;
}
function path(value: unknown, field: string, check: (path: string) => boolean, kind: string): string {
  if (typeof value !== 'string') fail(`${field} must be a path`);
  let normalized: string;
  try {
    normalized = normalizePackPath(value, field);
  } catch {
    fail(`${field}: "${value}" is not a safe relative path`);
  }
  if (!check(normalized)) fail(`${field}: "${value}" must be ${kind}`);
  return normalized;
}
function color(value: unknown, field: string) {
  if (typeof value !== 'string' || !HEX_COLOR.test(value)) fail(`${field} must be a hex color`);
  return value.toLowerCase();
}

export function validateManifest(input: unknown): ThemeManifest {
  if (!isRecord(input)) fail('theme.json must be an object');
  if (input.schema !== 1) fail(`schema ${JSON.stringify(input.schema)} is not supported (expected 1)`);
  const id = text(input.id, 'id', 40)!;
  if (!THEME_ID_PATTERN.test(id) || id.length < 2) fail('id must be lowercase words joined by dashes');
  if (RESERVED_THEME_IDS.has(id)) packError('reserved', `"${id}" is reserved`);
  const version = text(input.version, 'version', 32)!;
  if (!VERSION.test(version)) fail('version must look like 1.0.0');

  const modes = list(input.modes, 'modes', 2).map((mode) => {
    if (mode !== 'light' && mode !== 'dark') fail('modes may only contain "light" and "dark"');
    return mode;
  });
  const swatches = list(input.swatches, 'swatches', 6).map((value, index) => color(value, `swatches[${index}]`));
  let chrome: ThemeManifest['chrome'];
  if (input.chrome !== undefined) {
    if (!isRecord(input.chrome)) fail('chrome must map light/dark to colors');
    chrome = {};
    for (const [mode, value] of Object.entries(input.chrome)) {
      if (mode !== 'light' && mode !== 'dark') fail(`chrome.${mode} is not a color mode`);
      chrome[mode] = color(value, `chrome.${mode}`);
    }
  }
  const styles = list(input.styles, 'styles', 32, false).map((value, index) => path(value, `styles[${index}]`, (p) => packFileKind(p) === 'stylesheet', 'a .css file'));
  if (!styles.length) fail('styles must name at least one stylesheet');
  if (new Set(styles).size !== styles.length) fail('styles lists a file twice');

  const fonts = list(input.fonts, 'fonts', 16).map((value, index): ThemeFont => {
    const field = `fonts[${index}]`;
    if (!isRecord(value)) fail(`${field} must be an object`);
    const family = text(value.family, `${field}.family`, 64)!;
    if (!FAMILY.test(family)) fail(`${field}.family may only use letters, digits, spaces, _ and -`);
    const font: ThemeFont = { family, src: path(value.src, `${field}.src`, isFontPath, 'a .woff2 or .woff file') };
    if (value.weight !== undefined) {
      const weight = String(value.weight);
      if (!WEIGHT.test(weight)) fail(`${field}.weight must be like 400 or "400 700"`);
      font.weight = weight;
    }
    if (value.style !== undefined) {
      if (value.style !== 'normal' && value.style !== 'italic') fail(`${field}.style must be normal or italic`);
      font.style = value.style;
    }
    return font;
  });

  const agents: Record<string, ThemeAgentArt> = {};
  if (input.agents !== undefined) {
    if (!isRecord(input.agents) || Object.keys(input.agents).length > 64) fail('agents must map at most 64 agent keys to art');
    for (const [key, value] of Object.entries(input.agents)) {
      if (key !== 'default' && !AGENT_KEY_PATTERN.test(key)) fail(`agents: "${key}" is not an agent key`);
      if (!isRecord(value)) fail(`agents.${key} must be an object`);
      const art: ThemeAgentArt = {};
      if (value.avatar !== undefined) art.avatar = path(value.avatar, `agents.${key}.avatar`, isImagePath, 'an image');
      if (value.peek !== undefined) art.peek = path(value.peek, `agents.${key}.peek`, isImagePath, 'an image');
      agents[key] = art;
    }
  }

  const features = list(input.features, 'features', 64).map((value, index) => {
    if (typeof value !== 'string' || !FEATURE_PATTERN.test(value) || value.length > 64) fail(`features[${index}] is not a flag name`);
    return value;
  });

  const manifest: ThemeManifest = {
    schema: 1,
    id,
    name: localized(input.name, 'name', 60)!,
    version,
    modes: modes.length ? [...new Set(modes)] : ['light', 'dark'],
    swatches,
    styles,
    fonts,
    agents,
    features: [...new Set(features)],
  };
  const description = localized(input.description, 'description', 200, true);
  if (description) manifest.description = description;
  const author = text(input.author, 'author', 80, true);
  if (author) manifest.author = author;
  const license = text(input.license, 'license', 40, true);
  if (license) manifest.license = license;
  const engine = text(input.engine, 'engine', 40, true);
  if (engine) manifest.engine = engine;
  if (chrome) manifest.chrome = chrome;
  if (input.preview !== undefined) manifest.preview = path(input.preview, 'preview', isImagePath, 'an image');
  return manifest;
}

/** Every pack file the manifest points at. */
export function manifestReferences(manifest: ThemeManifest) {
  const refs = [...manifest.styles, ...manifest.fonts.map((font) => font.src)];
  for (const art of Object.values(manifest.agents)) {
    if (art.avatar) refs.push(art.avatar);
    if (art.peek) refs.push(art.peek);
  }
  if (manifest.preview) refs.push(manifest.preview);
  return refs;
}

/** The text for a locale: exact, then its language, then English, then any. */
export function localizedText(value: LocalizedText | undefined, locale: string): string {
  if (value === undefined) return '';
  if (typeof value === 'string') return value;
  const language = locale.split('-')[0]!;
  return value[locale] ?? Object.entries(value).find(([key]) => key.split('-')[0] === language)?.[1] ?? value.en ?? Object.values(value)[0] ?? '';
}
