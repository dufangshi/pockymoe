import { test, expect } from '@playwright/test';
import { seed } from './fixtures';
import type { ModelPriceRates } from '../../apps/supervisor-web/src/lib/modelPricingApi';

// Rust tests exercise persistent account isolation and synchronization. This
// browser case covers the account entry, edit/reset controls and theme geometry.
test('account reference prices edit unknown models and fit every theme on phone and desktop', async ({ page }, info) => {
  await seed(page, 'classic', false, false, false, false);
  const reference = { inputUsdPerMillion: 2, cachedInputUsdPerMillion: 0.2, outputUsdPerMillion: 8 };
  const models: Record<string, ModelPriceRates> = {
    'future-model': { unpriced: true } as unknown as ModelPriceRates,
    'official-model': { ...reference },
  };
  await page.route('**/relay/account/model-pricing', async route => {
    if (route.request().method() === 'PATCH') {
      const input = route.request().postDataJSON();
      models[input.id] = input.reset ? { ...reference } : { ...input.rates, custom: true };
    }
    await route.fulfill({ json: { models, syncedAt: '2026-11-03T12:00:00Z' } });
  });
  for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) {
    await page.addInitScript(({ pack, dark }) => {
      localStorage.setItem('pockymoe-theme-preset', pack);
      localStorage.setItem('remote-codex-theme-mode', dark ? 'dark' : 'light');
    }, { pack, dark });
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto('/relay-account');
      await expect(page.locator('html')).toHaveAttribute('data-theme-preset', pack);
      await expect(page.locator('html')).toHaveAttribute('data-theme-effective', dark ? 'dark' : 'light');
      await page.locator('summary').filter({ hasText: /^模型价格$/ }).click();
      const panel = page.getByRole('region', { name: '模型价格', exact: true });
      const row = panel.getByRole('row').filter({ hasText: 'future-model' });
      await expect(row).toContainText('暂未定价');
      await row.getByRole('button').click();
      const dialog = page.getByRole('dialog');
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole('spinbutton').first()).toBeVisible();
      const rect = (await dialog.boundingBox())!;
      expect(rect.x).toBeGreaterThanOrEqual(0);
      expect(rect.x + rect.width).toBeLessThanOrEqual(width + 1);
      if (width < 640) await expect.poll(async () => {const box=(await dialog.boundingBox())!; return Math.abs(box.y + box.height - 1000);}).toBeLessThan(2);
      await page.screenshot({ path: info.outputPath(`${pack}-${dark ? 'dark' : 'light'}-${width}.png`) });
      if (pack === 'classic' && !dark && width === 1440) {
        for (const [i, value] of ['2', '0.2', '8'].entries()) await dialog.getByRole('spinbutton').nth(i).fill(value);
        await dialog.getByRole('button', { name: '保存价格', exact: true }).click();
        await expect(dialog).toHaveCount(0);
        await expect(row).toContainText('$2');
        await row.getByRole('button').click();
        await page.getByRole('button', { name: '重置 / 移除自定义', exact: true }).click();
        await expect(page.getByRole('dialog')).toHaveCount(0);
        // Restore the unpriced fixture for the following visual checks.
        models['future-model'] = { unpriced: true } as unknown as ModelPriceRates;
      } else {
        await page.keyboard.press('Escape');
        await expect(dialog).toHaveCount(0);
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
  }
});
