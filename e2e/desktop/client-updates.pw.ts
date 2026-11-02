import { expect, test, type Page } from '@playwright/test';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { seed } from './fixtures';
const evidence = process.env.DESKTOP_EVIDENCE_DIR ?? join(tmpdir(), 'pockymoe-client-updates');
const release = { version: '0.2.1', releaseUrl: 'https://github.com/dufangshi/pockymoe-app/releases/tag/v0.2.1' };
async function update(page: Page, status: string, error?: string) {
  await page.evaluate(state => window.dispatchEvent(new CustomEvent('desktopMock:update', { detail: state })), { ...release, status, ...(error ? { error } : {}) });
}
async function setup(page: Page, pack = 'classic', dark = false, width = 1440, enabled = true) {
  await page.setViewportSize({ width, height: 1000 });
  if (enabled) await page.addInitScript(() => { const url = new URL(location.href); url.searchParams.set('desktopMockUpdates', '1'); history.replaceState(null, '', url); });
  await page.routeWebSocket(/^ws:\/\/127\.0\.0\.1:18929\/\?token=/, socket => socket.onMessage(() => undefined));
  await seed(page, pack, dark);
  await page.evaluate(() => {
    Object.assign(window, { updateInstallCount: 0, updateDownloadPage: '' });
    window.addEventListener('desktopMock:updateInstall', () => { (window as any).updateInstallCount++; });
    window.pockymoeHost!.shell.openExternal = url => { (window as any).updateDownloadPage = url; };
  });
  if (width < 640) {
    await page.getByRole('button', { name: '更多操作', exact: true }).click();
    await page.getByRole('button', { name: '工作区与快捷方式', exact: true }).click();
    await expect(page.getByRole('dialog', { name: '更多操作', exact: true })).toBeHidden();
  }
}
const icon = (page: Page) => page.getByRole('button', { name: '更新到 0.2.1', exact: true });

test('older hosts keep the client update control hidden', async ({ page }) => {
  await setup(page, 'classic', false, 1440, false); await update(page, 'ready'); await expect(icon(page)).toHaveCount(0);
});

test('ready, installing and failure recovery preserve the icon and host argument boundary', async ({ page }) => {
  await setup(page); await update(page, 'downloading'); await expect(icon(page)).toHaveCount(0);
  await update(page, 'ready'); await expect(icon(page)).toBeVisible();
  await icon(page).click(); await expect(icon(page)).toHaveAttribute('aria-busy', 'true');
  await expect(icon(page).locator('.composer-send-spinner')).toBeVisible(); await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).updateInstallCount)).toBe(1);
  await update(page, 'error', '磁盘空间不足。'); await expect(icon(page)).toBeEnabled();
  await page.locator('.desktop-client-update-error:visible').click(); await expect(page.getByRole('dialog')).toContainText('磁盘空间不足。');
  await page.getByRole('button', { name: '前往下载', exact: true }).click(); expect(await page.evaluate(() => (window as any).updateDownloadPage)).toBe(release.releaseUrl);
  await page.locator('.desktop-client-update-error:visible').click(); await page.getByRole('button', { name: '重试', exact: true }).click();
  expect(await page.evaluate(() => (window as any).updateInstallCount)).toBe(2);
});

for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 390]) {
  test(`client update appearance ${pack} ${dark ? 'dark' : 'light'} ${width}`, async ({ page }) => {
    await setup(page, pack, dark, width); await update(page, 'ready'); await expect(icon(page)).toBeVisible();
    const bounds = await icon(page).boundingBox(); expect(bounds!.width).toBeCloseTo(40, 1); expect(bounds!.height).toBeCloseTo(40, 1);
    const circle = icon(page).locator('.desktop-client-update-circle'); const dot = await circle.boundingBox(); expect(dot!.width).toBeCloseTo(32, 1); expect(dot!.height).toBeCloseTo(32, 1);
    const glyph = await icon(page).locator('svg').boundingBox(); expect(glyph!.width).toBeCloseTo(16, 1); expect(glyph!.height).toBeCloseTo(16, 1);
    await expect(icon(page)).toHaveAttribute('title', '更新到 0.2.1');
    const contrast = await circle.evaluate(element => {
      const canvas = document.createElement('canvas'); canvas.width = canvas.height = 1;
      const context = canvas.getContext('2d')!;
      const luminance = (colour: string) => { context.fillStyle = colour; context.fillRect(0, 0, 1, 1); const rgb = [...context.getImageData(0, 0, 1, 1).data].slice(0, 3).map(value => { const n = value / 255; return n <= .04045 ? n / 12.92 : ((n + .055) / 1.055) ** 2.4; }); return rgb[0]! * .2126 + rgb[1]! * .7152 + rgb[2]! * .0722; };
      const style = getComputedStyle(element), fg = luminance(style.color), bg = luminance(style.backgroundColor);
      return (Math.max(fg, bg) + .05) / (Math.min(fg, bg) + .05);
    });
    expect(contrast).toBeGreaterThanOrEqual(3);
    expect(await icon(page).locator('svg').evaluate(element => getComputedStyle(element).animationName)).toBe('none');
    const container = width >= 640 ? page.locator('.matter-rail-bottom') : page.locator('.matter-sidebar-host-actions');
    await expect(container.locator('.desktop-client-update')).toBeVisible();
    await page.screenshot({ path: join(evidence, `${pack}-${dark ? 'dark' : 'light'}-${width}-ready.png`) });
    await page.locator('.desktop-client-update:visible').screenshot({ path: join(evidence, `${pack}-${dark ? 'dark' : 'light'}-${width}-ready-control.png`) });
    await update(page, 'error', '磁盘空间不足。'); await page.locator('.desktop-client-update-error:visible').click();
    await expect(page.getByRole('dialog')).toContainText('前往下载'); await expect(page.getByRole('dialog')).toContainText('重试');
    if (width < 640) for (const button of await page.getByRole('dialog').getByRole('button').all()) expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(40);
    for (const button of await page.getByRole('dialog').getByRole('button').all()) {
      const contrast = await button.evaluate(element => {
        const canvas = document.createElement('canvas'); canvas.width = canvas.height = 1; const context = canvas.getContext('2d')!;
        const lum = (colour: string) => { context.fillStyle = colour; context.fillRect(0, 0, 1, 1); const rgb = [...context.getImageData(0, 0, 1, 1).data].slice(0, 3).map(value => { const n = value / 255; return n <= .04045 ? n / 12.92 : ((n + .055) / 1.055) ** 2.4; }); return rgb[0]! * .2126 + rgb[1]! * .7152 + rgb[2]! * .0722; };
        const fg = lum(getComputedStyle(element).color), bg = lum(getComputedStyle(element.closest('[role=dialog]')!).backgroundColor);
        return (Math.max(fg, bg) + .05) / (Math.min(fg, bg) + .05);
      });
      expect(contrast).toBeGreaterThanOrEqual(4.5);
    }
    await page.screenshot({ path: join(evidence, `${pack}-${dark ? 'dark' : 'light'}-${width}-error.png`) });
    await page.getByRole('dialog').screenshot({ path: join(evidence, `${pack}-${dark ? 'dark' : 'light'}-${width}-error-bubble.png`) });
  });
}
