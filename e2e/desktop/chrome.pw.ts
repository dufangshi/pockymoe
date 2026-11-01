import { expect, test } from '@playwright/test';
import { seed } from './fixtures';

test('signed-out desktop login and guide have a draggable 38px title bar', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: false } }));
  for (const path of ['/relay-portal', '/relay-guide']) {
    await page.goto(`${path}?desktopMockHost=1`);
    await expect(page.locator('body')).not.toBeEmpty();
    const region = () => page.evaluate(() => {
      const element = document.elementFromPoint(240, 18);
      return element ? getComputedStyle(element).getPropertyValue('-webkit-app-region') : '';
    });
    await expect.poll(region).toBe('drag');
    await expect(page.locator('[data-desktop-titlebar]')).toHaveCSS('height', '38px');
    await expect(page.locator('.desktop-window-page')).toHaveCSS('scrollbar-width', 'none');
    if (path === '/relay-portal') expect(await page.locator('.desktop-window-page').evaluate(element => element.scrollHeight <= element.clientHeight)).toBe(true);
    if (process.env.DESKTOP_EVIDENCE_DIR) await page.screenshot({ path: `${process.env.DESKTOP_EVIDENCE_DIR}/signed-out-${path.slice(1)}.png` });
  }
});

test('ordinary browsers keep their page layout without desktop chrome', async ({ page }) => {
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: false } }));
  await page.goto('/relay-portal');
  await expect(page.getByLabel('Email or username', { exact: true })).toBeVisible();
  await expect(page.locator('[data-desktop-titlebar]')).toHaveCount(0);
});


test('narrow desktop main content uses the viewport rather than an implicit third grid column', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await seed(page, 'classic', false, false, true);
  const main = page.locator('.desktop-map-scene:not([hidden]) .matter-content');
  await expect(main).toBeVisible();
  const rect = await main.boundingBox();
  expect(rect!.x).toBeLessThanOrEqual(1);
  expect(rect!.width).toBeGreaterThanOrEqual(389);
});


test('map and device pages disable inspector/explorer outside a workspace', async ({ page }) => {
  await seed(page);
  const explorer = page.locator('.desktop-map-scene .matter-rail button:has([data-icon=explorer])');
  await expect(explorer).toBeDisabled(); await expect(explorer).toHaveAttribute('aria-expanded', 'false');
  await expect(page.locator('.desktop-map-scene .matter-toolbar-toggle')).toBeDisabled();
  await expect(page.locator('.desktop-map-scene .matter-explorer')).toHaveCount(0);
  await page.locator('.desktop-map-scene .matter-rail button:has([data-icon=thread-map])').first().click();
  await expect(page.locator('.desktop-map-scene .thread-map')).toBeVisible();
  await expect(explorer).toBeDisabled();
});

for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 390]) {
  test(`signed-out chrome ${pack} ${dark ? 'dark' : 'light'} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(({ pack, dark }) => {
      localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem('pockymoe-theme-preset', pack);
      localStorage.setItem('remote-codex-theme-mode', dark ? 'dark' : 'light');
    }, { pack, dark });
    await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: false } }));
    for (const route of ['relay-portal', 'relay-guide']) {
      await page.goto(`/${route}?desktopMockHost=1`);
      await expect(page.locator('[data-desktop-titlebar]')).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-theme-preset', pack);
      expect(await page.evaluate(() => getComputedStyle(document.elementFromPoint(200, 18)!).getPropertyValue('-webkit-app-region'))).toBe('drag');
      if (process.env.DESKTOP_EVIDENCE_DIR) await page.screenshot({ path: `${process.env.DESKTOP_EVIDENCE_DIR}/${route}-${pack}-${dark ? 'dark' : 'light'}-${width}.png` });
    }
  });
}
