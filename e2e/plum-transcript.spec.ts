import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

test('Plum Pocket replies carry the agent byline and phone-sized step targets; Classic has no byline', async ({ page, request }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Plum transcript' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Plum transcript', provider: 'acp', agentId: 'claude', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const at = (minute: number, second = 0) => new Date(Date.UTC(2026, 9, 11, 9, minute, second)).toISOString();
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail, totalTurnCount: 2, activeSubagents: [], thread: { ...detail.thread, status: 'idle', activeTurnId: null },
    turns: [
      { id: 'chat', status: 'completed', startedAt: at(1), completedAt: at(1, 9), items: [
        { id: 'q', kind: 'userMessage', text: 'What does HttpOnly do?', createdAt: at(1) },
        { id: 'a', kind: 'agentMessage', text: 'Short answer without tools.', createdAt: at(1, 9) },
      ] },
      { id: 'work', status: 'completed', startedAt: at(2), completedAt: at(3), items: [
        { id: 'q2', kind: 'userMessage', text: 'Run the checks.', createdAt: at(2) },
        { id: 'c1', kind: 'commandExecution', text: 'pnpm test', status: 'completed', createdAt: at(2, 5) },
        { id: 'c2', kind: 'commandExecution', text: 'pnpm lint', status: 'completed', createdAt: at(2, 9) },
        { id: 'a2', kind: 'agentMessage', text: 'All checks pass.', createdAt: at(3) },
      ] },
    ],
  } }));
  await page.addInitScript(() => {
    localStorage.setItem('pockymoe-theme-preset', 'plum-pocket');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  await page.goto(`/threads/${id}`);
  const turns = page.getByTestId('chat-turn');
  await expect(turns).toHaveCount(2);

  // A turn without steps still introduces its reply: avatar and name come first.
  const byline = turns.nth(0).locator('.thread-transcript-agent');
  await expect(byline.locator('.matter-thread-avatar[data-agent="claude"]')).toBeVisible();
  await expect(byline.locator('.thread-transcript-agent-name')).toHaveText('Claude');
  const replyTop = await turns.nth(0).getByText('Short answer without tools.').evaluate(e => e.getBoundingClientRect().top);
  expect(await byline.evaluate(e => e.getBoundingClientRect().bottom)).toBeLessThanOrEqual(replyTop);

  // The steps open as a pocket card; on phones every row a thumb opens is at least 44px tall.
  await turns.nth(1).getByRole('button', { name: /Expand turn 2$/ }).click();
  await turns.nth(1).getByRole('button', { name: 'Expand 2 operations' }).click();
  await turns.nth(1).getByRole('button', { name: /Expand 2 command entries/ }).click();
  await expect(turns.nth(1).getByRole('button', { name: 'Open grouped command 1' })).toBeVisible();
  if (testInfo.project.name === 'mobile-chromium') {
    for (const target of [
      turns.nth(1).getByRole('button', { name: /Collapse turn 2$/ }),
      turns.nth(1).getByRole('button', { name: 'Collapse 2 operations' }),
      turns.nth(1).getByRole('button', { name: /Collapse 2 command entries/ }),
      turns.nth(1).getByRole('button', { name: 'Open grouped command 1' }),
    ]) expect((await target.boundingBox())!.height, await target.getAttribute('aria-label') ?? '').toBeGreaterThanOrEqual(44);
    expect(await page.getByTestId('thread-scroll-container').evaluate(e => e.scrollWidth <= e.clientWidth)).toBe(true);
  }

  // Classic keeps its DOM: no byline.
  await page.evaluate(() => localStorage.setItem('pockymoe-theme-preset', 'classic'));
  await page.addInitScript(() => localStorage.setItem('pockymoe-theme-preset', 'classic'));
  await page.reload();
  await expect(page.getByText('Short answer without tools.')).toBeVisible();
  await expect(page.locator('.thread-transcript-agent')).toHaveCount(0);
});
