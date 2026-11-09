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
      const composer = document.querySelector<HTMLElement>('[data-testid=chat-composer] .thread-graph-composer-shell')!;
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
      const c = document.querySelector<HTMLElement>('[data-testid=chat-composer] .thread-graph-composer-shell')!.getBoundingClientRect();
      return { bottom: c.bottom, viewport: innerHeight, visual: visualViewport?.height ?? innerHeight, scroll: scrollY };
    });
    expect(open.scroll).toBe(0);
    expect(open.bottom).toBeLessThanOrEqual(open.visual + 1);
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  }
  await writeFile('/tmp/rc-ios/evidence/phone-shell/composer-metrics.json', JSON.stringify(measurements, null, 2));
});

// However many tools a conversation has: nothing in the classic top bar overlaps or leaves the screen.
test('classic phone top bar fits, with tools in the thread tools menu', async ({ page, request }) => {
  const id = await threadFixture(request);
  await setPhone(page, false);
  for (const pack of ['classic', 'plum-pocket', 'terminal']) {
    await page.addInitScript((pack) => localStorage.setItem('pockymoe-theme-preset', pack), pack);
    for (const width of [360, 390, 402, 430]) {
      await page.setViewportSize({ width, height: 844 });
      await page.goto(`/threads/${id}`);
      await page.locator('header.matter-topbar').waitFor();
      await page.locator('header.matter-topbar .device-monitor-button').waitFor();
      const problems = await page.evaluate(() => {
        const items = [...document.querySelectorAll<HTMLElement>('header.matter-topbar :is(button, a)')].filter(item => item.offsetParent);
        const boxes = items.map(item => ({ name: item.getAttribute('aria-label') ?? item.className, box: item.getBoundingClientRect() }));
        return boxes.flatMap(({ name, box }, index) => [
          ...(box.left < 0 || box.right > innerWidth ? [`${name} leaves the screen`] : []),
          ...(index && box.left < boxes[index - 1]!.box.right - 1 ? [`${name} overlaps ${boxes[index - 1]!.name}`] : []),
        ]);
      });
      expect(problems, `${pack} at ${width}`).toEqual([]);
      await expect(page.locator('header.matter-topbar button[aria-label="Git"]')).toHaveCount(0);
    }
  }
  await page.locator('button.matter-toolbar-toggle').tap();
  await expect(page.locator('#matter-thread-tools .matter-thread-tools-panels button[aria-label="Git"]')).toBeVisible();
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
        const later=page.getByRole('button',{name:'Later',exact:true});if(await later.isVisible())await later.tap();
        await page.screenshot({ path: `/tmp/rc-ios/evidence/phone-shell/${pack}-${mode}-${dock ? 'one-hand' : 'normal'}.png` });
      }
    }
  }
});

// Android shells provide their inset through a variable when env() is zero.
test('Android variable reserves the safe area only once in the visible composer', async ({page,request}) => {
 const id=await threadFixture(request);
 for(const dock of [false,true]) {
  await setPhone(page,dock);await page.goto(`/threads/${id}`);
  await page.locator('[data-testid=chat-composer]').waitFor();
  await page.evaluate(()=>document.documentElement.style.setProperty('--android-safe-area-bottom','34px'));
  await expect.poll(()=>page.evaluate(()=>{
   const box=document.querySelector('[data-testid=chat-composer] .thread-graph-composer-shell')!.getBoundingClientRect();
   const dock=document.querySelector('.matter-phone-dock');
   if(dock) {const d=dock.getBoundingClientRect();return {gap:d.top-box.bottom,padding:parseFloat(getComputedStyle(dock).paddingBottom)};}
   return {gap:innerHeight-box.bottom,padding:34};
  })).toEqual({gap:dock?0:43,padding:34});
 }
});
