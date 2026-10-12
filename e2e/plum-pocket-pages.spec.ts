import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';

// Plum Pocket redraws the tab-bar tools and the outer pages with CSS only.
// These checks pin what CSS alone can break: icons, help text and button wrapping.
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

async function open(page: Page, url: string, preset: string) {
  await page.addInitScript(value => localStorage.setItem('pockymoe-theme-preset', value), preset);
  await page.goto(url);
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
}

test('Plum Pocket redraws the tab-bar tools and keeps Classic icons', async ({ page, request }) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `plum-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Plum tools' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.thread?.id ?? started.id;
  await open(page, `/threads/${id}`, 'plum-pocket');
  const tools = page.locator('.matter-tabs-row .matter-toolbar-toggle');
  const split = page.getByTestId('workbench-split-trigger');
  await expect(split).toBeVisible();
  for (const button of [tools, split]) {
    await expect(button.locator('svg')).toBeHidden();
    const glyph = await button.evaluate(element => {
      const style = getComputedStyle(element, '::before');
      return { mask: style.maskImage || style.webkitMaskImage, width: style.width };
    });
    expect(glyph.mask).toContain('data:image/svg+xml');
    expect(glyph.width).not.toBe('auto');
  }
  await expect(tools).toHaveAccessibleName('Thread tools');
  await expect(split).toHaveAccessibleName('Split conversation');
  await page.evaluate(() => { document.documentElement.dataset.themePreset = 'classic'; });
  await expect(tools.locator('svg')).toBeVisible();
  await expect(split.locator('svg')).toBeVisible();
});

test('Plum Pocket outer pages show one title, no help paragraphs and unwrapped buttons', async ({ page }) => {
  await open(page, '/workspaces', 'plum-pocket');
  const title = page.locator('.product-navigation h1');
  await expect(title).toHaveText('Workspaces');
  expect(await title.evaluate(element => getComputedStyle(element, '::before').backgroundImage)).toContain('pockymoe-icon.png');
  for (const url of ['/workspaces/new', '/threads/import']) {
    await page.goto(url);
    await expect(page.locator('.product-page')).toBeVisible();
    for (const help of await page.locator('.product-page :is(.product-description, .product-eyebrow)').all()) await expect(help).toBeHidden();
    const wrapped = await page.locator('.product-page :is(.relay-button-primary, .relay-button-secondary, .host-secondary-button, .ui-action-primary)').evaluateAll(buttons =>
      buttons.filter(button => {
        const box = button.getBoundingClientRect();
        const line = parseFloat(getComputedStyle(button).lineHeight) || 20;
        return box.width > 0 && (button.scrollWidth > button.clientWidth + 1 || box.height > Math.max(48, line * 2));
      }).map(button => button.textContent?.trim()),
    );
    expect(wrapped, url).toEqual([]);
  }
});

// The phone top bar holds the device monitor and every tool without overlap or
// overflow, in Plum (44px-tall slots) and in Classic, down to 320px.
for (const [preset, width] of [['plum-pocket', 360], ['plum-pocket', 320], ['classic', 360], ['classic', 320]] as const) test(`phone top bar keeps the device monitor and fits ${width}px (${preset})`, async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Phone top bar');
  await page.setViewportSize({ width, height: 780 });
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `plum-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Top bar fit' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  let hasGpu = false;
  await page.route('**/api/device/metrics', route => route.fulfill({ json: {
    sampledAt: new Date().toISOString(), platform: 'linux', environment: 'native', sampleWindowMs: 2000,
    cpu: { model: 'CPU', usagePercent: 100, logicalCoreCount: 4, cores: [] },
    memory: { totalBytes: 16 * 2 ** 30, usedBytes: 12.34 * 2 ** 30, availableBytes: 0, usagePercent: 100 },
    swap: { totalBytes: 0, usedBytes: 0, availableBytes: 0, usagePercent: null },
    cpuPower: { watts: null, source: null, reason: null }, hardwareNotes: [], limits: null,
    gpus: hasGpu ? [{ id: 'gpu', name: 'Test GPU', usagePercent: 100, usedMemoryBytes: 6.78 * 2 ** 30, totalMemoryBytes: 8 * 2 ** 30, power: { watts: null, source: null, reason: null }, source: 'test' }] : [],
  } }));
  await open(page, `/threads/${started.thread?.id ?? started.id}`, preset);
  const topbar = page.locator('header.matter-topbar');
  const monitor = topbar.getByRole('button', { name: 'Device monitor' });
  await expect(monitor).toBeVisible();
  await expect(page.locator('.matter-drawer-tools .device-monitor-button')).toHaveCount(0);
  for (const gpu of [false, true]) {
    hasGpu = gpu;
    if (gpu) await page.reload();
    await expect(monitor.locator('[aria-label^=DRAM]')).toHaveText('12.3/16.0G');
    await expect(monitor.locator('[aria-label^=CPU] svg')).toBeVisible();
    await expect(monitor.locator('.dm-summary-row')).toHaveCount(gpu ? 2 : 1);
    if (gpu) {
      await expect(monitor.locator('[aria-label^=GPU]')).toHaveText('100%');
      await expect(monitor.locator('[aria-label^=VRAM]')).toHaveText('6.8/8.0G');
    }
    const fit = await monitor.evaluate(button => {
      const box = button.getBoundingClientRect();
      return [...button.querySelectorAll('.dm-summary-reading')].map(row => {
        const r = row.getBoundingClientRect();
        return { overflow: row.scrollWidth - row.clientWidth, top: r.top - box.top, bottom: box.bottom - r.bottom };
      });
    });
    for (const row of fit) {
      expect(row.overflow, JSON.stringify(fit)).toBeLessThanOrEqual(0);
      expect(row.top).toBeGreaterThanOrEqual(0);
      expect(row.bottom).toBeGreaterThanOrEqual(0);
    }
    await page.screenshot({ path: testInfo.outputPath(`monitor-${preset}-${width}-${gpu ? 'gpu' : 'cpu'}.png`), scale: 'css' });
  }
  if (preset === 'plum-pocket') {
    // Readings shade from green when idle to red when busy.
    const hues = await monitor.evaluate(button => {
      const value = button.querySelector('b')!;
      return [5, 95].map(load => {
        value.style.setProperty('--load', String(load));
        return Number(/oklch\([\d.]+ [\d.]+ ([\d.]+)/.exec(getComputedStyle(value).color)![1]);
      });
    });
    expect(hues[0]).toBeGreaterThan(130);
    expect(hues[1]).toBeLessThan(40);
  }
  const items = await topbar.evaluate(header => [...header.querySelectorAll('button, a')]
    .filter(item => item.getClientRects().length && getComputedStyle(item).visibility !== 'hidden')
    .map(item => { const box = item.getBoundingClientRect(); return { name: item.getAttribute('aria-label') ?? item.textContent ?? '', left: box.left, right: box.right, height: box.height, clipped: item.scrollWidth > item.clientWidth + 1 }; })
    .sort((a, b) => a.left - b.left));
  expect(items.length).toBeGreaterThanOrEqual(8);
  for (const [index, item] of items.entries()) {
    expect(item.left, item.name).toBeGreaterThanOrEqual(0);
    expect(item.right, item.name).toBeLessThanOrEqual(width);
    if (preset === 'plum-pocket') expect(item.height, item.name).toBeGreaterThanOrEqual(40);
    expect(item.clipped, item.name).toBe(false);
    if (index) expect(item.left, `${item.name} overlaps ${items[index - 1]!.name}`).toBeGreaterThanOrEqual(items[index - 1]!.right - 0.5);
  }
  expect(await topbar.evaluate(header => header.scrollWidth <= header.clientWidth)).toBe(true);
  // The search trigger receives its own clicks.
  const search = topbar.getByRole('button', { name: 'Search conversation' });
  const box = (await search.boundingBox())!;
  expect(await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.closest('.matter-search-trigger') !== null, [box.x + box.width / 2, box.y + box.height / 2])).toBe(true);
});
