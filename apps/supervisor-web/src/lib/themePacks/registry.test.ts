import { THEME_AGENT_KEY_PATTERN } from '@pockymoe/thread-ui';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ThemePackError } from './errors';
import { AGENT_KEY_PATTERN, validateManifest } from './manifest';
import { readThemePackArchive, validatePackFiles, writeThemePackArchive } from './pack';
import {
  CLASSIC_THEME_ID,
  THEME_PRESET_KEY,
  applyTheme,
  configureThemeRegistryForTests,
  exportThemePack,
  getThemeRegistryState,
  importThemePack,
  removeThemePack,
  selectTheme,
  themeChromeColor,
} from './registry';
import { memoryThemePackStorage } from './storage';
import { testManifest, testPackFiles } from './testPack';
import type { BuiltinThemePack } from './types';
import { writeZip } from './zip';

const BUILTIN_CSS = ':root[data-theme-preset="builtin-demo"] { --theme-accent-solid: #abcdef; }';
const builtinFiles = testPackFiles({ manifest: testManifest({ id: 'builtin-demo' }) });
const builtin: BuiltinThemePack = {
  id: 'builtin-demo',
  hash: 'abc123',
  manifest: validateManifest(testManifest({ id: 'builtin-demo', features: ['transcript.agentByline'] })),
  files: builtinFiles.map(([name]) => name),
  base: '/themes/builtin-demo/',
  css: '/themes/builtin-demo.css?v=abc123',
};

const html = document.documentElement;
const packStyle = () => document.querySelector<HTMLStyleElement>('style[data-theme-pack]');

async function packFile(manifest = testManifest()) {
  const bytes = await writeZip(testPackFiles({ manifest }).map(([name, data]) => ({ name, bytes: data })));
  return new Blob([bytes as BlobPart], { type: 'application/zip' });
}

function blobBytes(blob: Blob) {
  return new Promise<Uint8Array>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer));
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(blob);
  });
}

function serve(routes: Record<string, { body: string | Uint8Array; type: string }>) {
  const fetch = vi.fn(async (url: string) => {
    const route = routes[url];
    if (!route) return new Response('missing', { status: 404 });
    return new Response(route.body as BodyInit, { headers: { 'content-type': route.type } });
  });
  vi.stubGlobal('fetch', fetch);
  return fetch;
}

beforeEach(async () => {
  localStorage.clear();
  delete html.dataset.themePreset;
  delete html.dataset.themeFeatures;
  packStyle()?.remove();
  vi.spyOn(console, 'warn').mockImplementation(() => undefined);
  await configureThemeRegistryForTests({ storage: memoryThemePackStorage(), builtins: [builtin] });
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('theme registry', () => {
  it('lists Classic first, then built-in packs', () => {
    expect(getThemeRegistryState().entries.map((entry) => [entry.id, entry.source])).toEqual([
      [CLASSIC_THEME_ID, 'classic'],
      ['builtin-demo', 'builtin'],
    ]);
  });

  it('applies a built-in pack: stylesheet, preset, feature flags and chrome color', async () => {
    serve({ [builtin.css]: { body: BUILTIN_CSS, type: 'text/css; charset=utf-8' } });
    await selectTheme('builtin-demo');
    expect(localStorage.getItem(THEME_PRESET_KEY)).toBe('builtin-demo');
    expect(packStyle()?.textContent).toBe(BUILTIN_CSS);
    expect(packStyle()?.dataset.themePack).toBe('builtin-demo');
    expect(html.dataset.themePreset).toBe('builtin-demo');
    expect(html.dataset.themeFeatures).toBe('transcript.agentByline');
    expect(getThemeRegistryState()).toMatchObject({ selected: 'builtin-demo', applied: 'builtin-demo' });
    expect(themeChromeColor('light')).toBe('#112233');

    await selectTheme(CLASSIC_THEME_ID);
    expect(packStyle()?.textContent).toBe('');
    expect(html.dataset.themePreset).toBe('classic');
    expect(html.dataset.themeFeatures).toBe('');
    expect(themeChromeColor('dark')).toBe('#171713');
  });

  it('falls back to Classic when a pack is unknown or its stylesheet is missing', async () => {
    await applyTheme('nowhere');
    expect(html.dataset.themePreset).toBe('classic');
    // The SPA fallback answers a missing file with index.html.
    serve({ [builtin.css]: { body: '<!doctype html>', type: 'text/html; charset=utf-8' } });
    await applyTheme('builtin-demo');
    expect(html.dataset.themePreset).toBe('classic');
    expect(getThemeRegistryState().applied).toBe('classic');
  });

  it('imports, applies with local asset URLs, exports and removes a pack', async () => {
    const manifest = await importThemePack(await packFile());
    expect(manifest.id).toBe('test-pack');
    expect(getThemeRegistryState().entries.at(-1)).toMatchObject({ id: 'test-pack', source: 'imported' });

    await selectTheme('test-pack');
    const css = packStyle()!.textContent!;
    expect(html.dataset.themePreset).toBe('test-pack');
    expect(html.dataset.themeFeatures).toBe('composer.richMenus future.flag');
    expect(css).toContain(':root[data-theme-preset="test-pack"] .brand');
    expect(css).toMatch(/url\("blob:[^"]+"\) center \/ cover/);
    // Fonts are data: URLs: the relay's CSP allows fonts from data: but not blob:.
    expect(css).toContain('src: url("data:font/woff2;base64,');
    expect(css).not.toContain('asset:');

    const exported = await exportThemePack('test-pack');
    expect(exported.fileName).toBe('test-pack.pockytheme');
    const roundTrip = await readThemePackArchive(await blobBytes(exported.blob));
    expect(roundTrip.manifest).toEqual(manifest);

    await removeThemePack('test-pack');
    expect(getThemeRegistryState().entries.some((entry) => entry.id === 'test-pack')).toBe(false);
    expect(html.dataset.themePreset).toBe('classic');
    expect(localStorage.getItem(THEME_PRESET_KEY)).toBe('classic');
  });

  it('replaces an imported pack with the same id and re-applies it', async () => {
    await importThemePack(await packFile());
    await selectTheme('test-pack');
    await importThemePack(await packFile(testManifest({ version: '1.1.0', features: [] })));
    expect(getThemeRegistryState().entries.filter((entry) => entry.id === 'test-pack')).toHaveLength(1);
    expect(getThemeRegistryState().entries.at(-1)!.manifest.version).toBe('1.1.0');
    expect(html.dataset.themeFeatures).toBe('');
  });

  it('refuses to import over a built-in pack or Classic and never removes them', async () => {
    await expect(importThemePack(await packFile(testManifest({ id: 'builtin-demo' })))).rejects.toMatchObject({ code: 'reserved' });
    await expect(importThemePack(await packFile(testManifest({ id: 'classic' })))).rejects.toBeInstanceOf(ThemePackError);
    await expect(removeThemePack('builtin-demo')).rejects.toThrow(/not an imported theme/);
    await expect(removeThemePack(CLASSIC_THEME_ID)).rejects.toThrow(/not an imported theme/);
  });

  it('rejects a pack whose CSS reaches the network and stores nothing', async () => {
    const bytes = await writeThemePackArchive(validatePackFiles(testPackFiles({ css: ':theme { background: url(https://evil.example/x.png); }' })));
    await expect(importThemePack(new Blob([bytes as BlobPart]))).rejects.toMatchObject({ code: 'css' });
    expect(getThemeRegistryState().entries.some((entry) => entry.id === 'test-pack')).toBe(false);
  });

  it('exports a built-in pack from its published files', async () => {
    const routes = Object.fromEntries(builtinFiles.map(([name, body]) => [`${builtin.base}${name}?v=${builtin.hash}`, { body, type: 'application/octet-stream' }]));
    serve(routes);
    const exported = await exportThemePack('builtin-demo');
    const pack = await readThemePackArchive(await blobBytes(exported.blob));
    expect(pack.manifest.id).toBe('builtin-demo');
    expect([...pack.files.keys()].sort()).toEqual(builtin.files.slice().sort());
  });

  it('shares the agent key format with the shared UI', () => {
    expect(AGENT_KEY_PATTERN.source).toBe(THEME_AGENT_KEY_PATTERN.source);
  });
});
