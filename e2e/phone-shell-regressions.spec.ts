import { test, expect, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

test.use({ actionTimeout: 15_000 });
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

async function threadFixture(request: import('@playwright/test').APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', randomUUID());
  await mkdir(absPath, { recursive: true });
  await writeFile(path.join(absPath, 'README.md'), '# phone shell');
  const workspaceResponse = await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Phone shell' } });
  expect(workspaceResponse.ok()).toBeTruthy();
  const workspace = await workspaceResponse.json();
  const response = await request.post(`${base}/api/threads/start`, { data: {
    workspaceId: workspace.id, title: 'Phone shell regression', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo',
  } });
  expect(response.ok()).toBeTruthy();
  const value = await response.json();
  return value.id ?? value.thread.id;
}

async function dragTouch(page: Page, selector: string, startX: number, endX: number) {
  const box = (await page.locator(selector).boundingBox())!;
  const y = box.y + Math.min(32, box.height / 2);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: startX, y }] });
  for (let i = 1; i <= 8; i++) {
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x: startX + (endX - startX) * i / 8, y }] });
  }
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await cdp.detach();
}

async function setPhone(page: Page, dock: boolean) {
  await page.addInitScript(({ dock }) => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('pockymoe.phoneDock', dock ? 'on' : 'off');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  }, { dock });
}

test.beforeEach(async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Phone layout and touch path');
  await page.setViewportSize({ width: 390, height: 844 });
});

test('edge touch belongs to system back while interior touch opens the drawer', async ({ page, request }) => {
  const id = await threadFixture(request);
  await setPhone(page, false);
  await page.goto(`/threads/${id}`);
  const drawer = page.locator('aside.matter-sidebar');
  const main = page.locator('.matter-main');
  await expect(main).toBeVisible();
  if (await drawer.evaluate(el => el.classList.contains('is-open'))) await page.keyboard.press('Escape');
  await dragTouch(page, '.matter-main', 12, 180);
  await expect(drawer).not.toHaveClass(/is-open/);
  await dragTouch(page, '.matter-main', 60, 220);
  await expect(drawer).toHaveClass(/is-open/);
  await expect(page.locator('.matter-sidebar-scrim')).toHaveAttribute('data-open', 'true');
});

test('normal and one-handed composers sit against their intended bottom anchor', async ({ page, request }) => {
  const id = await threadFixture(request);
  const measurements: unknown[] = [];
  for (const dock of [false, true]) {
    await setPhone(page, dock);
    await page.goto(`/threads/${id}`);
    const composer = page.locator('.thread-graph-composer-host');
    await expect(composer).toBeVisible();
    const metrics = await page.evaluate(() => {
      const composer = document.querySelector<HTMLElement>('.thread-graph-composer-host')!;
      const dock = document.querySelector<HTMLElement>('.matter-phone-dock');
      const safe = document.createElement('i');
      safe.style.paddingBottom = 'env(safe-area-inset-bottom)';
      document.body.append(safe);
      const safeBottom = Number.parseFloat(getComputedStyle(safe).paddingBottom) || 0;
      safe.remove();
      const c = composer.getBoundingClientRect();
      const d = dock?.getBoundingClientRect();
      return { composerBottom: c.bottom, viewportBottom: innerHeight, safeBottom, dockTop: d?.top ?? null, gapToDock: d ? d.top - c.bottom : null };
    });
    measurements.push({ dock, ...metrics });
    if (dock) {
      expect(metrics.dockTop).not.toBeNull();
      expect(metrics.gapToDock!).toBeGreaterThanOrEqual(0);
      expect(metrics.gapToDock!).toBeLessThanOrEqual(12);
    } else {
      expect(metrics.viewportBottom - metrics.composerBottom - metrics.safeBottom).toBeLessThanOrEqual(16);
    }
    await page.getByRole('textbox', { name: 'Prompt', exact: true }).tap();
    await page.waitForTimeout(200);
    const open = await page.evaluate(() => {
      const c = document.querySelector<HTMLElement>('.thread-graph-composer-host')!.getBoundingClientRect();
      return { bottom: c.bottom, viewport: innerHeight, visual: visualViewport?.height ?? innerHeight, scroll: scrollY };
    });
    expect(open.scroll).toBe(0);
    expect(open.bottom).toBeLessThanOrEqual(open.visual + 1);
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  }
  await writeFile('/tmp/rc-ios/evidence/phone-shell/composer-metrics.json', JSON.stringify(measurements, null, 2));
});

test('phone account menu follows the avatar anchor and stays inside safe bounds', async ({ page }) => {
  test.fixme(true, 'The account gate requires an encrypted browser vault; RelayUserMenu.test.tsx covers the anchored phone menu geometry without credentials.');
  await page.route('**/relay/**', route => route.fulfill({ json: {} }));
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: true, user: { id: 'phone-owner', username: 'phone-owner', email: 'phone@example.test', role: 'user', enabled: true, createdAt: '2026-01-01T00:00:00Z' }, account: { mode: 'account', setupRequired: false, recovery: false, hasVault: true, recentAuth: true } } }));
  await page.route('**/relay/auth/challenge', route => route.fulfill({ json: { ok: true } }));
  await page.route('**/relay/portal', route => route.fulfill({ json: { devices: [], workspaces: [], shares: [] } }));
  await page.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
  await page.addInitScript(() => localStorage.setItem('pockymoe.relayMode', 'true'));
  await page.goto('/relay-devices');
  const avatar = page.locator('.product-topbar [aria-label^="Relay account menu for"]');
  await expect(avatar).toBeVisible();
  await avatar.tap();
  const menu = page.getByRole('menu');
  await expect(menu).toBeVisible();
  const anchor = await avatar.boundingBox();
  const box = await menu.boundingBox();
  expect(anchor).not.toBeNull(); expect(box).not.toBeNull();
  expect(box!.x + box!.width).toBeLessThanOrEqual(390 - 8);
  expect(box!.y).toBeGreaterThanOrEqual(8);
  expect(box!.y).toBeGreaterThan(anchor!.bottom);
  await page.screenshot({ path: `/tmp/rc-ios/evidence/phone-shell/avatar-menu.png` });
});

test('phone shell theme matrix keeps composer and dock visible', async ({ page, request }) => {
  const id = await threadFixture(request);
  for (const pack of ['classic', 'plum-pocket', 'terminal']) {
    for (const mode of ['light', 'dark']) {
      for (const dock of [false, true]) {
        await page.addInitScript(({ dock, mode, pack }) => {
          localStorage.setItem('remote-codex.locale', 'en');
          localStorage.setItem('pockymoe.phoneDock', dock ? 'on' : 'off');
          localStorage.setItem('pockymoe-theme-preset', pack);
          localStorage.setItem('remote-codex-theme-mode', mode);
        }, { dock, mode, pack });
        await page.goto(`/threads/${id}`);
        await expect(page.locator('.thread-graph-composer-host')).toBeVisible();
        await page.screenshot({ path: `/tmp/rc-ios/evidence/phone-shell/${pack}-${mode}-${dock ? 'one-hand' : 'normal'}.png` });
      }
    }
  }
});
