import { expect, test } from '@playwright/test';
import { hardware } from './desktop/fixtures';

test('monitor keeps its closed reading budget and preserves every theme at desktop and phone widths', async ({ page }, info) => {
  let reads = 0;
  await page.route('**/api/device/metrics', async route => {
    reads++;
    return route.fulfill({ json: { ...hardware(0), sampledAt: await page.evaluate(() => new Date().toISOString()) } });
  });
  await page.clock.install();
  await page.clock.pauseAt(Date.now());
  for (const theme of ['classic', 'plum-pocket', 'terminal']) {
    for (const mode of ['light', 'dark']) {
      for (const width of [1440, 390]) {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(`/e2e/fixtures/device-monitor.html?theme=${theme}&mode=${mode}`);
        const monitor = page.getByRole('button', { name: 'Device monitor', exact: true });
        await expect(monitor).toBeVisible();
        await expect(monitor.locator('[aria-label^="CPU:"]')).toHaveText('32%');
        const before = reads;
        await page.clock.runFor(14999);
        expect(reads).toBe(before);
        await page.clock.runFor(1);
        await expect.poll(() => reads).toBe(before + 1);
        await monitor.click();
        const dialog = page.getByRole('dialog', { name: 'Device monitor', exact: true });
        await expect(dialog).toBeVisible();
        await expect(dialog.getByRole('meter', { name: 'CPU utilization' })).toHaveAttribute('aria-valuenow', '32');
        await expect.poll(() => reads).toBe(before + 2);
        await page.clock.runFor(4999);
        expect(reads).toBe(before + 2);
        await page.clock.runFor(1);
        await expect.poll(() => reads).toBe(before + 3);
        const bounds = await dialog.boundingBox();
        expect(bounds!.x).toBeGreaterThanOrEqual(0);
        expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
        await page.screenshot({ path: info.outputPath(`${theme}-${mode}-${width}.png`) });
      }
    }
  }
});
