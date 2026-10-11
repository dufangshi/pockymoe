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
  expect(await title.evaluate(element => getComputedStyle(element, '::before').backgroundImage)).toContain('icon-192');
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
