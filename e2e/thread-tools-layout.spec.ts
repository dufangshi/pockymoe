import { test, expect, type Locator } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

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

test('thread tools open as a row and leave the file panel controls reachable', async ({ page, request }, testInfo) => {
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
  const bar = (await tools.boundingBox())!;
  expect(bar.y).toBeGreaterThanOrEqual(tabs!.y + tabs!.height - 1);

  if (mobile) {
    await page.getByRole('button', { name: 'Toggle shortcuts sidebar', exact: true }).click();
    await page.getByRole('button', { name: 'Open Explorer', exact: true }).click();
  } else {
    await page.getByRole('navigation', { name: 'Workspace tools' }).getByRole('button', { name: 'Toggle Explorer' }).click();
  }
  await page.getByRole('treeitem', { name: 'notes.txt', exact: true }).click();
  await expectUncovered(page.getByRole('button', { name: 'File actions' }).first());
  await page.screenshot({ path: testInfo.outputPath('tools-and-file.png') });
  await expectUncovered(tools.getByRole('button').first());
});
