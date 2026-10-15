import { test, expect, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

// One-handed phone layout: status-only top bar, thread / view / More controls in a
// bottom dock, bottom sheets for threads and the actions the top bar used to hold.
test.use({ actionTimeout: 15_000 });
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

async function fixture(request: import('@playwright/test').APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(path.join(absPath, 'docs'), { recursive: true });
  await writeFile(path.join(absPath, 'docs/notes.md'), '# Notes');
  const workspace = await (await request.post(base + '/api/workspaces', { data: { absPath, label: 'Phone dock' } })).json();
  const ids: string[] = [];
  for (const title of ['Dock first', 'Dock second']) {
    const response = await request.post(base + '/api/threads/start', { data: { workspaceId: workspace.id, title, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
    expect(response.ok()).toBeTruthy();
    const value = await response.json();
    ids.push(value.id ?? value.thread.id);
  }
  return ids;
}

async function swipe(page: Page, selector: string, dx: number) {
  const box = (await page.locator(selector).boundingBox())!;
  const y = box.y + box.height / 2;
  const start = dx < 0 ? box.x + box.width - 16 : box.x + 16;
  const cdp = await page.context().newCDPSession(page);
  const touch = (type: string, x: number) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x, y }] });
  await touch('touchStart', start);
  for (let i = 1; i <= 8; i++) await touch('touchMove', start + (dx * i) / 8);
  await touch('touchEnd', start + dx);
}

test.beforeEach(async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Phone layout');
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
});

test('phone dock replaces the top bar controls and stays reachable', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const [first, second] = await fixture(request);
  await page.goto(`/threads/${first}`);
  const topbar = page.locator('header.matter-topbar');
  const dock = page.getByRole('navigation', { name: 'Thread navigation' });
  await expect(dock).toBeVisible();
  for (const label of ['Chat', 'Terminal', 'Toggle Explorer', 'Open settings', 'Toggle shortcuts sidebar']) {
    await expect(topbar.getByRole('button', { name: label, exact: true })).toHaveCount(0);
  }
  await expect(topbar.getByRole('button', { name: 'Notifications', exact: true })).toBeVisible();
  await expect(page.locator('.matter-tabs-row')).toHaveCount(0);

  // Every dock target is thumb-sized and the dock sits at the bottom edge.
  const boxes = await dock.locator('button').evaluateAll(buttons => buttons.map(b => b.getBoundingClientRect().toJSON() as DOMRect));
  for (const box of boxes) expect(Math.min(box.width, box.height)).toBeGreaterThanOrEqual(40);
  expect(Math.max(...boxes.map(box => box.bottom))).toBeGreaterThan(844 - 60);

  // Files and back to chat from the dock; the ask bar sits above the dock.
  await dock.getByRole('button', { name: 'Toggle Explorer', exact: true }).tap();
  const tree = page.getByTestId('workspace-panel').getByRole('tree', { name: 'Workspace files' });
  await expect(tree).toBeVisible();
  const ask = (await page.locator('form.file-ask-bar').boundingBox())!;
  const dockBox = (await dock.boundingBox())!;
  expect(ask.y + ask.height).toBeLessThanOrEqual(dockBox.y + 1);
  await dock.getByRole('button', { name: 'Chat', exact: true }).tap();
  await expect(tree).toBeHidden();

  // The keyboard gets the room while typing.
  await page.getByRole('textbox', { name: 'Prompt', exact: true }).tap();
  await expect(dock).toBeHidden();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await expect(dock).toBeVisible();

  // Thread sheet from the thread button.
  await dock.locator('.matter-phone-thread').tap();
  const sheet = page.getByRole('dialog', { name: 'Threads' });
  await expect(sheet).toBeVisible();
  await sheet.getByRole('link', { name: /Dock second/ }).tap();
  await expect(page).toHaveURL(new RegExp(`/threads/${second}`));
  await expect(sheet).toBeHidden();
  await expect(dock.locator('.matter-phone-thread-title')).toHaveText('Dock second');

  // Settings now lives in the More sheet.
  await dock.getByRole('button', { name: 'More actions', exact: true }).tap();
  const more = page.getByRole('dialog', { name: 'More actions' });
  await more.getByRole('button', { name: 'Open settings', exact: true }).tap();
  await expect(page.getByRole('dialog', { name: /Settings/ })).toBeVisible();
  await expect(more).toBeHidden();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: /Settings/ })).toBeHidden();

  // Sideways swipe on the thread button returns to the neighbouring thread.
  await swipe(page, '.matter-phone-thread', 120);
  await expect(page).toHaveURL(new RegExp(`/threads/${first}`));
  await expect(dock.locator('.matter-phone-thread-title')).toHaveText('Dock first');
});

test('phone dock can be switched off to restore the old top bar', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const [first] = await fixture(request);
  await page.addInitScript(() => localStorage.setItem('pockymoe.phoneDock', 'off'));
  await page.goto(`/threads/${first}`);
  await expect(page.locator('.matter-topbar').getByRole('button', { name: 'Toggle Explorer', exact: true })).toBeVisible();
  await expect(page.locator('.matter-phone-dock')).toBeHidden();
});
