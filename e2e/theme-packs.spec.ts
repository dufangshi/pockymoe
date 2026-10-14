import { readFile } from 'node:fs/promises';
import { expect, test, type Locator, type Page } from '@playwright/test';

import { readZip, writeZip, type ZipEntry } from '../apps/supervisor-web/src/lib/themePacks/zip';

// Theme packs in Settings → Appearance: export the built-in Plum Pocket pack,
// import a modified copy, apply it across a reload, remove it, and refuse packs
// that reach the network or escape their folder.

test.use({ actionTimeout: 15_000 });

const encoder = new TextEncoder();
const decoder = new TextDecoder();

async function openAppearance(page: Page) {
  await page.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
  await page.goto('/workspaces');
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
  await page.getByRole('button', { name: 'Open Navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(dialog).toBeVisible();
  return dialog;
}

function themes(dialog: Locator) {
  return dialog.getByRole('radiogroup', { name: 'Theme' });
}

async function importPack(page: Page, dialog: Locator, name: string, entries: ZipEntry[]) {
  const buffer = Buffer.from(await writeZip(entries));
  await dialog.getByTestId('theme-pack-file').setInputFiles({ name, mimeType: 'application/zip', buffer });
}

function edit(entries: ZipEntry[], name: string, change: (text: string) => string): ZipEntry[] {
  return entries.map((entry) => (entry.name === name ? { name, bytes: encoder.encode(change(decoder.decode(entry.bytes))) } : entry));
}

test('exports Plum Pocket, imports a modified copy, applies it across reloads and removes it', async ({ page }) => {
  const root = page.locator('html');
  let dialog = await openAppearance(page);
  await expect(themes(dialog).getByRole('radio', { name: /^Classic/ })).toBeChecked();
  await expect(themes(dialog).getByRole('button', { name: /^Remove/ })).toHaveCount(0);

  const [download] = await Promise.all([page.waitForEvent('download'), dialog.getByRole('button', { name: 'Export Plum Pocket' }).click()]);
  expect(download.suggestedFilename()).toBe('plum-pocket.pockytheme');
  const original = await readZip(new Uint8Array(await readFile((await download.path())!)));
  expect(original[0]!.name).toBe('theme.json');
  expect(original.map((entry) => entry.name)).toEqual(expect.arrayContaining(['styles/tokens.css', 'assets/agents/claude.webp', 'assets/fonts/nunito-latin.woff2']));

  let copy = edit(original, 'theme.json', (text) => JSON.stringify({ ...JSON.parse(text), id: 'plum-copy', name: 'Plum Copy', version: '1.0.1', author: 'E2E' }));
  copy = edit(copy, 'styles/controls.css', (text) => `${text}\n:root:root:root:theme { --theme-accent-solid: #ff00aa; }\n`);
  await importPack(page, dialog, 'plum-copy.pockytheme', copy);
  await expect(dialog.locator('.theme-preset-status')).toHaveText('Imported Plum Copy');
  const card = themes(dialog).locator('.theme-preset-card').filter({ hasText: 'Plum Copy' });
  await expect(card).toContainText('E2E · 1.0.1');

  await card.locator('label').click();
  await expect(root).toHaveAttribute('data-theme-preset', 'plum-copy');
  await expect(root).toHaveAttribute('data-theme-features', /workbench\.phoneDrawerTools.*code\.cssSyntaxPalette/);
  const token = () => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--theme-accent-solid').trim());
  expect(await token()).toBe('#ff00aa');
  // The pack's own fonts load from the imported files.
  await expect.poll(() => page.evaluate(async () => {
    await document.fonts.ready;
    return [...document.fonts].some((font) => font.family.includes('plum-copy--Nunito') && font.status === 'loaded');
  })).toBe(true);

  await page.reload();
  await expect(root).toHaveAttribute('data-theme-preset', 'plum-copy');
  expect(await token()).toBe('#ff00aa');
  expect(await page.locator('style[data-theme-pack="plum-copy"]').evaluate((style) => style.textContent!.includes('blob:'))).toBe(true);

  dialog = await openAppearance(page);
  await expect(themes(dialog).getByRole('radio', { name: /^Plum Copy/ })).toBeChecked();
  await themes(dialog).getByRole('button', { name: 'Remove Plum Copy' }).click();
  await expect(card).toHaveCount(0);
  await expect(root).toHaveAttribute('data-theme-preset', 'classic');
  await expect(themes(dialog).getByRole('radio', { name: /^Classic/ })).toBeChecked();
  await page.reload();
  await expect(root).toHaveAttribute('data-theme-preset', 'classic');
});

test('refuses packs that load remote files, import stylesheets or escape their folder', async ({ page }) => {
  const dialog = await openAppearance(page);
  const manifest = {
    schema: 1,
    id: 'evil-pack',
    name: 'Evil',
    version: '1.0.0',
    styles: ['styles/main.css'],
  };
  const pack = (css: string, extra: ZipEntry[] = []): ZipEntry[] => [
    { name: 'theme.json', bytes: encoder.encode(JSON.stringify(manifest)) },
    { name: 'styles/main.css', bytes: encoder.encode(css) },
    ...extra,
  ];
  const cases: [string, ZipEntry[], RegExp][] = [
    ['remote.pockytheme', pack(':theme { background: url(https://evil.example/track.png); }'), /a stylesheet uses something themes may not \(styles\/main\.css:1: url\(https:\/\/evil\.example\/track\.png\) must use asset: or data:\)/],
    ['import.pockytheme', pack('@import url("https://evil.example/x.css");\n:theme { color: red; }'), /\(styles\/main\.css:1: @import is not allowed\)/],
    ['traversal.pockytheme', pack(':theme { color: red; }', [{ name: '../../outside.css', bytes: encoder.encode(':theme{}') }]), /the theme contains an unsafe file path \(\.\.\/\.\.\/outside\.css: parent directory\)/],
  ];
  for (const [name, entries, message] of cases) {
    await importPack(page, dialog, name, entries);
    const alert = dialog.getByRole('alert');
    await expect(alert).toHaveText(message);
    expect(await alert.evaluate((node) => node.scrollHeight <= node.clientHeight + 1)).toBe(true);
  }
  await expect(themes(dialog).locator('.theme-preset-card')).toHaveCount(2);
  await expect(page.locator('html')).toHaveAttribute('data-theme-preset', 'classic');
});

test('a light-only pack shows light while the app is dark, and Classic brings dark back', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('remote-codex-theme-mode', 'dark'));
  const root = page.locator('html');
  const meta = page.locator('meta[name="theme-color"]');
  const dialog = await openAppearance(page);
  await expect(root).toHaveAttribute('data-theme-effective', 'dark');
  const modes = dialog.locator('input[name="settings-theme"]');
  await expect(modes).toHaveCount(3);
  for (const input of await modes.all()) await expect(input).toBeEnabled();

  const manifest = { schema: 1, id: 'light-only', name: 'Light Only', version: '1.0.0', modes: ['light'], chrome: { light: '#fdf2f6' }, styles: ['styles/main.css'] };
  await importPack(page, dialog, 'light-only.pockytheme', [
    { name: 'theme.json', bytes: encoder.encode(JSON.stringify(manifest)) },
    { name: 'styles/main.css', bytes: encoder.encode(':root:root:theme { --theme-accent-solid: #d6417a; }') },
  ]);
  const card = themes(dialog).locator('.theme-preset-card').filter({ hasText: 'Light Only' });
  await expect(card.getByRole('img', { name: 'Light only' })).toBeVisible();
  await card.locator('label').click();
  await expect(root).toHaveAttribute('data-theme-preset', 'light-only');
  await expect(root).toHaveAttribute('data-theme-effective', 'light');
  expect(await root.evaluate((node) => getComputedStyle(node).colorScheme)).toBe('light');
  await expect(meta).toHaveAttribute('content', '#fdf2f6');
  await expect(page.locator('.thread-ui-shell[data-theme-effective="dark"]')).toHaveCount(0);
  // The color mode is fixed while the pack is applied; the stored choice stays dark.
  await expect(dialog.getByTestId('theme-mode-fixed')).toHaveText('Light only');
  for (const input of await modes.all()) await expect(input).toBeDisabled();
  await expect(dialog.locator('input[name="settings-theme"][value="light"]')).toBeChecked();
  expect(await page.evaluate(() => localStorage.getItem('remote-codex-theme-mode'))).toBe('dark');

  await themes(dialog).locator('.theme-preset-card').filter({ hasText: 'Classic' }).locator('label').click();
  await expect(root).toHaveAttribute('data-theme-preset', 'classic');
  await expect(root).toHaveAttribute('data-theme-effective', 'dark');
  await expect(meta).toHaveAttribute('content', '#171713');
  await expect(dialog.getByTestId('theme-mode-fixed')).toHaveCount(0);
  await expect(dialog.locator('input[name="settings-theme"][value="dark"]')).toBeChecked();
  for (const input of await modes.all()) await expect(input).toBeEnabled();
});
