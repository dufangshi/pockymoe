/* A theme pack as files: validation of the file set and manifest, and the
   compiled stylesheet. DOM-free, so the Vite plugin uses it for built-in packs. */

import { compileStylesheets, quote } from './css';
import { packError } from './errors';
import { MANIFEST_FILE, THEME_PACK_LIMITS, checkAssetSignature, normalizePackPath, packFileKind } from './limits';
import { manifestReferences, validateManifest, type ThemeManifest } from './manifest';
import { readZip, writeZip } from './zip';

export interface ThemePackFiles {
  manifest: ThemeManifest;
  /** Every file by its pack path, `theme.json` included. */
  files: Map<string, Uint8Array>;
}

const utf8 = new TextDecoder('utf-8', { fatal: true });

function decode(path: string, bytes: Uint8Array) {
  try {
    return utf8.decode(bytes).replace(/^﻿/, '');
  } catch {
    packError('asset', `${path}: not UTF-8 text`);
  }
}

/** Validates a pack's files (from an archive or a directory) and its manifest. */
export function validatePackFiles(input: Iterable<[string, Uint8Array]>): ThemePackFiles {
  const files = new Map<string, Uint8Array>();
  const seen = new Set<string>();
  let total = 0;
  for (const [raw, bytes] of input) {
    const path = normalizePackPath(raw);
    const folded = path.toLowerCase();
    if (seen.has(folded)) packError('path', `${path}: duplicate file`);
    seen.add(folded);
    const kind = packFileKind(path);
    if (!kind) packError('asset', `${path}: unsupported file type`);
    if (kind === 'manifest' && bytes.length > THEME_PACK_LIMITS.manifestBytes) packError('limits', 'theme.json is larger than 64 KB');
    if (kind === 'stylesheet' && bytes.length > THEME_PACK_LIMITS.stylesheetBytes) packError('limits', `${path} is larger than 1 MB`);
    if (kind === 'image' || kind === 'font') checkAssetSignature(path, bytes);
    total += bytes.length;
    files.set(path, bytes);
  }
  if (files.size > THEME_PACK_LIMITS.files) packError('limits', `more than ${THEME_PACK_LIMITS.files} files`);
  if (total > THEME_PACK_LIMITS.totalBytes) packError('limits', 'the files add up to more than 8 MB');
  const manifestBytes = files.get(MANIFEST_FILE);
  if (!manifestBytes) packError('archive', 'theme.json is missing');
  let json: unknown;
  try {
    json = JSON.parse(decode(MANIFEST_FILE, manifestBytes));
  } catch (error) {
    if (error instanceof Error && error.name === 'ThemePackError') throw error;
    packError('manifest', 'theme.json is not valid JSON');
  }
  const manifest = validateManifest(json);
  for (const ref of manifestReferences(manifest)) if (!files.has(ref)) packError('asset', `${ref} is listed in theme.json but missing`);
  return { manifest, files };
}

/** Reads a `.pockytheme` archive. A single top-level folder (a zipped pack
 *  directory) is accepted and stripped. */
export async function readThemePackArchive(bytes: Uint8Array): Promise<ThemePackFiles> {
  const entries = await readZip(bytes);
  let prefix = '';
  if (!entries.some((entry) => entry.name === MANIFEST_FILE)) {
    const roots = new Set(entries.map((entry) => entry.name.split('/')[0]));
    const [root] = roots;
    if (roots.size === 1 && root && entries.some((entry) => entry.name === `${root}/${MANIFEST_FILE}`)) prefix = `${root}/`;
  }
  return validatePackFiles(entries.map((entry) => [prefix && entry.name.startsWith(prefix) ? entry.name.slice(prefix.length) : entry.name, entry.bytes]));
}

/** A `.pockytheme` archive with the manifest first. */
export function writeThemePackArchive(pack: ThemePackFiles) {
  const names = [...pack.files.keys()].sort((a, b) => (a === MANIFEST_FILE ? -1 : b === MANIFEST_FILE ? 1 : a.localeCompare(b)));
  return writeZip(names.map((name) => ({ name, bytes: pack.files.get(name)! })));
}

export type AssetResolver = (path: string) => string;

/** Rules generated from the manifest: @font-face for `fonts`, and the agent art as
 *  `--theme-agent-avatar` / `--theme-agent-peek` on `[data-agent]` elements. */
export function generatedRules(manifest: ThemeManifest, resolve: AssetResolver) {
  const root = `:root[data-theme-preset="${manifest.id}"]`;
  const rules: string[] = [];
  for (const font of manifest.fonts) {
    const parts = [`font-family: ${quote(`${manifest.id}--${font.family}`)}`, `src: url(${quote(resolve(font.src))}) format(${quote(font.src.endsWith('.woff') ? 'woff' : 'woff2')})`];
    if (font.weight) parts.push(`font-weight: ${font.weight}`);
    parts.push(`font-style: ${font.style ?? 'normal'}`, 'font-display: swap');
    rules.push(`@font-face { ${parts.join('; ')}; }`);
  }
  const fallback = manifest.agents.default ?? {};
  const art = (path: string | undefined) => (path ? `url(${quote(resolve(path))})` : 'none');
  const vars = (entry: { avatar?: string; peek?: string }) =>
    `--theme-agent-avatar: ${art(entry.avatar ?? fallback.avatar)}; --theme-agent-peek: ${art(entry.peek ?? fallback.peek)};`;
  if (Object.keys(manifest.agents).length) {
    rules.push(`${root}, ${root} [data-agent] { ${vars(fallback)} }`);
    for (const [key, entry] of Object.entries(manifest.agents)) if (key !== 'default') rules.push(`${root} [data-agent="${key}"] { ${vars(entry)} }`);
  }
  return rules.join('\n');
}

/** The pack's whole stylesheet: generated rules, then the styles in manifest order. */
export function compileThemePack(pack: ThemePackFiles, resolve: AssetResolver): string {
  const { manifest, files } = pack;
  const sheets = manifest.styles.map((file) => ({ file, source: decode(file, files.get(file)!) }));
  const styles = compileStylesheets(sheets, {
    id: manifest.id,
    fontFamilies: manifest.fonts.map((font) => font.family),
    resolveAsset: (path) => {
      if (!files.has(path)) packError('asset', `asset:${path} is missing`);
      return resolve(path);
    },
  });
  return `/* Theme pack ${manifest.id} ${manifest.version} */\n${generatedRules(manifest, resolve)}\n\n${styles}\n`;
}

/** A short content hash of the pack: built-in URLs carry it for cache busting. */
export async function packHash(files: Map<string, Uint8Array>) {
  const encoder = new TextEncoder();
  const parts: Uint8Array[] = [];
  for (const name of [...files.keys()].sort()) parts.push(encoder.encode(`${name}\0${files.get(name)!.length}\0`), files.get(name)!);
  const all = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    all.set(part, at);
    at += part.length;
  }
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', all));
  return Array.from(digest.subarray(0, 6), (byte) => byte.toString(16).padStart(2, '0')).join('');
}
