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
  const workspace = await (await request.post(base + '/api/workspaces', { data: { absPath, label: 'Phone dock' } })).json();
  const ids: string[] = [];
  for (const title of ['Dock first', 'Dock second']) {
    const response = await request.post(base + '/api/threads/start', { data: { workspaceId: workspace.id, title, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
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
  const touch = (type: string, x: number) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x, y }] });
  await touch('touchStart', start);
  for (let i = 1; i <= 8; i++) await touch('touchMove', start + (dx * i) / 8);
  await touch('touchEnd', start + dx);
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
  await page.route(/\/api\/threads(?:\?.*)?$/, route => route.fulfill({ json: threads.map((thread: { id: string }) => ({ ...thread, status: thread.id === second ? 'running' : 'idle' })) }));
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
  const boxes = await dock.locator('button').evaluateAll(buttons => buttons.map(b => b.getBoundingClientRect().toJSON() as DOMRect));
  for (const box of boxes) expect(Math.min(box.width, box.height)).toBeGreaterThanOrEqual(40);
  expect(Math.max(...boxes.map(box => box.bottom))).toBeGreaterThan(844 - 60);

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
  await page.route('**/subscription-usage**', route => route.fulfill({ json: { usage: {
    provider: 'codex', authKind: 'subscription', observedAt: new Date().toISOString(), stale: false,
    windows: [
      { id: 'five_hour', label: '5h', durationMinutes: 300, usedPercent: 30, resetsAt: null },
      { id: 'weekly', label: '7d', durationMinutes: 10080, usedPercent: 50, resetsAt: null },
    ],
  } } }));
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
  const toggle = settings.getByRole('checkbox', { name: 'One-handed', exact: true });
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
  const trigger = more.getByRole('button', { name: 'Split conversation', exact: true });
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
    id: `dock-turn-${index}`, status: 'completed', startedAt: at, completedAt: at,
    items: [
      { id: `prompt-${index}`, kind: 'userMessage', text: `Phone turn ${index}` },
      { id: `reply-${index}`, kind: 'agentMessage', text: `Answer ${index}\n\n` + 'A readable paragraph in this long phone conversation.\n\n'.repeat(24) },
    ],
  }));
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, totalTurnCount: turns.length, turns } }));
  await page.goto(`/threads/${id}`);
  const nav = page.getByRole('group', { name: 'Timeline navigation', exact: true });
  const latest = nav.locator('.thread-jump-latest[data-action="jump-latest"]');
  await expect(page.locator('.thread-jump-latest-badge')).toHaveCount(0);
  await expect(page.locator('.thread-jump-latest-cluster > .thread-jump-nav')).toHaveCount(1);
  await expect(nav.locator('.thread-jump-turn')).toHaveCount(2);
  await expect(nav).toHaveAttribute('data-following');
  const strip = (await nav.boundingBox())!;
  const composer = (await page.locator('.thread-graph-composer-shell').boundingBox())!;
  expect(strip.x + strip.width).toBeGreaterThan(composer.x + composer.width - 12);
  expect(strip.y + strip.height).toBeLessThanOrEqual(composer.y);
  const scroll = page.getByTestId('thread-scroll-container');
  await scroll.dispatchEvent('touchstart');
  await scroll.evaluate(el => { el.scrollTop -= 400; });
  await expect(nav).not.toHaveAttribute('data-following');
  await latest.tap();
  await expect(nav).toHaveAttribute('data-following');
  const previous = nav.getByRole('button', { name: 'Jump to previous turn', exact: true });
  const next = nav.getByRole('button', { name: 'Jump to next turn', exact: true });
  const aligned = async (index: number) => expect.poll(() => page.locator(`[data-timeline-turn][data-turn-id="dock-turn-${index}"]`).evaluate(el =>
    Math.abs(el.getBoundingClientRect().top - document.querySelector('[data-testid="thread-scroll-container"]')!.getBoundingClientRect().top - 8),
  )).toBeLessThan(3);
  await previous.tap(); await aligned(3);
  await previous.tap(); await aligned(2);
  await next.tap(); await aligned(3);
  await previous.tap(); await aligned(2);
  await latest.tap();
  await expect(nav).toHaveAttribute('data-following');
  await expect.poll(() => scroll.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThan(3);
});
