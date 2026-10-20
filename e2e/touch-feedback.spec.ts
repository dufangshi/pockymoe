import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const background = (target: Locator) => target.evaluate(element => getComputedStyle(element).backgroundColor);

async function fixture(page: Page, request: APIRequestContext) {
  if (!process.env.E2E_WORKSPACE_ROOT || !process.env.E2E_DATABASE_URL) throw new Error('Use an isolated fake Supervisor');
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT, `touch-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspaceResponse = await request.post(`${api}/api/workspaces`, { data: { absPath, label: '点击反馈' } });
  expect(workspaceResponse.ok()).toBeTruthy();
  const workspace = await workspaceResponse.json();
  const started = await request.post(`${api}/api/threads/start`, { data: { workspaceId: workspace.id, title: '点击反馈', model: 'ios-e2e-stream' } });
  expect(started.ok()).toBeTruthy();
  const created = await started.json();
  const id = created.id ?? created.thread.id;
  const detail = await (await request.get(`${api}/api/threads/${id}`)).json();
  const at = new Date().toISOString();
  let subscribed = false;
  await page.route('**/api/account/notifications/preferences', route => route.fulfill({ json: { scope: 'subscribed', devices: [], workspaces: [] } }));
  await page.route(`**/api/account/notifications/turns/${id}/press-turn`, route => {
    if (route.request().method() === 'PUT') subscribed = route.request().postDataJSON().subscribed;
    return route.fulfill({ json: { subscribed } });
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail, totalTurnCount: 1, thread: { ...detail.thread, status: 'running', activeTurnId: 'press-turn' },
    turns: [{ id: 'press-turn', status: 'inProgress', startedAt: at, items: [
      { id: 'prompt', kind: 'userMessage', text: '检查命令和订阅按钮的点击反馈。' },
      { id: 'command', kind: 'commandExecution', text: 'git status', status: 'completed', detailText: 'working tree clean', createdAt: at },
      { id: 'running', kind: 'commandExecution', text: 'pnpm test', status: 'in_progress', createdAt: at },
    ] }],
  } }));
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
  });
  return `/threads/${id}`;
}

test('触屏抬手后清除铃铛和命令行的点击背景，保留订阅和展开状态', async ({ page, request, isMobile }, testInfo) => {
  test.skip(!isMobile, '触屏回归');
  const href = await fixture(page, request);
  await page.goto(href);
  for (const preset of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
    await page.evaluate(({ preset, mode }) => {
      localStorage.setItem('pockymoe-theme-preset', preset);
      localStorage.setItem('remote-codex-theme-mode', mode);
    }, { preset, mode });
    await page.reload();
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', preset);
    const bell = page.locator('.turn-notification-button');
    await expect(bell).toBeEnabled();
    const resting = await background(bell);
    await bell.tap();
    await expect(bell).toHaveAttribute('aria-pressed', 'true');
    await expect(bell.locator('svg')).toHaveAttribute('fill', 'currentColor');
    await expect(bell).toHaveCSS('background-color', resting);
    await expect(bell).toHaveAttribute('title', '结束时通知你');
    await bell.tap();
    await expect(bell).toHaveAttribute('aria-pressed', 'false');
    await expect(bell).toHaveCSS('background-color', resting);

    const row = page.locator('.tlog-row').filter({ hasText: 'git status' });
    const head = row.locator('.tlog-row-head');
    await expect(head).toBeVisible();
    const line = row.locator('.tlog-row-line');
    const before = await background(line), beforeHead = await background(head);
    await head.tap();
    await expect(head).toHaveAttribute('aria-expanded', 'true');
    await expect(row.locator('.tlog-output')).toContainText('working tree clean');
    await expect(row.locator('.tlog-collapse')).toHaveAttribute('data-settled', 'true');
    await expect(line).toHaveCSS('background-color', before);
    await expect(head).toHaveCSS('background-color', beforeHead);
    await page.screenshot({ path: testInfo.outputPath(`${preset}-${mode}.png`), scale: 'css' });
  }
});

test('鼠标移开后恢复背景，键盘焦点仍清晰可见', async ({ page, request, isMobile }, testInfo) => {
  test.skip(isMobile, '桌面鼠标和键盘回归');
  const href = await fixture(page, request);
  await page.goto(href);
  for (const preset of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
    await page.evaluate(({ preset, mode }) => {
      localStorage.setItem('pockymoe-theme-preset', preset);
      localStorage.setItem('remote-codex-theme-mode', mode);
    }, { preset, mode });
    await page.reload();
    const bell = page.locator('.turn-notification-button');
    await expect(bell).toBeEnabled();
    await page.mouse.move(0, 0);
    const resting = await background(bell);
    await bell.hover();
    await expect.poll(() => background(bell)).not.toBe(resting);
    await page.mouse.move(0, 0);
    await expect(bell).toHaveCSS('background-color', resting);
    await bell.focus();
    await page.keyboard.press('Tab');
    await page.keyboard.press('Shift+Tab');
    await expect(bell).toBeFocused();
    await expect(bell).toHaveCSS('outline-style', 'solid');
    await page.screenshot({ path: testInfo.outputPath(`${preset}-${mode}.png`), scale: 'css' });
  }
});
