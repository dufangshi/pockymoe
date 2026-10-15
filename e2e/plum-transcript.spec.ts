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

  // The steps open as a pocket card; the work disclosure keeps its touch target; typed batches open directly.
  await turns.nth(1).getByRole('button', { name: /Expand turn 2$/ }).click();
  await expect(turns.nth(1).locator('.thread-graph-history-group-activity')).toHaveCount(0);
  await turns.nth(1).locator('.tlog-run-head').click();
  await expect(turns.nth(1).locator('.tlog-row-head').first()).toBeVisible();
  if (testInfo.project.name === 'mobile-chromium') {
    for (const target of [
      turns.nth(1).getByRole('button', { name: /Collapse turn 2$/ }),
    ]) expect((await target.boundingBox())!.height, await target.getAttribute('aria-label') ?? '').toBeGreaterThanOrEqual(44);
    for (const target of [
      turns.nth(1).locator('.tlog-run-head'),
      turns.nth(1).locator('.tlog-row-head').first(),
    ]) expect((await target.boundingBox())!.height).toBeGreaterThanOrEqual(24);
    expect(await page.getByTestId('thread-scroll-container').evaluate(e => e.scrollWidth <= e.clientWidth)).toBe(true);
  }

  // Classic keeps its DOM: no byline.
  await page.evaluate(() => localStorage.setItem('pockymoe-theme-preset', 'classic'));
  await page.addInitScript(() => localStorage.setItem('pockymoe-theme-preset', 'classic'));
  await page.reload();
  await expect(page.getByText('Short answer without tools.')).toBeVisible();
  await expect(page.locator('.thread-transcript-agent')).toHaveCount(0);
});

test('Plum Pocket step stream reads prose first: quiet toggle, small muted steps, line icons', async ({ page, request }) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Plum stream' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Plum stream', provider: 'acp', agentId: 'claude', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const at = (seconds: number) => new Date(Date.UTC(2026, 9, 11, 9, 0, seconds)).toISOString();
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail, totalTurnCount: 1, activeSubagents: [], thread: { ...detail.thread, status: 'running', activeTurnId: 'run' },
    turns: [{ id: 'run', status: 'inProgress', startedAt: at(0), completedAt: null, items: [
      { id: 'p', kind: 'userMessage', text: 'Fix the banner.', createdAt: at(0) },
      { id: 'c1', kind: 'commandExecution', text: 'rg -n banner src', status: 'completed', createdAt: at(2) },
      { id: 'r1', kind: 'fileRead', text: '/repo/src/banner.ts', status: 'completed', createdAt: at(3) },
      { id: 'm1', kind: 'agentMessage', text: 'The banner gets the raw API message, so translating it at render time covers this case.', createdAt: at(9) },
      { id: 'w1', kind: 'other', origin: 'nativeTaskNotification', taskStatus: 'completed', text: 'Run the tests', createdAt: at(11), awakenedAt: at(11) },
      { id: 'm2', kind: 'agentMessage', text: 'Tests pass.', createdAt: at(12) },
      { id: 'c2', kind: 'commandExecution', text: 'git commit', status: 'in_progress', createdAt: at(13) },
    ] }],
  } }));
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem('pockymoe-theme-preset', 'plum-pocket');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  await page.goto(`/threads/${id}`);
  const toggle = page.locator('.thread-graph-worked-summary > button').first();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  // The expanded work toggle is plain text: no pill or shadow, even right after a tap.
  expect(await toggle.evaluate((e) => [getComputedStyle(e).backgroundColor, getComputedStyle(e).boxShadow])).toEqual(['rgba(0, 0, 0, 0)', 'none']);

  const timeline = page.locator('.thread-execution-timeline').first();
  const narration = timeline.getByText('The banner gets the raw API message', { exact: false });
  await expect(narration).toBeVisible();
  const stepsLabel = timeline.locator('.tlog-run-head .tlog-summary').first();
  const style = (locator: typeof narration) => locator.evaluate((e) => {
    const css = getComputedStyle(e);
    return { size: parseFloat(css.fontSize), weight: Number(css.fontWeight) };
  });
  const prose = await style(narration);
  const steps = await style(stepsLabel);
  expect(prose.size).toBeGreaterThan(steps.size);
  expect(steps.weight).toBeLessThanOrEqual(500);
  expect(steps.weight).toBeLessThanOrEqual(prose.weight + 100);

  // Step icons are cap-height line doodles with no tile behind them.
  await stepsLabel.click();
  const icon = timeline.locator('.tlog-row .tlog-glyph').first();
  await expect(icon).toBeVisible();
  const box = (await icon.boundingBox())!;
  expect(box.width).toBeLessThanOrEqual(13);
  await expect(icon).toHaveText('$');
  await expect(icon).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
  // Wake notices follow the same muted size.
  const wake = timeline.locator('.thread-graph-task-notice').first();
  expect((await style(wake)).size).toBeLessThan(prose.size);
});
