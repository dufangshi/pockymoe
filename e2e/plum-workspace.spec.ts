import { test, expect, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, symlink, writeFile } from 'node:fs/promises';
import path from 'node:path';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

/** WCAG contrast of an element's text against its own background. */
function contrast(page: Page, selector: string) {
  return page.locator(selector).first().evaluate((element) => {
    // Any CSS color (rgb, oklch, color-mix…) as sRGB bytes, through a canvas.
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 1;
    const paint = canvas.getContext('2d', { willReadFrequently: true })!;
    const parse = (value: string) => {
      paint.clearRect(0, 0, 1, 1);
      paint.fillStyle = value;
      paint.fillRect(0, 0, 1, 1);
      return Array.from(paint.getImageData(0, 0, 1, 1).data.slice(0, 3));
    };
    const luminance = (rgb: number[]) => {
      const [r, g, b] = rgb.map((c) => { const s = c / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; });
      return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
    };
    const style = getComputedStyle(element);
    const [a, b] = [luminance(parse(style.color)), luminance(parse(style.backgroundColor))].sort((x, y) => y - x);
    return (a! + 0.05) / (b! + 0.05);
  });
}

test('workspace errors stay readable when the system scheme differs, and Explorer row actions stay compact', async ({ page, request, isMobile }) => {
  // The phone is in dark mode while the app is light: the case that hid the banner text.
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('remote-codex-theme-mode', 'light');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  for (const preset of ['classic', 'plum-pocket']) {
    const root = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
    await mkdir(path.join(root, 'src'), { recursive: true });
    await writeFile(path.join(root, 'package.json'), '{ "name": "demo" }\n');
    await symlink('loop-link', path.join(root, 'loop-link'));
    const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath: root, label: 'Explorer regression' } })).json();
    const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Explorer regression', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
    const id = started.id ?? started.thread.id;
    await page.addInitScript((value) => localStorage.setItem('pockymoe-theme-preset', value), preset);
    await page.goto(`/threads/${id}`);
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', preset);
    const files = page.getByTestId('workspace-panel');
    if (!(await files.isVisible())) await page.locator(isMobile ? '.matter-topbar' : '.matter-rail').getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
    const backToFiles = files.getByRole('button', { name: 'Back to files', exact: true });
    if (await backToFiles.isVisible()) await backToFiles.click();
    const row = files.getByRole('treeitem', { name: 'package.json', exact: true });
    await expect(row).toBeVisible();
    // On a phone, "Show Editor" only appears once there is a file to show.
    if (isMobile) {
      // With a folder selected there is no file to show, so the button is gone.
      await files.getByRole('treeitem', { name: 'src', exact: true }).getByRole('button', { name: 'src', exact: true }).click({ force: true });
      await expect(files.getByRole('treeitem', { name: 'src', exact: true })).toHaveAttribute('aria-selected', 'true');
      await expect(files.getByTestId('expand-viewer')).toHaveCount(0);
    }
    if (!isMobile) await row.hover();

    // Plum row actions are bare icons: no resting box on the always-visible phone trigger.
    if (preset === 'plum-pocket' && isMobile) expect(await files.getByRole('button', { name: 'More actions for package.json', exact: true }).evaluate((e) => getComputedStyle(e).backgroundColor)).toBe('rgba(0, 0, 0, 0)');
    // Escape closes the row menu even though focus is inside it.
    await files.getByRole('button', { name: 'More actions for package.json', exact: true }).click();
    const menu = page.getByRole('menu', { name: 'Actions for package.json' });
    await expect(menu).toBeVisible();
    await expect(menu.getByRole('menuitem', { name: 'Copy relative path for package.json', exact: true })).toBeVisible();
    if (preset === 'plum-pocket') {
      await expect(menu.getByRole('menuitem', { name: 'Copy relative path for package.json', exact: true })).toHaveText('Copy relative path');
      for (const item of await menu.getByRole('menuitem').all()) expect(await item.evaluate((e) => e.scrollWidth <= e.clientWidth && e.getBoundingClientRect().height <= 44.5)).toBe(true);
    }
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);

    // A file that cannot open shows a mark; its reason is readable in every preset (AA body text).
    const open = async (name: string) => {
      if (await backToFiles.isVisible()) await backToFiles.click();
      await files.getByRole('treeitem', { name, exact: true }).getByRole('button', { name, exact: true }).click({ force: true });
    };
    await open('package.json');
    await expect(page.getByRole('tab', { name: 'package.json', exact: true })).toHaveAttribute('aria-selected', 'true');
    await open('loop-link');
    const mark = page.getByRole('button', { name: 'Why it failed' }).first();
    await expect(mark).toBeVisible();
    await mark.click();
    await expect(page.locator('.error-mark-bubble')).toBeVisible();
    expect(await contrast(page, '.error-mark-bubble')).toBeGreaterThanOrEqual(4.5);
    await page.keyboard.press('Escape');
  }
});
