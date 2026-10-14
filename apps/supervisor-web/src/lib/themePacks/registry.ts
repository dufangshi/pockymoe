import { appUrl } from '../webBase';
/* The theme registry: Classic, the built-in packs and the packs imported into this
   browser. Applying a pack fills one `<style data-theme-pack>` (created at startup,
   after the app's own stylesheets and before lazily loaded ones, so the cascade
   order never depends on timing) and then sets `<html data-theme-preset>` and
   `data-theme-features`. The choice is stored per browser like before. */

import { useSyncExternalStore } from 'react';
import builtinPacks from 'virtual:pockymoe-theme-packs';

import { ThemePackError, packError } from './errors';
import { isFontPath, packFileType } from './limits';
import { type ThemeManifest, type ThemeMode } from './manifest';
import { compileThemePack, packHash, readThemePackArchive, validatePackFiles, writeThemePackArchive, type ThemePackFiles } from './pack';
import { indexedDbThemePackStorage, memoryThemePackStorage, type StoredPackInfo, type ThemePackStorage } from './storage';
import type { BuiltinThemePack } from './types';

export const THEME_PRESET_KEY = 'pockymoe-theme-preset';
export const CLASSIC_THEME_ID = 'classic';
const CHANNEL = 'pockymoe-theme-packs';

export type ThemeSource = 'classic' | 'builtin' | 'imported';

export interface ThemeEntry {
  id: string;
  source: ThemeSource;
  manifest: ThemeManifest;
}

export interface ThemeRegistryState {
  entries: ThemeEntry[];
  /** The stored choice. */
  selected: string;
  /** The theme on the page: the choice, or Classic while it loads or when it failed. */
  applied: string;
  /** The color mode the applied pack fixes (it supports only that one), or null. */
  fixedMode: ThemeMode | null;
}

const CLASSIC: ThemeEntry = {
  id: CLASSIC_THEME_ID,
  source: 'classic',
  manifest: {
    schema: 1,
    id: CLASSIC_THEME_ID,
    name: 'Classic',
    version: '',
    author: 'Pockymoe',
    modes: ['light', 'dark'],
    swatches: ['#e2e7e2', '#0f1317', '#008b53', '#a2afb9'],
    chrome: { light: '#f3f6f7', dark: '#171713' },
    styles: [],
    fonts: [],
    agents: {},
    features: [],
  },
};

let builtins: BuiltinThemePack[] = builtinPacks;
let storage: ThemePackStorage = typeof indexedDB === 'undefined' ? memoryThemePackStorage() : indexedDbThemePackStorage;
let imported: StoredPackInfo[] = [];
let state: ThemeRegistryState = { entries: entriesFor([]), selected: readStoredThemeId(), applied: CLASSIC_THEME_ID, fixedMode: null };
const listeners = new Set<() => void>();
let styleElement: HTMLStyleElement | null = null;
/** The applied pack's manifest (the registry's entries may still be loading). */
let appliedManifest: ThemeManifest | null = null;
let applySequence = 0;
let channel: BroadcastChannel | null = null;
const builtinCss = new Map<string, Promise<string>>();
/** The compiled stylesheet of the applied imported pack and its blob URLs. */
let importedCache: { id: string; hash: string; css: string; urls: string[] } | null = null;

function entriesFor(list: StoredPackInfo[]): ThemeEntry[] {
  return [
    CLASSIC,
    ...builtins.map((pack): ThemeEntry => ({ id: pack.id, source: 'builtin', manifest: pack.manifest })),
    ...list
      .filter((pack) => !builtins.some((builtin) => builtin.id === pack.id))
      .sort((a, b) => a.importedAt - b.importedAt)
      .map((pack): ThemeEntry => ({ id: pack.id, source: 'imported', manifest: pack.manifest })),
  ];
}

function update(patch: Partial<ThemeRegistryState>) {
  state = { ...state, ...patch };
  for (const listener of listeners) listener();
}

export function readStoredThemeId() {
  try {
    return localStorage.getItem(THEME_PRESET_KEY) || CLASSIC_THEME_ID;
  } catch {
    return CLASSIC_THEME_ID;
  }
}

export function getThemeRegistryState() {
  return state;
}

export function subscribeThemeRegistry(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useThemeRegistry() {
  return useSyncExternalStore(subscribeThemeRegistry, getThemeRegistryState, getThemeRegistryState);
}

export function themeEntry(id: string) {
  return state.entries.find((entry) => entry.id === id);
}

/** The one color mode a pack supports, or null when it supports both. */
export function themeFixedMode(manifest: ThemeManifest | null | undefined): ThemeMode | null {
  return manifest?.modes.length === 1 ? manifest.modes[0]! : null;
}

/** The color mode on the page: a single-mode pack fixes it while it is applied,
 *  otherwise it is the user's preference. */
export function effectiveColorMode(preferred: ThemeMode, current: ThemeRegistryState = state): ThemeMode {
  return current.fixedMode ?? preferred;
}

/** Browser chrome color (`<meta name="theme-color">`) for the applied theme. */
export function themeChromeColor(mode: ThemeMode) {
  const manifest = appliedManifest ?? CLASSIC.manifest;
  return manifest.chrome?.[mode] ?? CLASSIC.manifest.chrome![mode]!;
}

function ensureStyleElement() {
  if (styleElement?.isConnected) return styleElement;
  styleElement = document.querySelector<HTMLStyleElement>('style[data-theme-pack]') ?? document.createElement('style');
  styleElement.dataset.themePack = '';
  if (!styleElement.isConnected) document.head.append(styleElement);
  return styleElement;
}

function setDocumentTheme(id: string, css: string, manifest: ThemeManifest | null) {
  const style = ensureStyleElement();
  // The stylesheet first, so the attributes never point at rules that are not there.
  if (style.textContent !== css) style.textContent = css;
  style.dataset.themePack = manifest ? id : '';
  const root = document.documentElement;
  appliedManifest = manifest;
  root.dataset.themePreset = id;
  root.dataset.themeFeatures = manifest?.features.join(' ') ?? '';
  // A single-mode pack never shows with the other mode's base tokens, not even for
  // the frame before the app re-renders with `effectiveColorMode`.
  const fixed = themeFixedMode(manifest);
  if (fixed) {
    root.dataset.themeEffective = fixed;
    root.style.colorScheme = fixed;
  }
  if (state.applied !== id || state.fixedMode !== fixed) update({ applied: id, fixedMode: fixed });
}

function releaseImportedCache() {
  for (const url of importedCache?.urls ?? []) URL.revokeObjectURL(url);
  importedCache = null;
}

async function fetchText(url: string) {
  const response = await fetch(appUrl(url), { credentials: 'same-origin' });
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  // A missing file can come back as the SPA's index.html.
  if (/text\/html/i.test(response.headers.get('content-type') ?? '')) throw new Error(`${url}: not a stylesheet`);
  return response.text();
}

/** A `data:` URL; fonts use one because the relay's CSP allows `font-src data:`
 *  but not `blob:`. */
function dataUrl(path: string, bytes: Uint8Array) {
  let binary = '';
  for (let at = 0; at < bytes.length; at += 0x8000) binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  return `data:${packFileType(path)};base64,${btoa(binary)}`;
}

async function compiledImported(info: StoredPackInfo) {
  if (importedCache?.id === info.id && importedCache.hash === info.hash) return importedCache.css;
  const files = await storage.files(info.id);
  if (!files) throw new Error(`${info.id}: files are missing`);
  // Validate again: the rules may be stricter than when the pack was imported.
  const pack = validatePackFiles(files);
  const urls = new Map<string, string>();
  const fonts = new Map<string, string>();
  const css = compileThemePack(pack, (path) => {
    if (isFontPath(path)) {
      if (!fonts.has(path)) fonts.set(path, dataUrl(path, files.get(path)!));
      return fonts.get(path)!;
    }
    let url = urls.get(path);
    if (!url) {
      url = URL.createObjectURL(new Blob([files.get(path)! as BlobPart], { type: packFileType(path) }));
      urls.set(path, url);
    }
    return url;
  });
  releaseImportedCache();
  importedCache = { id: info.id, hash: info.hash, css, urls: [...urls.values()] };
  return css;
}

/** Applies a theme; anything unknown or broken falls back to Classic. */
export async function applyTheme(id: string): Promise<void> {
  const sequence = ++applySequence;
  try {
    if (id === CLASSIC_THEME_ID) {
      setDocumentTheme(CLASSIC_THEME_ID, '', null);
      return;
    }
    const builtin = builtins.find((pack) => pack.id === id);
    let css: string;
    let manifest: ThemeManifest;
    if (builtin) {
      if (!builtinCss.has(id)) builtinCss.set(id, fetchText(builtin.css).catch((error) => (builtinCss.delete(id), Promise.reject(error))));
      css = await builtinCss.get(id)!;
      manifest = builtin.manifest;
    } else {
      const info = await storage.info(id);
      if (!info) throw new Error(`theme "${id}" is not installed`);
      css = await compiledImported(info);
      manifest = info.manifest;
    }
    if (sequence !== applySequence) return;
    setDocumentTheme(id, css, manifest);
  } catch (error) {
    if (sequence !== applySequence) return;
    console.warn('[theme packs] falling back to Classic:', error);
    setDocumentTheme(CLASSIC_THEME_ID, '', null);
  }
  // The previous imported pack's blob URLs are no longer referenced.
  if (importedCache && importedCache.id !== state.applied) releaseImportedCache();
}

async function refreshImported() {
  try {
    imported = await storage.list();
  } catch (error) {
    console.warn('[theme packs] imported packs are unavailable:', error);
    imported = [];
  }
  update({ entries: entriesFor(imported) });
}

function broadcast() {
  channel?.postMessage({ type: 'changed' });
}

/** Selects and applies a theme for this browser. */
export async function selectTheme(id: string) {
  try {
    localStorage.setItem(THEME_PRESET_KEY, id);
  } catch {
    /* The current page still switches. */
  }
  update({ selected: id });
  await applyTheme(id);
}

async function blobBytes(blob: Blob) {
  if (typeof blob.arrayBuffer === 'function') return new Uint8Array(await blob.arrayBuffer());
  return new Promise<Uint8Array>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer));
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(blob);
  });
}

const previewUrls = new Map<string, string>();

/** URL of a pack's card preview image, if it has one. */
export async function themePreviewUrl(id: string): Promise<string | null> {
  const entry = themeEntry(id);
  const preview = entry?.manifest.preview;
  if (!entry || !preview) return null;
  const builtin = builtins.find((pack) => pack.id === id);
  if (builtin) return `${builtin.base}${preview}?v=${builtin.hash}`;
  const info = imported.find((pack) => pack.id === id);
  if (!info) return null;
  const key = `${id}:${info.hash}`;
  if (!previewUrls.has(key)) {
    const bytes = (await storage.files(id))?.get(preview);
    if (!bytes) return null;
    previewUrls.set(key, URL.createObjectURL(new Blob([bytes as BlobPart], { type: packFileType(preview) })));
  }
  return previewUrls.get(key)!;
}

/** Validates and stores a `.pockytheme` file. A pack with the id of an imported
 *  pack replaces it; built-in ids are reserved. */
export async function importThemePack(file: Blob): Promise<ThemeManifest> {
  const pack = await readThemePackArchive(await blobBytes(file));
  const { id } = pack.manifest;
  if (builtins.some((builtin) => builtin.id === id)) packError('reserved', `"${id}" is a built-in theme`);
  compileThemePack(pack, () => 'about:invalid');
  const info: StoredPackInfo = { id, manifest: pack.manifest, hash: await packHash(pack.files), importedAt: (await storage.info(id))?.importedAt ?? Date.now() };
  try {
    await storage.put(info, pack.files);
  } catch (error) {
    throw new ThemePackError('storage', error instanceof Error ? error.message : String(error));
  }
  await refreshImported();
  broadcast();
  if (state.applied === id || state.selected === id) await applyTheme(id);
  return pack.manifest;
}

/** Removes an imported pack; Classic takes over when it was selected. */
export async function removeThemePack(id: string) {
  if (themeEntry(id)?.source !== 'imported') throw new Error(`${id} is not an imported theme`);
  await storage.remove(id);
  if (importedCache?.id === id) releaseImportedCache();
  await refreshImported();
  broadcast();
  if (state.selected === id) await selectTheme(CLASSIC_THEME_ID);
  else if (state.applied === id) await applyTheme(CLASSIC_THEME_ID);
}

async function packFiles(id: string): Promise<ThemePackFiles> {
  const builtin = builtins.find((pack) => pack.id === id);
  if (builtin) {
    const files = new Map<string, Uint8Array>();
    await Promise.all(
      builtin.files.map(async (path) => {
        const response = await fetch(appUrl(`${builtin.base}${path}?v=${builtin.hash}`), { credentials: 'same-origin' });
        if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
        files.set(path, new Uint8Array(await response.arrayBuffer()));
      }),
    );
    return validatePackFiles(builtin.files.map((path) => [path, files.get(path)!]));
  }
  const files = await storage.files(id);
  if (!files) throw new Error(`theme "${id}" is not installed`);
  return validatePackFiles(files);
}

/** The pack as a `.pockytheme` file, built-in packs included. */
export async function exportThemePack(id: string): Promise<{ fileName: string; blob: Blob }> {
  const pack = await packFiles(id);
  const bytes = await writeThemePackArchive(pack);
  return { fileName: `${id}.pockytheme`, blob: new Blob([bytes as BlobPart], { type: 'application/zip' }) };
}

/** Applies the stored theme before the first render. Resolves when it is applied
 *  or after `timeoutMs`; a slow pack still applies when it arrives. */
export function initializeThemePacks({ timeoutMs = 1500 } = {}) {
  ensureStyleElement();
  if (!document.documentElement.dataset.themePreset) setDocumentTheme(CLASSIC_THEME_ID, '', null);
  window.addEventListener('storage', (event) => {
    if (event.key !== THEME_PRESET_KEY && event.key !== null) return;
    const id = readStoredThemeId();
    update({ selected: id });
    void applyTheme(id);
  });
  if (typeof BroadcastChannel === 'function') {
    channel = new BroadcastChannel(CHANNEL);
    channel.onmessage = () => {
      importedCache = importedCache && { ...importedCache, hash: '' };
      void refreshImported().then(() => applyTheme(state.selected));
    };
  }
  const selected = readStoredThemeId();
  update({ selected });
  const ready = Promise.all([refreshImported(), applyTheme(selected)]).then(() => undefined);
  return Promise.race([ready, new Promise<void>((resolve) => setTimeout(resolve, timeoutMs))]);
}

/** Test hooks. */
export function configureThemeRegistryForTests(options: { storage?: ThemePackStorage; builtins?: BuiltinThemePack[] }) {
  if (options.storage) storage = options.storage;
  if (options.builtins) builtins = options.builtins;
  builtinCss.clear();
  releaseImportedCache();
  imported = [];
  appliedManifest = null;
  applySequence += 1;
  state = { entries: entriesFor([]), selected: readStoredThemeId(), applied: CLASSIC_THEME_ID, fixedMode: null };
  return refreshImported();
}
