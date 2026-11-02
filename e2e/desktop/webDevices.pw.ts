import { expect, test } from '@playwright/test';
import { seed } from './fixtures';

const evidence = process.env.DESKTOP_EVIDENCE_DIR ?? 'test-results/desktop';
for (const [pack, dark] of [['classic', false], ['classic', true], ['plum-pocket', false], ['plum-pocket', true], ['terminal', false], ['terminal', true]] as const) {
  test(`statistics work in a plain browser: ${pack} ${dark ? 'dark' : 'light'}`, async ({ page }) => {
    await seed(page, pack, dark, false, true, false);
    expect(await page.evaluate(() => 'pockymoeHost' in window && !!window.pockymoeHost)).toBe(false);
    await expect(page.getByTestId('desktop-devices')).toBeVisible();
    await expect(page.locator('.pd-account-usage')).toBeVisible();
    await expect(page.locator('.pd-usage-device').first()).toBeVisible();
    await page.locator('.pd-device-usage').scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${evidence}/web-devices-${pack}-${dark ? 'dark' : 'light'}.png`, animations: 'disabled' });
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    await page.screenshot({ path: `${evidence}/web-devices-${pack}-${dark ? 'dark' : 'light'}-390.png`, animations: 'disabled' });
  });
}
