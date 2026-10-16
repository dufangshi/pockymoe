import { test, expect, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

// One-handed phone layout: status-only top bar, thread / view / More controls in a
// bottom dock, bottom sheets for threads and the actions the top bar used to hold.
test.use({ actionTimeout: 15_000 });
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

async function fixture(request: import('@playwright/test').APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(path.join(absPath, 'docs'), { recursive: true });
  await writeFile(path.join(absPath, 'docs/notes.md'), '# Notes');
  const workspace = await (
    await request.post(base + '/api/workspaces', {
      data: { absPath, label: 'Phone dock' },
    })
  ).json();
  const ids: string[] = [];
  for (const title of ['Dock first', 'Dock second']) {
    const response = await request.post(base + '/api/threads/start', {
      data: {
        workspaceId: workspace.id,
        title,
        provider: 'acp',
        agentId: 'codex',
        model: 'ios-e2e-stream',
        approvalMode: 'yolo',
      },
    });
    expect(response.ok()).toBeTruthy();
    const value = await response.json();
    ids.push(value.id ?? value.thread.id);
  }
  return ids;
}

async function swipe(page: Page, selector: string, dx: number) {
  const box = (await page.locator(selector).boundingBox())!;
  const y = box.y + box.height / 2;
  const start = dx < 0 ? box.x + box.width - 16 : box.x + 16;
  const cdp = await page.context().newCDPSession(page);
  const touch = (type: string, x: number) =>
    cdp.send('Input.dispatchTouchEvent', {
      type,
      touchPoints: type === 'touchEnd' ? [] : [{ x, y }],
    });
  await touch('touchStart', start);
  for (let i = 1; i <= 8; i++) await touch('touchMove', start + (dx * i) / 8);
  await touch('touchEnd', start + dx);
}

async function touchDrag(page: Page, selector: string, points: Array<{ x?: number; y: number }>, pause = 0, finish = true) {
  const box = (await page.locator(selector).boundingBox())!;
  const cdp = await page.context().newCDPSession(page);
  const point = (value: { x?: number; y: number }) => ({
    x: value.x ?? box.x + box.width / 2,
    y: value.y,
  });
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchStart',
    touchPoints: [point(points[0]!)],
  });
  for (const value of points.slice(1)) {
    await cdp.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [point(value)],
    });
    if (pause) await page.waitForTimeout(pause);
  }
  if (finish)
    await cdp.send('Input.dispatchTouchEvent', {
      type: 'touchEnd',
      touchPoints: [],
    });
  return cdp;
}

async function verticalSwipe(page: Page, selector: string, distance: number) {
  const box = (await page.locator(selector).boundingBox())!;
  const x = box.x + box.width / 2;
  const start = box.y + box.height / 2;
  await touchDrag(page, selector, [
    { x, y: start },
    { x: x + 2, y: start - distance / 2 },
    { x, y: start - distance },
  ]);
}

test.beforeEach(async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Phone layout');
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
});

test('phone dock replaces the top bar controls and stays reachable', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const [first, second] = await fixture(request);
  const threads = await (await request.get(`${base}/api/threads`)).json();
  await page.route(/\/api\/threads(?:\?.*)?$/, (route) =>
    route.fulfill({
      json: threads.map((thread: { id: string }) => ({
        ...thread,
        status: thread.id === second ? 'running' : 'idle',
      })),
    }),
  );
  await page.goto(`/threads/${first}`);
  const topbar = page.locator('header.matter-topbar');
  const dock = page.getByRole('navigation', { name: 'Thread navigation' });
  await expect(dock).toBeVisible();
  await expect(topbar).toHaveClass(/is-status-only/);
  await expect(topbar.getByRole('button', { name: 'Device monitor' })).toBeVisible();
  await expect(topbar.locator('.matter-connection')).toBeVisible();
  await expect(topbar.locator('.matter-topbar-usage')).toHaveCount(0);
  await expect(topbar.locator('.matter-running-chip')).toHaveText('1Running');
  await topbar.locator('.matter-running-chip').tap();
  await expect(page.getByRole('dialog', { name: 'Threads' })).toBeVisible();
  await page.keyboard.press('Escape');
  for (const label of ['Chat', 'Terminal', 'Toggle Explorer', 'Open settings', 'Toggle shortcuts sidebar']) {
    await expect(topbar.getByRole('button', { name: label, exact: true })).toHaveCount(0);
  }
  await expect(topbar.getByRole('button', { name: 'Notifications', exact: true })).toBeVisible();
  await expect(page.locator('.matter-tabs-row')).toHaveCount(0);

  // Every dock target is thumb-sized and the dock sits at the bottom edge.
  const boxes = await dock.locator('button').evaluateAll((buttons) => buttons.map((b) => b.getBoundingClientRect().toJSON() as DOMRect));
  for (const box of boxes) expect(Math.min(box.width, box.height)).toBeGreaterThanOrEqual(40);
  expect(Math.max(...boxes.map((box) => box.bottom))).toBeGreaterThan(844 - 60);

  // Files and back to chat from the dock; the ask bar sits above the dock.
  await dock.getByRole('button', { name: 'Toggle Explorer', exact: true }).tap();
  const tree = page.getByTestId('workspace-panel').getByRole('tree', { name: 'Workspace files' });
  await expect(tree).toBeVisible();
  const ask = (await page.locator('form.file-ask-bar').boundingBox())!;
  const dockBox = (await dock.boundingBox())!;
  expect(ask.y + ask.height).toBeLessThanOrEqual(dockBox.y + 1);
  await dock.getByRole('button', { name: 'Chat', exact: true }).tap();
  await expect(tree).toBeHidden();

  // The keyboard gets the room while typing.
  await page.getByRole('textbox', { name: 'Prompt', exact: true }).tap();
  await expect(dock).toBeHidden();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await expect(dock).toBeVisible();

  // Thread sheet from the thread button.
  await dock.locator('.matter-phone-thread').tap();
  const sheet = page.getByRole('dialog', { name: 'Threads' });
  await expect(sheet).toBeVisible();
  await sheet.getByRole('link', { name: /Dock second/ }).tap();
  await expect(page).toHaveURL(new RegExp(`/threads/${second}`));
  await expect(sheet).toBeHidden();
  await expect(dock.locator('.matter-phone-thread-title')).toHaveText('Dock second');

  // Settings now lives in the More sheet.
  await dock.getByRole('button', { name: 'More actions', exact: true }).tap();
  const more = page.getByRole('dialog', { name: 'More actions' });
  await more.getByRole('button', { name: 'Open settings', exact: true }).tap();
  await expect(page.getByRole('dialog', { name: /Settings/ })).toBeVisible();
  await expect(more).toBeHidden();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: /Settings/ })).toBeHidden();

  // Sideways swipe on the thread button returns to the neighbouring thread.
  await swipe(page, '.matter-phone-thread', 120);
  await expect(page).toHaveURL(new RegExp(`/threads/${first}`));
  await expect(dock.locator('.matter-phone-thread-title')).toHaveText('Dock first');
});

test('One-handed setting round trip persists the classic layout and moves subscription usage', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route('**/subscription-usage**', (route) =>
    route.fulfill({
      json: {
        usage: {
          provider: 'codex',
          authKind: 'subscription',
          observedAt: new Date().toISOString(),
          stale: false,
          windows: [
            {
              id: 'five_hour',
              label: '5h',
              durationMinutes: 300,
              usedPercent: 30,
              resetsAt: null,
            },
            {
              id: 'weekly',
              label: '7d',
              durationMinutes: 10080,
              usedPercent: 50,
              resetsAt: null,
            },
          ],
        },
      },
    }),
  );
  const [first] = await fixture(request);
  await page.goto(`/threads/${first}`);
  const dock = page.locator('nav.matter-phone-dock');
  const topbar = page.locator('header.matter-topbar');
  const composer = page.getByTestId('chat-composer');
  const statusUsage = topbar.locator('.matter-topbar-usage .thread-subscription-usage');
  const classic = async () => {
    await expect(dock).toHaveCount(0);
    await expect(topbar).not.toHaveClass(/is-status-only/);
    await expect(topbar.getByRole('button', { name: 'Toggle Explorer', exact: true })).toBeVisible();
    await expect(page.locator('.matter-tabs-row')).toBeVisible();
    await expect(composer.locator('.thread-subscription-usage')).toBeVisible();
    await expect(statusUsage).toHaveCount(0);
  };
  const oneHanded = async () => {
    await expect(dock).toBeVisible();
    await expect(topbar).toHaveClass(/is-status-only/);
    await expect(page.locator('.matter-tabs-row')).toHaveCount(0);
    await expect(statusUsage).toBeVisible();
    await expect(statusUsage).toContainText('5h');
    await expect(statusUsage).toContainText('7d');
    await expect(composer.locator('.thread-subscription-usage')).toHaveCount(0);
  };
  const settings = page.getByRole('dialog', { name: /Settings/ });
  const toggle = settings.getByRole('checkbox', {
    name: 'One-handed',
    exact: true,
  });
  await oneHanded();
  await statusUsage.tap();
  await expect(statusUsage).toHaveAttribute('aria-expanded', 'true');
  await page.keyboard.press('Escape');
  await expect(statusUsage).toHaveAttribute('aria-expanded', 'false');
  await dock.getByRole('button', { name: 'More actions', exact: true }).tap();
  await page.getByRole('dialog', { name: 'More actions' }).getByRole('button', { name: 'Open settings', exact: true }).tap();
  await settings.getByRole('tab', { name: 'Preferences', exact: true }).tap();
  await expect(toggle).toBeChecked();
  // Switching layouts remounts the settings trigger; verify the resulting layout/storage.
  await toggle.tap();
  await expect.poll(() => page.evaluate(() => localStorage.getItem('pockymoe.phoneDock'))).toBe('off');
  await expect(dock).toHaveCount(0);
  await page.keyboard.press('Escape');
  await classic();
  await page.reload();
  await classic();
  await topbar.getByRole('button', { name: 'Open settings', exact: true }).tap();
  await settings.getByRole('tab', { name: 'Preferences', exact: true }).tap();
  await expect(toggle).not.toBeChecked();
  await toggle.tap();
  await expect.poll(() => page.evaluate(() => localStorage.getItem('pockymoe.phoneDock'))).toBe('on');
  await expect(dock).toBeVisible();
  await page.keyboard.press('Escape');
  await oneHanded();
  await page.reload();
  await oneHanded();
});

test('More split picker pins both conversations in the dock and switches the shown pane', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const [first, second] = await fixture(request);
  await page.goto(`/threads/${first}`);
  const dock = page.locator('nav.matter-phone-dock');
  await dock.locator('.matter-phone-more').tap();
  const more = page.getByRole('dialog', { name: 'More actions' });
  const trigger = more.getByRole('button', {
    name: 'Split conversation',
    exact: true,
  });
  await trigger.tap();
  // The host picker uses a portal above the still-open More sheet.
  const picker = page.getByTestId('workbench-thread-picker');
  await expect(picker).toBeVisible();
  await expect(more).toBeVisible();
  await picker.locator(`[data-thread-id="${second}"]`).tap();
  await expect(more).toBeHidden();
  await expect(dock.locator('.matter-phone-pin')).toHaveCount(2);
  await expect(page.locator('.thread-pane-switch, .workbench-pane-switch-fallback')).toHaveCount(0);
  const primary = dock.locator('.matter-phone-pin[data-side="primary"]');
  const reference = dock.locator('.matter-phone-pin[data-side="reference"]');
  await expect(primary).toContainText('Dock first');
  await expect(reference).toContainText('Dock second');
  await expect(reference).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByTestId('reference-pane')).toBeVisible();
  await primary.tap();
  await expect(primary).toHaveAttribute('aria-pressed', 'true');
  await expect(reference).toHaveAttribute('aria-pressed', 'false');
  await expect(page.getByTestId('primary-pane')).toBeVisible();
  await expect(page.getByTestId('reference-pane')).toBeHidden();
  await primary.tap();
  const threads = page.getByRole('dialog', { name: 'Threads' });
  await expect(threads).toBeVisible();
  await expect(threads.getByRole('link', { name: /Dock second/ })).toBeVisible();
  await page.keyboard.press('Escape');
  await reference.tap();
  await expect(reference).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByTestId('reference-pane')).toBeVisible();
  await reference.tap();
  await expect(threads).toBeVisible();
  await expect(page.locator('.thread-pane-switch, .workbench-pane-switch-fallback')).toHaveCount(0);
});

test('turn strip follows latest, leaves following after scrolling up, and jumps previous and next', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const [id] = await fixture(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const at = new Date().toISOString();
  const turns = Array.from({ length: 4 }, (_, index) => ({
    id: `dock-turn-${index}`,
    status: 'completed',
    startedAt: at,
    completedAt: at,
    items: [
      {
        id: `prompt-${index}`,
        kind: 'userMessage',
        text: `Phone turn ${index}`,
      },
      {
        id: `reply-${index}`,
        kind: 'agentMessage',
        text: `Answer ${index}\n\n` + 'A readable paragraph in this long phone conversation.\n\n'.repeat(24),
      },
    ],
  }));
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, (socket) => {
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.route(`**/api/threads/${id}?**`, (route) => route.fulfill({ json: { ...detail, totalTurnCount: turns.length, turns } }));
  await page.goto(`/threads/${id}`);
  const nav = page.getByRole('group', {
    name: 'Timeline navigation',
    exact: true,
  });
  const latest = nav.locator('.thread-jump-latest[data-action="jump-latest"]');
  // The old badge class stays on the strip for imported theme packs.
  await expect(page.locator('.thread-jump-latest-cluster > .thread-jump-nav.thread-jump-latest-badge')).toHaveCount(1);
  await expect(nav.locator('.thread-jump-turn')).toHaveCount(2);
  await expect(nav).toHaveAttribute('data-following');
  const strip = (await nav.boundingBox())!;
  const composer = (await page.locator('.thread-graph-composer-shell').boundingBox())!;
  expect(strip.x + strip.width).toBeGreaterThan(composer.x + composer.width - 12);
  expect(strip.y + strip.height).toBeLessThanOrEqual(composer.y);
  const scroll = page.getByTestId('thread-scroll-container');
  await scroll.dispatchEvent('touchstart');
  await scroll.evaluate((el) => {
    el.scrollTop -= 400;
  });
  await expect(nav).not.toHaveAttribute('data-following');
  await latest.tap();
  await expect(nav).toHaveAttribute('data-following');
  const previous = nav.getByRole('button', {
    name: 'Jump to previous turn',
    exact: true,
  });
  const next = nav.getByRole('button', {
    name: 'Jump to next turn',
    exact: true,
  });
  const aligned = async (index: number) => expect.poll(() => page.locator(`[data-timeline-turn][data-turn-id="dock-turn-${index}"]`).evaluate((el) => Math.abs(el.getBoundingClientRect().top - document.querySelector('[data-testid="thread-scroll-container"]')!.getBoundingClientRect().top - 8))).toBeLessThan(3);
  await previous.tap();
  await aligned(3);
  await previous.tap();
  await aligned(2);
  await next.tap();
  await aligned(3);
  await previous.tap();
  await aligned(2);
  await latest.tap();
  await expect(nav).toHaveAttribute('data-following');
  await expect.poll(() => scroll.evaluate((el) => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThan(3);
});

test('the bottom panel joins the dock and the Plum status row fits at 360px', async ({ page, request }) => {
  await page.setViewportSize({ width: 360, height: 844 });
  await page.addInitScript(() => localStorage.setItem('pockymoe-theme-preset', 'plum-pocket'));
  await page.route('**/subscription-usage**', (route) =>
    route.fulfill({
      json: {
        usage: {
          provider: 'codex',
          authKind: 'subscription',
          observedAt: new Date().toISOString(),
          stale: false,
          windows: [
            {
              id: 'five_hour',
              label: '5h',
              durationMinutes: 300,
              usedPercent: 30,
              resetsAt: null,
            },
          ],
        },
      },
    }),
  );
  const [id] = await fixture(request);
  await page.goto(`/threads/${id}`);
  await expect(page.locator('html')).toHaveAttribute('data-theme-preset', 'plum-pocket');
  const dock = page.locator('nav.matter-phone-dock');
  const workbench = page.locator('.matter-workbench');
  const assertPanel = async (selector: string) => {
    const panel = page.locator(selector);
    const panelBox = (await panel.boundingBox())!;
    const dockBox = (await dock.boundingBox())!;
    expect(Math.abs(panelBox.y + panelBox.height - dockBox.y)).toBeLessThanOrEqual(1);
    const colors = await panel.evaluate((element) => {
      const style = getComputedStyle(element);
      return {
        background: style.backgroundColor,
        borderTop: style.borderTopWidth,
      };
    });
    expect(colors.background).not.toBe('rgba(0, 0, 0, 0)');
    expect(colors.borderTop).toBe('1px');
  };
  await assertPanel('.thread-graph-composer-host');
  expect(await dock.evaluate((element) => getComputedStyle(element).borderTopWidth)).toBe('0px');
  const status = page.locator('header.matter-topbar.is-status-only');
  await expect(status.locator('.matter-topbar-usage .thread-subscription-usage')).toContainText('5h');
  const statusLayout = await status.evaluate((element) => {
    const boxes = [...element.children].map((child) => child.getBoundingClientRect()).filter((box) => box.width > 0 && box.height > 0);
    return {
      scrollWidth: element.scrollWidth,
      clientWidth: element.clientWidth,
      right: Math.max(...boxes.map((box) => box.right)),
      overlaps: boxes.some((box, index) => boxes.slice(index + 1).some((other) => box.left < other.right - 1 && other.left < box.right - 1)),
    };
  });
  expect(statusLayout.scrollWidth).toBeLessThanOrEqual(statusLayout.clientWidth);
  expect(statusLayout.right).toBeLessThanOrEqual(360);
  expect(statusLayout.overlaps).toBe(false);
  await dock.getByRole('button', { name: 'Toggle Explorer', exact: true }).tap();
  await expect(page.getByRole('tree', { name: 'Workspace files' })).toBeVisible();
  await assertPanel('form.file-ask-bar');
  expect(await workbench.evaluate((element) => getComputedStyle(element).backgroundColor)).not.toBe('rgba(0, 0, 0, 0)');
});

test('touch swipes open Threads only when they move up and sheets follow and close by drag', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const [id] = await fixture(request);
  await page.goto(`/threads/${id}`);
  const dock = page.locator('nav.matter-phone-dock');
  const sheet = page.getByRole('dialog', { name: 'Threads' });
  const panes = dock.locator('.matter-phone-panes');
  const paneBox = (await panes.boundingBox())!;
  await touchDrag(page, '.matter-phone-panes', [
    { x: paneBox.x + paneBox.width / 2, y: paneBox.y + paneBox.height / 2 },
    {
      x: paneBox.x + paneBox.width / 2 + 100,
      y: paneBox.y + paneBox.height / 2 + 2,
    },
  ]);
  await expect(sheet).toBeHidden();
  await verticalSwipe(page, '.matter-phone-panes', 64);
  await expect(sheet).toBeVisible();
  await page.keyboard.press('Escape');
  await verticalSwipe(page, '.thread-graph-composer-host', 64);
  await expect(sheet).toBeVisible();
  await page.keyboard.press('Escape');

  await dock.locator('.matter-phone-thread').tap();
  await expect(sheet).toBeVisible();
  await page.waitForTimeout(250);
  const grabber = sheet.locator('.matter-phone-sheet-grabber');
  const grabberBox = (await grabber.boundingBox())!;
  const cdp = await touchDrag(
    page,
    'section[aria-label="Threads"] .matter-phone-sheet-grabber',
    [
      {
        x: grabberBox.x + grabberBox.width / 2,
        y: grabberBox.y + grabberBox.height / 2,
      },
      {
        x: grabberBox.x + grabberBox.width / 2,
        y: grabberBox.y + grabberBox.height / 2 + 16,
      },
      {
        x: grabberBox.x + grabberBox.width / 2,
        y: grabberBox.y + grabberBox.height / 2 + 32,
      },
    ],
    20,
    false,
  );
  await expect(sheet).toHaveAttribute('data-dragging', '');
  expect(await sheet.evaluate((element) => element.style.transform)).toBe('translateY(32px)');
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchEnd',
    touchPoints: [],
  });
  await expect(sheet).toBeVisible();
  expect(await sheet.evaluate((element) => element.style.transform)).toBe('');
  const body = sheet.locator('.matter-phone-sheet-body');
  await body.evaluate((element) => {
    element.scrollTop = 0;
  });
  await touchDrag(page, 'section[aria-label="Threads"] .matter-phone-sheet-body', [{ y: (await body.boundingBox())!.y + 30 }, { y: (await body.boundingBox())!.y + 42 }, { y: (await body.boundingBox())!.y + 54 }], 140);
  await expect(sheet).toBeVisible();
  await expect(sheet).not.toHaveAttribute('data-dragging');
  const closeGrabber = sheet.locator('.matter-phone-sheet-grabber');
  const closeBox = (await closeGrabber.boundingBox())!;
  await touchDrag(page, 'section[aria-label="Threads"] .matter-phone-sheet-grabber', [{ y: closeBox.y + closeBox.height / 2 }, { y: closeBox.y + closeBox.height / 2 + 140 }]);
  await expect(sheet).toBeHidden();
  await page.keyboard.press('Escape');
  await dock.getByRole('button', { name: 'More actions', exact: true }).tap();
  const more = page.getByRole('dialog', { name: 'More actions' });
  await expect(more).toBeVisible();
});

test('left hand preference mirrors the dock, composer and sheets and survives reload', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const [id] = await fixture(request);
  await page.goto(`/threads/${id}`);
  const dock = page.locator('nav.matter-phone-dock');
  await dock.getByRole('button', { name: 'More actions', exact: true }).tap();
  const more = page.getByRole('dialog', { name: 'More actions' });
  await more.getByRole('button', { name: 'Open settings', exact: true }).tap();
  const settings = page.getByRole('dialog', { name: /Settings/ });
  await settings.getByRole('tab', { name: 'Preferences', exact: true }).tap();
  const left = settings.getByRole('radio', { name: 'Left', exact: true });
  const right = settings.getByRole('radio', { name: 'Right', exact: true });
  await expect(right).toBeChecked();
  await left.tap();
  await expect(left).toBeChecked();
  await expect.poll(() => page.evaluate(() => localStorage.getItem('pockymoe.phoneHand'))).toBe('left');
  await page.keyboard.press('Escape');
  const workbench = page.locator('.matter-workbench');
  await expect(workbench).toHaveClass(/is-left-handed/);
  const moreBox = (await dock.locator('.matter-phone-more').boundingBox())!;
  const panesBox = (await dock.locator('.matter-phone-panes').boundingBox())!;
  const threadBox = (await dock.locator('.matter-phone-thread').boundingBox())!;
  expect(moreBox.x).toBeLessThan(panesBox.x);
  expect(panesBox.x).toBeLessThan(threadBox.x);
  const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true });
  const send = page.getByRole('button', { name: 'Send Prompt', exact: true });
  expect((await send.boundingBox())!.x).toBeLessThan((await prompt.boundingBox())!.x);
  const nav = page.getByRole('group', {
    name: 'Timeline navigation',
    exact: true,
  });
  const latest = nav.locator('.thread-jump-latest');
  const previous = nav.locator('.thread-jump-turn').first();
  expect((await nav.boundingBox())!.x).toBeLessThan(12);
  expect((await latest.boundingBox())!.x).toBeLessThan((await previous.boundingBox())!.x);
  await dock.getByRole('button', { name: 'Toggle Explorer', exact: true }).tap();
  const fileBar = page.locator('form.file-ask-bar');
  await expect(fileBar).toBeVisible();
  expect((await fileBar.getByRole('button', { name: 'Send', exact: true }).boundingBox())!.x).toBeLessThan((await fileBar.getByRole('textbox', { name: 'Ask the agent', exact: true }).boundingBox())!.x);
  await dock.getByRole('button', { name: 'Chat', exact: true }).tap();
  await dock.locator('.matter-phone-thread').tap();
  const threads = page.getByRole('dialog', { name: 'Threads' });
  // New thread spans the sheet's footer (the workspace drawer has its own swipe).
  await expect(threads.locator('.matter-phone-sheet-footer .matter-new-thread')).toBeVisible();
  await page.keyboard.press('Escape');
  await page.reload();
  await expect(workbench).toHaveClass(/is-left-handed/);
  await dock.getByRole('button', { name: 'More actions', exact: true }).tap();
  await expect(page.getByRole('dialog', { name: 'More actions' }).getByRole('button', { name: 'Left-handed', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('dialog', { name: 'More actions' }).getByRole('button', { name: 'Left-handed', exact: true }).tap();
  await expect.poll(() => page.evaluate(() => localStorage.getItem('pockymoe.phoneHand'))).toBe('right');
  await expect(workbench).not.toHaveClass(/is-left-handed/);
  expect((await dock.locator('.matter-phone-thread').boundingBox())!.x).toBeLessThan((await dock.locator('.matter-phone-more').boundingBox())!.x);
  await dock.getByRole('button', { name: 'More actions', exact: true }).tap();
  await page.getByRole('dialog', { name: 'More actions' }).getByRole('button', { name: 'Open settings', exact: true }).tap();
  const settingsAgain = page.getByRole('dialog', { name: /Settings/ });
  await settingsAgain.getByRole('tab', { name: 'Preferences', exact: true }).tap();
  await expect(settingsAgain.getByRole('radio', { name: 'Right', exact: true })).toBeChecked();
});

test('the dock returns after the tap that leaves a focused field, or after a plain blur settles', async ({ page, request }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const [id] = await fixture(request);
  await page.goto(`/threads/${id}`);
  const dock = page.locator('nav.matter-phone-dock');
  const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true });
  const topbar = page.locator('header.matter-topbar');
  await prompt.fill('typing keeps the dock hidden');
  await expect(dock).toBeHidden();
  await topbar.getByRole('button', { name: 'Notifications', exact: true }).tap();
  await expect(page.locator('.matter-notifications')).toBeVisible();
  await expect(dock).toBeVisible();
  await page.keyboard.press('Escape');
  await prompt.fill('blur returns the dock');
  await prompt.evaluate((element) => (element as HTMLElement).blur());
  await expect(dock).toBeHidden();
  await page.waitForTimeout(450);
  await expect(dock).toBeVisible();
});
