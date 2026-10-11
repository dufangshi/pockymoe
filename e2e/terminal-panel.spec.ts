import { test, expect, type Page } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import path from 'node:path';

test.use({ actionTimeout: 15_000 });
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const screenshots = process.env.TERMINAL_SCREENSHOT_DIR;

async function workspaceThreads(request: import('@playwright/test').APIRequestContext, titles: string[]) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `terminal-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Terminal review' } })).json();
  const ids: string[] = [];
  for (const title of titles) {
    const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title, provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
    expect(response.ok()).toBeTruthy();
    const value = await response.json();
    ids.push(value.id ?? value.thread.id);
  }
  return ids;
}

async function snapshot(page: Page, name: string) {
  if (!screenshots) return;
  await mkdir(screenshots, { recursive: true });
  await page.screenshot({ path: path.join(screenshots, name), scale: 'css', animations: 'disabled' });
}

const visibleRows = (page: Page) =>
  page.locator('[data-testid="workbench-terminal-target"] [data-testid="terminal-pane"]:visible .xterm-rows');

test('desktop terminal docks below the conversations, splits, resizes and survives reload and focus changes', async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-chromium', 'Desktop panel layout');
  const theme = process.env.TERMINAL_THEME ?? 'dark';
  await page.addInitScript((mode) => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('remote-codex-theme-mode', mode);
  }, theme);
  await page.setViewportSize({ width: 1440, height: 900 });
  const [first, second] = await workspaceThreads(request, ['Terminal primary', 'Terminal peer']);
  const attaches: string[] = [];
  page.on('websocket', socket => socket.on('framesent', frame => {
    const text = String(frame.payload);
    if (text.includes('"shell.attach"')) attaches.push(JSON.parse(text).shellId);
  }));
  await page.goto(`/threads/${first}`);
  const rail = page.getByRole('navigation', { name: 'Workspace tools', exact: true });
  const primary = page.getByTestId('primary-pane');
  await expect(primary.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
  await rail.getByRole('button', { name: 'Terminal', exact: true }).click();
  const panel = page.getByTestId('workbench-bottom-panel');
  await expect(panel).toBeVisible();
  await expect(panel.getByTestId('terminal-target')).toContainText('Terminal primary');
  // An explicit open creates and focuses the first terminal.
  await expect(visibleRows(page)).toContainText('$', { timeout: 20_000 });
  await page.keyboard.type('echo panel-marker');
  await page.keyboard.press('Enter');
  await expect(visibleRows(page)).toContainText('panel-marker\n');

  // In-flow: the conversation and its prompt end where the panel begins.
  const assertDocked = async () => {
    const panelBox = (await panel.boundingBox())!, primaryBox = (await primary.boundingBox())!;
    expect(primaryBox.y + primaryBox.height).toBeLessThanOrEqual(panelBox.y + 1);
    const composer = (await primary.getByRole('textbox', { name: 'Prompt', exact: true }).boundingBox())!;
    expect(composer.y + composer.height).toBeLessThanOrEqual(panelBox.y);
  };
  await assertDocked();
  // Files keep their full-height column and width handle beside the panel.
  await rail.getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  const drawer = page.locator('.workbench-tool-drawer').filter({ has: page.getByTestId('workbench-close-files') });
  await expect(drawer).toBeVisible();
  await expect(drawer.getByRole('separator', { name: 'Resize Explorer', exact: true })).toBeVisible();
  const drawerBox = (await drawer.boundingBox())!, panelBox = (await panel.boundingBox())!;
  expect(panelBox.x + panelBox.width).toBeLessThanOrEqual(drawerBox.x + 1);
  expect(drawerBox.height).toBeGreaterThan(panelBox.height + 200);
  await assertDocked();
  await page.getByTestId('workbench-close-files').click();

  await panel.getByTestId('terminal-new').click();
  await expect(page.getByRole('tab')).toHaveCount(2);
  await panel.getByTestId('terminal-split').click();
  const tabs = page.getByRole('tablist', { name: 'Terminals', exact: true }).getByRole('tab');
  await expect(tabs).toHaveCount(3);
  await expect(tabs.nth(2)).toHaveAttribute('aria-label', 'Terminal 3, split 2 of 2');
  await expect(tabs.nth(2)).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2);
  await snapshot(page, `desktop-terminal-split-${theme}.png`);

  // Drag the panel edge and the split boundary; both persist.
  const sash = (await page.getByTestId('workbench-panel-sash').boundingBox())!;
  const heightBefore = (await panel.boundingBox())!.height;
  await page.mouse.move(sash.x + sash.width / 2, sash.y + sash.height / 2);
  await page.mouse.down();
  await page.mouse.move(sash.x + sash.width / 2, sash.y - 140, { steps: 8 });
  await page.mouse.up();
  await expect.poll(async () => (await panel.boundingBox())!.height).toBeGreaterThan(heightBefore + 130);
  await assertDocked();
  const paneSash = (await page.getByTestId('terminal-pane-sash').boundingBox())!;
  await page.mouse.move(paneSash.x + 0.5, paneSash.y + paneSash.height / 2);
  await page.mouse.down();
  await page.mouse.move(paneSash.x + 120, paneSash.y + paneSash.height / 2, { steps: 8 });
  await page.mouse.up();
  const widths = () => page.locator('[data-testid="terminal-pane"]:visible').evaluateAll(nodes => nodes.map(node => Math.round(node.getBoundingClientRect().width)));
  await expect.poll(async () => { const [left, right] = await widths(); return left! - right!; }).toBeGreaterThan(200);
  const dragged = { height: (await panel.boundingBox())!.height, widths: await widths() };
  await snapshot(page, `desktop-terminal-dragged-${theme}.png`);

  await page.reload();
  await expect(panel).toBeVisible();
  await expect.poll(async () => (await panel.boundingBox())!.height).toBeCloseTo(dragged.height, 0);
  await expect.poll(widths).toEqual(dragged.widths);
  // Reattaching replays the PTY output instead of a blank terminal.
  await tabs.nth(0).click();
  await expect(visibleRows(page)).toContainText('panel-marker');

  await panel.getByTestId('terminal-maximize').click();
  await expect(panel).toHaveClass(/is-maximized/);
  await expect(rail.getByRole('button', { name: 'Chat', exact: true })).toHaveAttribute('aria-pressed', 'false');
  await rail.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(panel).not.toHaveClass(/is-maximized/);
  await assertDocked();

  // Split chats: the terminal follows the last focused conversation without
  // unmounting the other one's running terminal.
  await page.getByTestId('workbench-split-trigger').click();
  await page.getByTestId('workbench-thread-picker').locator(`[data-thread-id="${second}"]`).click();
  const reference = page.getByTestId('reference-pane');
  await reference.getByRole('textbox', { name: 'Prompt', exact: true }).click();
  const target = page.getByTestId('workbench-terminal-target');
  await expect(target).toHaveAttribute('data-thread', second!);
  await expect(panel.getByText('No terminal in this conversation yet')).toBeVisible();
  await expect(page.getByTestId('workbench-terminal-cached')).toHaveAttribute('data-thread', first!);
  await expect(reference.getByRole('textbox', { name: 'Prompt', exact: true })).toBeFocused();
  const shells = (await (await request.get(`${base}/api/threads/${second}/shell`)).json()).shells;
  expect(shells).toHaveLength(0);
  const attachCount = attaches.length;
  await primary.getByRole('textbox', { name: 'Prompt', exact: true }).click();
  await expect(target).toHaveAttribute('data-thread', first!);
  await expect(visibleRows(page)).toContainText('panel-marker');
  expect(attaches.slice(attachCount)).toEqual([]);
  await snapshot(page, `desktop-terminal-dual-chat-${theme}.png`);
  // Hiding keeps terminals running; reopening for the peer creates its first one.
  await panel.getByTestId('workbench-close-tools').click();
  await expect(panel).toBeHidden();
  await reference.getByRole('textbox', { name: 'Prompt', exact: true }).click();
  await rail.getByRole('button', { name: 'Terminal', exact: true }).click();
  await expect(target).toHaveAttribute('data-thread', second!);
  await expect(visibleRows(page)).toContainText('$', { timeout: 20_000 });
  await expect.poll(async () => (await (await request.get(`${base}/api/threads/${second}/shell`)).json()).shells.length).toBe(1);
  expect((await (await request.get(`${base}/api/threads/${first}/shell`)).json()).shells).toHaveLength(3);
});

test('phone terminal keeps the prompt and newest reply visible, resizes by touch and switches terminals', async ({ page, request, context }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Touch layout');
  const theme = process.env.TERMINAL_THEME ?? 'dark';
  await page.addInitScript((mode) => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('remote-codex-theme-mode', mode);
  }, theme);
  const [id] = await workspaceThreads(request, ['Phone terminal']);
  const sent = await request.post(`${base}/api/threads/${id}/prompt`, { data: { prompt: 'Summarize the terminal layout for a phone.' } });
  expect(sent.ok()).toBeTruthy();
  await page.goto(`/threads/${id}`);
  const reply = page.locator('.thread-graph-message').filter({ hasText: 'Summarize the terminal layout' }).last();
  await expect(reply).toBeVisible();
  await page.locator('.matter-topbar').getByRole('button', { name: 'Terminal', exact: true }).tap();
  const panel = page.getByTestId('workbench-bottom-panel');
  await expect(panel).toBeVisible();
  await expect(visibleRows(page)).toContainText('$', { timeout: 20_000 });
  const viewport = page.viewportSize()!;
  const assertUsable = async () => {
    const panelBox = (await panel.boundingBox())!;
    const prompt = (await page.getByTestId('primary-pane').getByRole('textbox', { name: 'Prompt', exact: true }).boundingBox())!;
    expect(prompt.y + prompt.height).toBeLessThanOrEqual(panelBox.y);
    expect(panelBox.y + panelBox.height).toBeLessThanOrEqual(viewport.height + 1);
    // The newest reply stays readable above the floating prompt.
    const replyBox = (await reply.boundingBox())!;
    expect(replyBox.y + replyBox.height).toBeGreaterThan((await page.getByTestId('primary-pane').boundingBox())!.y);
    expect(replyBox.y).toBeLessThan(prompt.y);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(viewport.width);
  };
  await assertUsable();
  await expect(page.getByRole('toolbar', { name: 'Terminal controls' })).toBeVisible();

  // A phone opens one terminal, with direct xterm input and no extra input form.
  await expect(panel.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(1);
  await panel.locator('.shell-pane-host:visible').tap({ position: { x: 50, y: 35 } });
  await expect(panel.locator('.xterm-helper-textarea:visible')).toBeFocused();
  await page.keyboard.type('echo phone-direct-input');
  await page.keyboard.press('Enter');
  await expect(visibleRows(page)).toContainText('phone-direct-input');
  await expect(panel.locator('input, textarea:not(.xterm-helper-textarea)')).toHaveCount(0);
  await snapshot(page, `mobile-terminal-direct-${theme}.png`);

  await panel.getByTestId('terminal-new').tap();
  await panel.getByRole('button', { name: /^Switch terminal/ }).tap();
  const menu = page.getByRole('menu', { name: 'Switch terminal', exact: true });
  await expect(menu.getByRole('menuitemradio')).toHaveCount(2);
  const menuBox = (await menu.boundingBox())!;
  expect(menuBox.x).toBeGreaterThanOrEqual(0);
  expect(menuBox.x + menuBox.width).toBeLessThanOrEqual(viewport.width);
  await snapshot(page, `mobile-terminal-switcher-${theme}.png`);
  await menu.getByRole('menuitemradio', { name: /Terminal 1/ }).tap();
  await expect(panel.getByRole('button', { name: 'Switch terminal: Terminal 1', exact: true })).toBeVisible();
  // Splits stack vertically on a phone.
  await panel.getByTestId('terminal-more').tap();
  await page.getByRole('menuitem', { name: 'Split terminal', exact: true }).tap();
  await expect(page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2);
  await expect(page.locator('.terminal-groups')).toHaveAttribute('data-orientation', 'vertical');
  await snapshot(page, `mobile-terminal-split-${theme}.png`);
  // Touch-drag the visible grab bar (last: Chrome eats a tap right after a
  // synthetic drag as a fling stop).
  const grab = (await page.getByTestId('workbench-panel-sash').boundingBox())!;
  const before = (await panel.boundingBox())!.height;
  const cdp = await context.newCDPSession(page);
  const touch = (type: string, y?: number) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: y === undefined ? [] : [{ x: grab.x + grab.width / 2, y }] });
  await touch('touchStart', grab.y + grab.height / 2);
  for (let step = 1; step <= 5; step++) await touch('touchMove', grab.y + grab.height / 2 - step * 20);
  await touch('touchEnd');
  await expect.poll(async () => (await panel.boundingBox())!.height).toBeGreaterThan(before + 60);
  await assertUsable();
});

test('phone terminal reserves space for a visual-only keyboard without relying on browser panning', async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-chromium', 'Mobile keyboard layout');
  await page.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
  const [id] = await workspaceThreads(request, ['Keyboard avoidance']);
  await page.goto(`/threads/${id}`);
  await page.locator('.matter-topbar').getByRole('button', { name: 'Terminal', exact: true }).tap();
  const panel = page.getByTestId('workbench-bottom-panel');
  await expect(visibleRows(page)).toContainText('$');
  await panel.locator('.shell-pane-host:visible').tap({ position: { x: 50, y: 35 } });
  const before = (await panel.boundingBox())!;
  await page.evaluate(() => {
    // Model the actual failure: keyboard overlays a fixed layout viewport,
    // and the browser never pans the page to rescue the terminal cursor.
    const viewport = Object.assign(new EventTarget(), {
      height: window.innerHeight - 300, offsetTop: 0, scale: 1,
    });
    Object.defineProperty(window, 'visualViewport', { configurable: true, value: viewport });
    window.dispatchEvent(new Event('resize'));
  });
  const assertAboveKeyboard = async () => {
    const bounds = await page.evaluate(() => {
      const panel = document.querySelector('[data-testid="workbench-bottom-panel"]')!.getBoundingClientRect();
      const toolbar = document.querySelector('.shell-touch-controls')!.getBoundingClientRect();
      const cursor = document.querySelector('.terminal-pane:not([hidden]) .xterm-cursor')!.getBoundingClientRect();
      return { panelBottom: panel.bottom, toolbarTop: toolbar.top, toolbarBottom: toolbar.bottom, cursorBottom: cursor.bottom, visibleBottom: window.visualViewport!.height };
    });
    expect(bounds.panelBottom).toBeLessThanOrEqual(bounds.visibleBottom + 1);
    expect(bounds.toolbarBottom).toBeLessThanOrEqual(bounds.visibleBottom + 1);
    expect(bounds.cursorBottom).toBeLessThanOrEqual(bounds.toolbarTop + 1);
  };
  await expect(async () => { await assertAboveKeyboard(); }).toPass({ timeout: 5000 });
  await page.keyboard.type('seq 1 40');
  await page.keyboard.press('Enter');
  await expect(visibleRows(page)).toContainText('40');
  await expect(async () => { await assertAboveKeyboard(); }).toPass({ timeout: 5000 });
  await panel.getByTestId('terminal-maximize').tap();
  await expect(async () => { await assertAboveKeyboard(); }).toPass({ timeout: 5000 });
  await panel.getByTestId('terminal-maximize').tap();
  await page.evaluate(() => {
    Object.assign(window.visualViewport!, { height: window.innerHeight });
    window.dispatchEvent(new Event('resize'));
  });
  await expect.poll(async () => (await panel.boundingBox())!.height).toBeCloseTo(before.height, 0);
  await expect.poll(async () => (await panel.boundingBox())!.y).toBeCloseTo(before.y, 0);
});

test('dragging the terminal panel keeps the buffer steady and resizes the shell once', async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-chromium', 'Desktop panel sash');
  await page.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
  await page.setViewportSize({ width: 1280, height: 860 });
  const [id] = await workspaceThreads(request, ['Terminal drag']);
  const resizes: string[] = [];
  page.on('websocket', socket => socket.on('framesent', frame => {
    const text = String(frame.payload);
    if (text.includes('"shell.resize"')) resizes.push(text);
  }));
  await page.goto(`/threads/${id}`);
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
  await page.getByRole('navigation', { name: 'Workspace tools', exact: true }).getByRole('button', { name: 'Terminal', exact: true }).click();
  const panel = page.getByTestId('workbench-bottom-panel');
  await expect(visibleRows(page)).toContainText('$', { timeout: 20_000 });
  await page.keyboard.type('echo drag-marker');
  await page.keyboard.press('Enter');
  await expect(visibleRows(page)).toContainText('drag-marker\n');
  await page.waitForTimeout(500);
  const lines = async () => (await visibleRows(page).innerText()).split('\n').filter(line => line.trim()).length;
  const linesBefore = await lines();
  resizes.length = 0;
  // Count frames in which the terminal shows no text while the panel moves.
  await page.evaluate(() => {
    const state = window as unknown as { blankFrames: number; watching: boolean };
    state.blankFrames = 0;
    state.watching = true;
    const tick = () => {
      if (!state.watching) return;
      const rows = document.querySelector('[data-testid="workbench-terminal-target"] [data-testid="terminal-pane"] .xterm-rows');
      if (!rows?.textContent?.trim()) state.blankFrames += 1;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  const sash = (await page.getByTestId('workbench-panel-sash').boundingBox())!;
  const x = sash.x + sash.width / 2;
  const y = sash.y + sash.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  for (const offset of [-60, -120, -180, -90, 0, 60, -40, -150]) {
    await page.mouse.move(x, y + offset, { steps: 6 });
  }
  await page.mouse.up();
  await page.waitForTimeout(800);
  const blankFrames = await page.evaluate(() => {
    const state = window as unknown as { blankFrames: number; watching: boolean };
    state.watching = false;
    return state.blankFrames;
  });
  expect(blankFrames).toBe(0);
  expect(resizes.length).toBeLessThanOrEqual(2);
  expect((await panel.boundingBox())!.height).toBeGreaterThan(200);
  await expect(visibleRows(page)).toContainText('drag-marker\n');
  expect(await lines()).toBeLessThanOrEqual(linesBefore);
});
