import { test, expect, type Locator } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile, readFile } from 'node:fs/promises';
import path from 'node:path';

test.beforeEach(async ({ page }) => {
  // This spec exercises the classic phone bars/tabs and composer split controls.
  await page.addInitScript(() => localStorage.setItem('pockymoe.phoneDock', 'off'));
});

test.use({ actionTimeout: 15_000 });
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
test('file toolbar stays compact, tabs swipe independently and icon actions inherit dark and light themes', async ({ page, request, context, isMobile }, testInfo) => {
  const root = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(root, { recursive: true });
  const names = ['README-文件浏览说明.md', 'architecture-工作区设计.md', 'screenshots-教程图册.md', 'references-参考资料.md', 'review-交互验收记录.md'];
  for (const name of names) await writeFile(path.join(root, name), `# ${name}\n\nSwitch between files without losing their tabs.\n\nThe actions below the tabs inherit this page’s theme.`);
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath: root, label: 'Workbench tabs' } })).json();
  const ids: string[] = [];
  for (let i = 0; i < 6; i++) {
    const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: `Workspace conversation ${i + 1}`, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
    expect(response.ok()).toBeTruthy();
    const started = await response.json(); ids.push(started.id ?? started.thread.id);
  }
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('remote-codex-theme-mode', 'system');
    localStorage.setItem('pockymoe.onboarding.v1:' + JSON.stringify([location.origin, 'local:owner']), JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.goto(`/threads/${ids[0]}`);
  await page.locator(isMobile ? '.matter-topbar' : '.matter-rail').getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  const files = page.getByTestId('workspace-panel');
  const back = files.getByRole('button', { name: 'Back to files', exact: true });
  for (const name of names) {
    await files.getByRole('treeitem', { name, exact: true }).getByRole('button', { name, exact: true }).click();
    await expect(files.getByRole('tab', { name, exact: true })).toHaveAttribute('aria-selected', 'true');
    if (name !== names.at(-1)) await back.click();
  }
  const row = files.locator('.workspace-file-toolbar');
  const strip = files.getByRole('tablist', { name: 'Open workspace files' });
  const edge = files.locator('.workspace-tab-scroll');
  const more = files.getByRole('button', { name: 'File actions', exact: true });
  const shelf = files.getByRole('toolbar', { name: 'File actions', exact: true });
  await expect(files.getByRole('tab')).toHaveCount(names.length);
  await expect(files.getByTestId('workbench-close-files')).toHaveCount(0);
  await expect(row.locator('.thread-graph-editor-toolbar-button')).toHaveCount(1);
  await expect(shelf).toHaveCount(0);
  expect((await row.boundingBox())!.height).toBeCloseTo((await page.locator('.matter-tabs-row').boundingBox())!.height, 0);
  for (const tab of await files.locator('.thread-graph-editor-tab').all()) expect((await tab.boundingBox())!.width).toBeLessThanOrEqual(isMobile ? 136 : 156);
  await expect(files.locator('.workspace-file-tab-name').last()).toHaveCSS('direction', 'rtl');
  expect(await strip.evaluate(el => el.scrollWidth > el.clientWidth)).toBe(true);
  await expect(strip).toHaveCSS('scrollbar-width', 'none');
  const beforeBack = (await back.boundingBox())!, beforeMore = (await more.boundingBox())!;
  await strip.evaluate(el => { el.scrollLeft = 0; });
  await expect(edge).toHaveAttribute('data-overflow-end', 'true');
  if (isMobile) {
    const cdp = await context.newCDPSession(page); const box = (await strip.boundingBox())!;
    const x = box.x + box.width - 25, y = box.y + box.height / 2;
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ id: 1, x, y }] });
    for (const distance of [30, 70, 120, 160]) await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ id: 1, x: x - distance, y }] });
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await expect.poll(() => strip.evaluate(el => el.scrollLeft)).toBeGreaterThan(10);
  }
  await strip.evaluate(el => { el.scrollLeft = el.scrollWidth; });
  await expect(edge).toHaveAttribute('data-overflow-start', 'true');
  await expect(edge).toHaveAttribute('data-overflow-end', 'false');
  expect((await back.boundingBox())!.x).toBeCloseTo(beforeBack.x);
  expect((await more.boundingBox())!.x).toBeCloseTo(beforeMore.x);
  const lastTab = files.getByRole('tab', { name: names.at(-1)!, exact: true });
  const lastClose = files.getByRole('button', { name: `Close ${names.at(-1)}`, exact: true });
  await expect(lastClose).toBeInViewport();
  expect((await lastClose.boundingBox())!.x).toBeGreaterThan((await lastTab.boundingBox())!.x + 60);
  const threadStrip = page.getByRole('navigation', { name: 'Workspace threads', exact: true });
  await threadStrip.evaluate(el => { el.scrollLeft = 0; });
  await expect(page.locator('.matter-tab-scroll')).toHaveAttribute('data-overflow-end', 'true');
  await more.click();
  expect((await shelf.boundingBox())!.height).toBeLessThanOrEqual(30);
  expect((await shelf.boundingBox())!.width).toBeLessThan((await row.boundingBox())!.width * .7);
  expect((await shelf.boundingBox())!.x + (await shelf.boundingBox())!.width).toBeCloseTo((await row.boundingBox())!.x + (await row.boundingBox())!.width - 4, 0);
  await expect(shelf.locator('.workspace-file-state')).toHaveCount(0);
  await expect(shelf.getByRole('button', {name: /^Back/})).toHaveCount(0);
  expect((await shelf.boundingBox())!.y).toBeGreaterThanOrEqual((await row.boundingBox())!.y + (await row.boundingBox())!.height - 1);
  await expect(page.getByRole('menu', { name: 'File actions', exact: true })).toHaveCount(0);
  for (const name of ['Edit file', 'Markdown source', 'Reload from disk', 'Download file', 'Copy file path', 'Copy file name']) await expect(shelf.getByRole('button', { name, exact: true })).toBeVisible();
  await page.evaluate(() => Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { (window as unknown as { copiedFileText: string }).copiedFileText = text; } } }));
  await shelf.getByRole('button', {name: 'Copy file path', exact: true}).click();
  expect(await page.evaluate(() => (window as unknown as { copiedFileText: string }).copiedFileText)).toBe(path.join(root, names.at(-1)!));
  await shelf.getByRole('button', {name: 'Copy file name', exact: true}).click();
  expect(await page.evaluate(() => (window as unknown as { copiedFileText: string }).copiedFileText)).toBe(names.at(-1));
  // Check actual rendered colors, not only theme attributes or a CSS class.
  async function colors(target: Locator) {
    return target.evaluate(el => {
      const ctx = document.createElement('canvas').getContext('2d')!;
      const rgb = (color: string) => { ctx.clearRect(0, 0, 1, 1); ctx.fillStyle = color; ctx.fillRect(0, 0, 1, 1); return [...ctx.getImageData(0, 0, 1, 1).data].slice(0, 3); };
      const style = getComputedStyle(el); return { bg: rgb(style.backgroundColor), fg: rgb(style.color) };
    });
  }
  for (const theme of ['dark', 'light'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    await expect(page.locator('html')).toHaveAttribute('data-theme-effective', theme);
    const color = await colors(shelf);
    if (theme === 'dark') { expect(Math.max(...color.bg)).toBeLessThan(90); expect(Math.min(...color.fg)).toBeGreaterThan(100); }
    else { expect(Math.min(...color.bg)).toBeGreaterThan(180); expect(Math.max(...color.fg)).toBeLessThan(150); }
    await page.screenshot({ path: testInfo.outputPath(`${isMobile ? 'mobile' : 'desktop'}-${theme}-actions.png`), scale: 'css', animations: 'disabled' });
  }
  await shelf.getByRole('button', { name: 'Markdown source', exact: true }).click();
  await expect(shelf.getByRole('button', { name: 'Markdown preview', exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath(`${isMobile ? 'mobile' : 'desktop'}-light-source.png`), scale: 'css', animations: 'disabled' });
  await shelf.getByRole('button', { name: 'Markdown preview', exact: true }).click();
  await writeFile(path.join(root, names.at(-1)!), '# Refreshed from disk\n\nFresh content.');
  await shelf.getByRole('button', { name: 'Reload from disk', exact: true }).click();
  await expect(files.getByRole('heading', { name: 'Refreshed from disk', exact: true })).toBeVisible();
  const downloadPromise = page.waitForEvent('download');
  await shelf.getByRole('button', { name: 'Download file', exact: true }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe(names.at(-1));
  expect(await readFile((await download.path())!, 'utf8')).toContain('Fresh content.');
  await shelf.getByRole('button', { name: 'Edit file', exact: true }).click();
  await expect(shelf.getByRole('button', { name: 'Save file', exact: true })).toBeVisible();
  await expect(shelf.getByRole('button', { name: 'Cancel edits', exact: true })).toBeVisible();
  await shelf.getByRole('button', { name: 'Cancel edits', exact: true }).click();
  await more.click();
  await expect(shelf).toHaveCount(0);
  await page.emulateMedia({ colorScheme: 'dark' });
  await expect(page.locator('html')).toHaveAttribute('data-theme-effective', 'dark');
  await page.screenshot({ path: testInfo.outputPath(`${isMobile ? 'mobile' : 'desktop'}-dark-collapsed.png`), scale: 'css', animations: 'disabled' });
  await lastClose.click();
  await expect(files.getByRole('tab')).toHaveCount(names.length - 1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width);
});
