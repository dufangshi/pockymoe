import { test, expect } from '@playwright/test';
import { seed } from './fixtures';
import { mkdirSync, writeFileSync } from 'node:fs';
const evidence = process.env.DEVICE_MANAGEMENT_EVIDENCE ?? '.scratch/device-management';
const phase = process.env.DEVICE_MANAGEMENT_PHASE ?? 'after';
test('workspace visual evidence and route frames', async ({ page }) => {
  mkdirSync(`${evidence}/${phase}`, { recursive: true });
  await page.addInitScript(() => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'relay:devices-test'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })));
  await seed(page);
  await page.getByRole('button', { name: 'MacBook Pro 的操作', exact: true }).click();
  // Navigate through the app so the desktop shell stays mounted.
  await page.getByRole('menuitem', { name: '新建线程', exact: true }).click();
  await page.evaluate(() => { history.pushState({}, '', '/devices/device-0/workspaces'); dispatchEvent(new PopStateEvent('popstate')); });
  await expect(page.locator('[data-page="workspaces"]')).toBeVisible();
  for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 1000, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(({ pack, dark }) => { localStorage.setItem('pockymoe-theme-preset', pack); localStorage.setItem('remote-codex-theme-mode', dark ? 'dark' : 'light'); }, { pack, dark });
    await page.reload();
    await expect(page.locator('[data-page="workspaces"]')).toBeVisible();
    await expect(page.locator('[data-page="workspaces"] .product-list a').filter({ hasText: 'Pockymoe' }).first()).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', pack);
    if (phase !== 'before' && width >= 640) {
      const content = page.locator('.desktop-workspaces-content');
      const box = await content.boundingBox();
      const sidebar = await page.locator('.desktop-workspaces-shell .matter-sidebar').boundingBox();
      const title = await page.locator('.desktop-workspaces-title').boundingBox();
      expect(title!.x - (sidebar!.x + sidebar!.width)).toBe(32);
      expect(title!.y).toBe(70);
      expect(box!.width).toBeLessThanOrEqual(944);
      await expect(page.locator('.desktop-workspaces-content > details')).not.toHaveAttribute('open', '');
      await expect(page.locator('.desktop-workspace-actions')).toContainText('添加工作区');
    }
    await page.screenshot({ path: `${evidence}/${phase}/workspaces-${pack}-${dark ? 'dark' : 'light'}-${width}.png` });
  }
});


test('workspace states keep their gaps below 900px and a plain browser keeps its header', async ({ page, browser }) => {
  await page.setViewportSize({ width: 850, height: 1000 });
  await seed(page);
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  let failed = false;
  await page.route('**/relay/devices/device-0/api/workspaces*', async route => {
    if (failed) { await route.fulfill({ status: 503, json: { code: 'unavailable', message: 'Fixture unavailable' } }); return; }
    await pending;
    await route.fulfill({ json: [] });
  });
  await page.goto('/devices/device-0/workspaces?desktopMockHost=1');
  const content = page.locator('.desktop-workspaces-content');
  const gap = async () => {
    const title = (await page.locator('.desktop-workspaces-title').boundingBox())!;
    const sidebar = (await page.locator('.desktop-workspaces-shell .matter-sidebar').boundingBox())!;
    expect(title.x - sidebar.x - sidebar.width).toBe(20);
    expect(title.y).toBe(70);
  };
  await expect(content.locator('.product-skeleton').first()).toBeVisible(); await gap();
  release();
  await expect(content.locator('.product-empty')).toBeVisible(); await gap();
  failed = true; await page.reload();
  await expect(content.getByRole('button', { name: '重试', exact: true })).toBeVisible(); await gap();
  const ordinary = await browser.newPage();
  await seed(ordinary, 'classic', false, false, false, false);
  await ordinary.route('**/api/workspaces', route => route.fulfill({ json: [] }));
  await ordinary.route('**/api/storage*', route => route.fulfill({ json: { deviceName: 'Fixture' } }));
  await ordinary.routeWebSocket(/\/ws/, socket => socket.onMessage(() => undefined));
  await ordinary.goto('/devices/device-0/workspaces');
  await expect(ordinary.locator('.product-navigation')).toBeVisible();
  await expect(ordinary.locator('.desktop-workspaces-shell')).toHaveCount(0);
  await ordinary.close();
});

test('desktop workspace content clears Windows controls on the right', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.addInitScript(() => {
    let value: typeof window.pockymoeHost;
    Object.defineProperty(window, 'pockymoeHost', {
      configurable: true, get: () => value,
      set(host: NonNullable<typeof window.pockymoeHost>) {
        value = host; const original = host.capabilities;
        host.capabilities = async () => ({ ...await original(), platform: 'win32', windowChrome: { kind: 'hiddenInset', controlsSide: 'right', trafficLightsWidth: 150, toolbarHeight: 38 } });
      },
    });
  });
  await seed(page);
  await page.getByRole('link', { name: '打开 MacBook Pro 的工作区', exact: true }).click();
  const title = page.locator('.desktop-workspaces-title');
  await expect(title).toBeVisible();
  expect((await title.boundingBox())!.y).toBe(70);
  await expect(page.locator('.desktop-workspaces-shell .matter-tabs-row')).toHaveCSS('padding-right', '150px');
  await expect(page.locator('.desktop-workspaces-shell .matter-tabs-row')).toHaveCSS('padding-left', '0px');
});
