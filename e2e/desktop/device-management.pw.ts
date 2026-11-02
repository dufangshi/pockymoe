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

