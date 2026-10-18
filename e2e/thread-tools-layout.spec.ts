import { test, expect, type Locator } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

test.beforeEach(async ({ page }) => {
  // This spec exercises the classic phone bars/tabs and composer split controls.
  await page.addInitScript(() => localStorage.setItem('pockymoe.phoneDock', 'off'));
});

test.use({ actionTimeout: 15_000 });

// The element at a control's center must be the control itself, not an overlay.
async function expectUncovered(control: Locator) {
  await expect(control).toBeVisible();
  expect(await control.evaluate(node => {
    const r = node.getBoundingClientRect();
    const top = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
    return !!top && (top === node || node.contains(top));
  })).toBe(true);
}

for (const preset of ['classic', 'plum-pocket'] as const) test(`thread tools open as a dropdown and leave the file panel controls reachable (${preset})`, async ({ page, request }, testInfo) => {
  await page.addInitScript(value => localStorage.setItem('pockymoe-theme-preset', value), preset);
  const mobile = testInfo.project.name === 'mobile-chromium';
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  await writeFile(path.join(absPath, 'notes.txt'), 'hello\n');
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Tools layout' } })).json();
  const thread = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Tools layout', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  await page.goto(`/threads/${thread.id ?? thread.thread.id}`);
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();

  await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
  const tools = page.locator('#matter-thread-tools');
  await expect(tools).toBeVisible();
  const tabs = await page.locator('.matter-thread-tabs').boundingBox();
  const menu = (await tools.boundingBox())!;
  expect(menu.y).toBeGreaterThanOrEqual(tabs!.y + tabs!.height - 1);
  expect(menu.x + menu.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
  // A dropdown over the page: the workspace path heads it, every action is named.
  await expect(tools.locator('.matter-thread-tools-path')).toContainText('/');
  await expect(tools.getByRole('button', { name: 'Download transcript', exact: true })).toBeVisible();
  await expectUncovered(tools.getByRole('button', { name: 'Download transcript', exact: true }));
  // Esc closes it and returns to its toggle.
  await page.keyboard.press('Escape');
  await expect(tools).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Thread tools', exact: true })).toBeFocused();
  await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
  await expect(tools).toBeVisible();

  if (mobile && preset === 'plum-pocket') {
    await page.locator('header.matter-topbar').getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  } else if (mobile) {
    await page.getByRole('button', { name: 'Toggle shortcuts sidebar', exact: true }).click();
    await page.getByRole('button', { name: 'Open Explorer', exact: true }).click();
  } else {
    await page.getByRole('navigation', { name: 'Workspace tools' }).getByRole('button', { name: 'Toggle Explorer' }).click();
  }
  await page.getByRole('treeitem', { name: 'notes.txt', exact: true }).click();
  await expectUncovered(page.getByRole('button', { name: 'File actions' }).first());
  await page.screenshot({ path: testInfo.outputPath(`tools-and-file-${preset}.png`) });
  // Pressing outside closed the dropdown.
  await expect(tools).toHaveCount(0);
});
