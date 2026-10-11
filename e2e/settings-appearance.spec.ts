import { test, expect, type Locator, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

test.use({ actionTimeout: 15_000 });
// A fresh browser shows the guided tour's welcome card, which covers the navigation.
async function dismissTour(page: Page) {
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
}
async function geometry(dialog: Locator) {
  return dialog.evaluate(node => {
    const r = node.getBoundingClientRect(), s = getComputedStyle(node);
    const panel = node.querySelector('.settings-panel')!.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height, panelX: panel.x, panelY: panel.y, font: s.fontFamily, padding: s.padding, transform: s.transform, translate: s.translate };
  });
}

test('settings retain centered geometry through theme switches and share their shell inside and outside conversations', async ({ page, request }, testInfo) => {
  const mobile = testInfo.project.name === 'mobile-chromium';
  await page.setViewportSize(mobile ? { width: 390, height: 844 } : { width: 1440, height: 1000 });
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(base + '/api/workspaces', { data: { absPath, label: 'Appearance' } })).json();
  const thread = await (await request.post(base + '/api/threads/start', { data: { workspaceId: workspace.id, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  await page.goto('/workspaces');
  await dismissTour(page);
  await page.getByRole('button', { name: 'Open Navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  let dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(dialog).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
  await expect(dialog).toHaveCSS('opacity', '1');
  const outside = await geometry(dialog);
  for (const mode of ['Light', 'Dark', 'Light']) {
    await dialog.getByRole('radio', { name: mode, exact: true }).locator('..').click();
    await expect(dialog).toHaveAttribute('data-theme-effective', mode.toLowerCase());
    await expect.poll(async () => Math.abs((await geometry(dialog)).x - outside.x)).toBeLessThan(1);
    await expect.poll(async () => Math.abs((await geometry(dialog)).y - outside.y)).toBeLessThan(1);
  }
  await page.keyboard.press('Escape');
  await page.goto(`/threads/${thread.id}`);
  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveCSS('opacity', '1');
  const inside = await geometry(dialog);
  expect(inside, JSON.stringify({ outside, inside })).toEqual(outside);
  for (const mode of ['Dark', 'Light']) {
    await dialog.getByRole('radio', { name: mode, exact: true }).locator('..').click();
    await expect(dialog).toHaveAttribute('data-theme-effective', mode.toLowerCase());
    await expect.poll(async () => Math.abs((await geometry(dialog)).x - inside.x)).toBeLessThan(1);
    await expect.poll(async () => Math.abs((await geometry(dialog)).y - inside.y)).toBeLessThan(1);
  }
  await page.screenshot({ path: testInfo.outputPath('shared-settings.png') });
});

test('theme presets restyle the app, combine with color modes and persist across reloads', async ({ page, request }, testInfo) => {
  const mobile = testInfo.project.name === 'mobile-chromium';
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(base + '/api/workspaces', { data: { absPath, label: 'Presets' } })).json();
  const thread = await (await request.post(base + '/api/threads/start', { data: { workspaceId: workspace.id, title: 'Preset thread', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const root = page.locator('html');
  const token = (name: string) => page.evaluate(n => getComputedStyle(document.documentElement).getPropertyValue(n).trim(), name);
  await page.goto('/workspaces');
  await expect(root).toHaveAttribute('data-theme-preset', 'classic');
  await dismissTour(page);
  await page.getByRole('button', { name: 'Open Navigation' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  await dialog.getByRole('radio', { name: 'Light', exact: true }).locator('..').click();
  const presets = dialog.getByRole('radiogroup', { name: 'Theme' });
  await expect(presets.getByRole('radio', { name: /^Classic/ })).toBeChecked();
  await presets.locator('label').filter({ hasText: 'Plum Pocket' }).click();
  await expect(root).toHaveAttribute('data-theme-preset', 'plum-pocket');
  await expect(presets.getByRole('radio', { name: /^Plum Pocket/ })).toBeChecked();
  expect(await token('--theme-accent-solid')).toBe('#e8a93a');
  await dialog.getByRole('radio', { name: 'Dark', exact: true }).locator('..').click();
  await expect(dialog).toHaveAttribute('data-theme-effective', 'dark');
  expect(await token('--theme-accent-solid')).toBe('#f0b54a');
  await page.keyboard.press('Escape');

  await page.goto(`/threads/${thread.id}`);
  await expect(root).toHaveAttribute('data-theme-preset', 'plum-pocket');
  await expect(root).toHaveAttribute('data-theme-effective', 'dark');
  const composer = page.locator('.matter-workbench .thread-graph-composer-shell');
  // A solid felt edge with a stitched seam inside it.
  await expect(composer).toHaveCSS('border-top-style', 'solid');
  await expect.poll(() => composer.evaluate((node) => getComputedStyle(node, '::before').borderTopStyle)).toBe('dashed');
  if (!mobile) {
    const avatar = page.locator('.matter-thread-row .matter-thread-avatar[data-agent="codex"]').first();
    await expect(avatar).toBeVisible();
    await expect(avatar).toHaveCSS('background-image', /codex\.webp/);
  }
  await page.screenshot({ path: testInfo.outputPath('plum-pocket-dark.png') });

  await page.getByRole('button', { name: 'Open settings', exact: true }).click();
  const inside = page.getByRole('dialog', { name: 'Settings', exact: true });
  await inside.getByRole('radiogroup', { name: 'Theme' }).locator('label').filter({ hasText: 'Classic' }).click();
  await expect(root).toHaveAttribute('data-theme-preset', 'classic');
  await expect(root).toHaveAttribute('data-theme-effective', 'dark');
  await page.keyboard.press('Escape');
  await expect(composer).toHaveCSS('border-top-style', 'solid');
  for (const avatar of await page.locator('.matter-thread-avatar').all()) await expect(avatar).toBeHidden();
});
