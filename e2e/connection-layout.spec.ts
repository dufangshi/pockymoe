import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

async function openThread(page: Page, request: APIRequestContext, preset: string) {
  await page.addInitScript(preset => {
    localStorage.setItem('pockymoe-theme-preset', preset);
    localStorage.setItem('remote-codex-theme-mode', 'dark');
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (text: string) => { (window as unknown as { copied: string }).copied = text; } } });
  }, preset);
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${api}/api/workspaces`, { data: { absPath, label: 'Connection layout' } })).json();
  const result = await (await request.post(`${api}/api/threads/start`, { data: { workspaceId: workspace.id, provider: 'acp', agentId: 'claude', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = result.id ?? result.thread.id;
  const detail = await (await request.get(`${api}/api/threads/${id}`)).json();
  const text = `${Array.from({ length: 22 }, (_, i) => `Paragraph ${i + 1}: The review has verified the implementation and its visible behavior.`).join('\n\n')}\n\n**The final line remains fully readable.**`;
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail, totalTurnCount: 1, activeSubagents: [], thread: { ...detail.thread, status: 'idle', activeTurnId: null, isLoaded: true },
    turns: [{ id: 'completed', status: 'completed', startedAt: new Date().toISOString(), completedAt: new Date().toISOString(), items: [
      { id: 'prompt', kind: 'userMessage', text: 'Review the work' },
      { id: 'final', kind: 'agentMessage', responsePhase: 'final', text },
    ] }],
  } }));
  return { id, text };
}

for (const preset of ['classic', 'plum-pocket']) {
  test(`${preset}: final reply and copy control clear the floating composer and navigation`, async ({ page, request }, info) => {
    const { id, text } = await openThread(page, request, preset);
    await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
      socket.send(JSON.stringify({ type: 'supervisor.connected' }));
      socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
    });
    await page.goto(`/threads/${id}`);
    const reply = page.locator('[data-role="assistant"]').last();
    await expect(reply).toContainText('Paragraph 1:');
    const expand = reply.locator('.thread-graph-show-more');
    if (await expand.count()) await expand.click();
    const scroll = page.locator('.thread-graph-scroll-container').first();
    const tail = page.getByText('The final line remains fully readable.', { exact: true });
    const copy = reply.locator('.thread-graph-message-copy');
    const check = async () => {
      await scroll.evaluate(el => { el.scrollTop = el.scrollHeight; });
      await expect(tail).toBeVisible();
      await expect.poll(async () => {
        await scroll.evaluate(el => { el.scrollTop = el.scrollHeight; });
        const line = (await tail.boundingBox())!;
        const navigation = (await page.locator('.thread-jump-latest-cluster').boundingBox())!;
        const button = (await copy.boundingBox())!;
        return Math.max(line.y + line.height, button.y + button.height) <= navigation.y - 6;
      }).toBe(true);
      if (info.project.name === 'mobile-chromium') {
        if (await reply.locator('.thread-graph-message-bubble').getAttribute('data-touch-actions') !== 'true') await tail.tap();
      } else await tail.hover();
      await copy.click({ timeout: 3000 });
      expect(await page.evaluate(() => (window as unknown as { copied: string }).copied)).toBe(text);
    };
    await check();
    await page.screenshot({ path: info.outputPath(`${preset}-bottom-clear.png`), scale: 'css' });
    await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill('A draft that expands the composer.\nAnother line.\nOne more line.');
    await check();
  });

  test(`${preset}: reconnect state keeps the toolbar still and foregrounding probes before reconnecting`, async ({ page, request }, info) => {
    const { id } = await openThread(page, request, preset);
    let connections = 0;
    let pong = true;
    await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
      connections++;
      socket.send(JSON.stringify({ type: 'supervisor.connected' }));
      socket.onMessage(() => { if (pong) socket.send(JSON.stringify({ type: 'supervisor.pong' })); });
    });
    await page.clock.install();
    await page.goto(`/threads/${id}`);
    await expect(page.getByTestId('chat-composer')).toBeVisible();
    await expect(page.locator('.device-connection-slot')).toHaveCount(1);
    await expect(page.locator('.device-connection-button')).toHaveCount(0);
    const count = connections;
    const geometry = () => page.locator('.matter-topbar').evaluate(el => Array.from(el.querySelectorAll('button, a')).filter(e => !e.closest('.matter-connection')).map(e => {
      const r = e.getBoundingClientRect(); return [r.x, r.y, r.width, r.height];
    }));
    for (const width of info.project.name === 'mobile-chromium' ? [393, 320] : [1280]) {
      await page.setViewportSize({ width, height: 800 });
      const before = await geometry();
      await page.evaluate(() => window.dispatchEvent(new Event('offline')));
      const status = page.locator('.device-connection-button');
      await expect(status).toHaveAttribute('data-connection', 'offline');
      expect(await geometry()).toEqual(before);
      expect(await page.locator('.matter-topbar').evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
      await status.click();
      await expect(page.locator('.device-connection-popover')).toBeVisible();
      await page.getByRole('button', { name: 'Close connection information' }).click();
      await page.screenshot({ path: info.outputPath(`${preset}-${width}-offline.png`), scale: 'css' });
      await page.evaluate(() => window.dispatchEvent(new Event('online')));
      await expect(status).toHaveCount(0);
      expect(await geometry()).toEqual(before);
    }
    const beforePause = connections;
    pong = false;
    // Six seconds of latency used to tear down the socket.
    await page.clock.runFor(8000);
    expect(connections).toBe(beforePause);
    await expect(page.locator('.device-connection-button')).toHaveCount(0);
    await page.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await page.clock.fastForward(90_000);
    expect(connections).toBe(beforePause);
    await page.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await page.clock.runFor(8000);
    expect(connections).toBe(beforePause);
    pong = true;
    await page.clock.runFor(2000);
    await expect(page.locator('.device-connection-button')).toHaveCount(0);
    // A real foreground timeout must still reconnect.
    pong = false;
    await page.clock.runFor(32_000);
    await expect.poll(() => connections).toBeGreaterThan(beforePause);
    expect(connections).toBeGreaterThanOrEqual(count);
  });
}
