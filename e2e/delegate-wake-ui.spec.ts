import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

for (const preset of ['classic', 'plum-pocket']) test(`registered delegate wakes identify sources and separate reports from handling (${preset})`, async ({ page, request }, info) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Delegate wake UI' } })).json();
  const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Delegate wake UI', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(response.ok()).toBeTruthy();
  const created = await response.json(), id = created.id ?? created.thread.id;
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  await page.addInitScript(value => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem('remote-codex-theme-mode', 'dark');
    localStorage.setItem('pockymoe-theme-preset', value);
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  }, preset);
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  const first = '12345678-1234-1234-1234-123456789012', second = '12345678-1234-1234-1234-123456789013';
  const reviewer = '设备配对与工作区边界安全审阅线程';
  const startedAt = new Date(Date.now() - 300000).toISOString(), completedAt = new Date().toISOString();
  const priceEstimate = { currency: 'USD', totalUsd: 3.04, inputUsd: 1, cachedInputUsd: 0, cacheWriteInputUsd: 0, outputUsd: 2.04, pricingModelKey: 'claude-opus-5-5', pricingTierKey: 'standard' };
  const common = { status: 'completed', startedAt, completedAt, model: 'claude-opus-5-5', error: null, priceEstimate };
  const task = { ...common, id: 'wake-task', items: [{ id: 'task:user', kind: 'userMessage', text: '核实子线程结果，处理审阅意见。' }, { id: 'task:handoff', kind: 'agentMessage', text: '已安排两个子线程，等待它们汇报。', responsePhase: 'commentary' }] };
  const peerWork = { taskTurnId: task.id, messages: [
    { id: 'delegate:first', name: 'b-lab', fromThreadId: first, kind: 'result', subject: 'idle' },
    { id: 'delegate:second', name: reviewer, fromThreadId: second, kind: 'result', subject: 'blocked' },
  ] };
  const header = '[Pockymoe wake: the delegates you registered a wake for have settled]\n';
  const envelope = `${header}- b-lab (${first}): idle\n  closing message: 完整端到端测试已通过。\n\n- 登录与设备配对：通过\n- 工作区权限边界：通过\n- ${reviewer} (${second}): blocked, waiting on: approval\n  closing message: 请确认是否合并设备配对修复。\n\nVerify results before acting on them. Full transcripts: pockymoe transcript ID --limit 1`;
  const wake = { ...common, id: 'delegate-wake', hasDeferredItems: true, deferredItemCount: 1, items: [
    { id: 'wake:user', kind: 'userMessage', text: header, peerWork },
    { id: 'wake:progress', kind: 'agentMessage', text: '先核实报告，再处理合并请求。', responsePhase: 'commentary' },
    { id: 'wake:final', kind: 'agentMessage', text: '报告已核实；合并请求等待用户确认。', responsePhase: 'final' },
  ] };
  let loads = 0;
  await page.route(`**/api/threads/${id}/turns/${wake.id}/detail*`, route => {
    loads++;
    return route.fulfill({ json: { ...wake, hasDeferredItems: false, deferredItemCount: 0, items: [
      { ...wake.items[0], text: envelope, peerWork: { ...peerWork, messages: peerWork.messages.map(message => ({ ...message, text: null })) } },
      wake.items[1], { id: 'wake:command', kind: 'commandExecution', text: 'git show --stat', status: 'completed', createdAt: completedAt }, wake.items[2],
    ] } });
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, totalTurnCount: 2, activeSubagents: [], thread: { ...detail.thread, status: 'idle', activeTurnId: null }, turns: [task, wake] } }));
  await page.goto(`/threads/${id}`);
  const summary = page.locator('.thread-graph-worked-summary');
  await expect(summary).toHaveCount(1);
  await expect(page.locator('[data-role="user"]')).toHaveCount(1);
  await summary.locator('button[aria-expanded]').first().click();
  const group = page.locator('.thread-peer-work'), toggle = group.locator('.thread-peer-work-toggle');
  await expect(toggle).toContainText('子线程唤醒');
  await expect(toggle).toContainText('b-lab');
  await expect(toggle).toContainText(reviewer);
  await expect(toggle).not.toContainText('idle');
  await expect(group.locator('.thread-turn-usage-price')).toHaveText('$3.04');
  expect(loads).toBe(0);
  await toggle.scrollIntoViewIfNeeded();
  await page.screenshot({ path: info.outputPath(`${preset}-wake-collapsed.png`), scale: 'css' });
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  await expect(group.locator('.thread-peer-report')).toHaveCount(2);
  await expect(group.locator('.thread-peer-report').first()).toContainText('完整端到端测试已通过。');
  await expect(group.locator('.thread-peer-source-state').first()).toHaveText('已结束执行');
  await expect(group.locator('.thread-peer-source-state').last()).toHaveText('等待答复');
  await expect(group.locator('.thread-peer-report').last()).toContainText('请确认是否合并设备配对修复。');
  await expect(group.locator('.thread-peer-processing-label')).toHaveText('唤醒后的处理');
  await expect(group.locator('.thread-peer-work-operations')).toContainText('先核实报告，再处理合并请求。');
  await expect(group.locator('.thread-peer-work-operations')).not.toContainText('完整端到端测试已通过。');
  await expect(page.getByText('报告已核实；合并请求等待用户确认。', { exact: true })).toBeVisible();
  expect(loads).toBe(1);
  await toggle.scrollIntoViewIfNeeded();
  await page.screenshot({ path: info.outputPath(`${preset}-wake-expanded.png`), scale: 'css' });
  expect(await group.evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
  await toggle.click();
  await expect(group.locator('.thread-peer-report')).toHaveCount(0);
  await toggle.click();
  expect(loads).toBe(1);
  await summary.locator('button[aria-expanded]').first().click();
  await expect(group).toHaveCount(0);
  await expect(page.getByText('报告已核实；合并请求等待用户确认。', { exact: true })).toBeVisible();
});

for (const preset of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) test(`a registered wake and a queued wake report say where they come from (${preset}, ${mode})`, async ({ page, request }, info) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Pending wake UI' } })).json();
  const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Pending wake UI', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(response.ok()).toBeTruthy();
  const created = await response.json(), id = created.id ?? created.thread.id;
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  await page.addInitScript(([value, theme]) => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem('remote-codex-theme-mode', theme);
    localStorage.setItem('pockymoe-theme-preset', value);
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  }, [preset, mode]);
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  const oil = '14b40fee-0794-4fc4-ac30-9f98cc75adfa', macro = 'dc65117b-8c4e-47db-abf1-26dc5888a8b4';
  const report = `[Pockymoe wake: a delegate you registered a wake for is blocked]\nThis continues the task you delegated from: it adds information, not a new instruction.\n- xa-oil (${oil}): running\n- xa-macro (${macro}): blocked, waiting on: question q1\n\nVerify results before acting on them. Full transcripts: pockymoe transcript ID --limit 1`;
  const now = new Date().toISOString();
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, totalTurnCount: 0, activeSubagents: [], thread: { ...detail.thread, status: 'idle', activeTurnId: null }, turns: [],
    pendingSteers: [{ id: 'queued-wake', clientRequestId: null, turnId: '', prompt: report, delivery: 'continuation', createdAt: now }],
    pendingWakes: [{ id: 'w1', createdAt: now, members: [
      { threadId: oil, name: 'xa-oil', status: 'running' },
      { threadId: 'f7a53d7d-3aca-4510-a735-497f4e34f849', name: 'xa-regime', status: 'running' },
      { threadId: macro, name: 'xa-macro', status: 'idle' },
    ] }] } }));
  await page.goto(`/threads/${id}`);
  const wakes = page.locator('.thread-composer-pending-wakes');
  const head = wakes.locator('.thread-composer-pending-wake-head');
  await expect(head).toContainText('等待唤醒');
  await expect(head).toContainText('xa-regime');
  await expect(head.locator('[data-status="running"]')).toHaveCount(2);
  const queued = page.locator('.thread-composer-pending-row');
  await expect(queued.locator('.thread-composer-pending-source')).toHaveText('子线程唤醒');
  await expect(queued).toContainText('xa-macro 等待答复');
  await expect(queued).not.toContainText('[Pockymoe wake');
  await page.screenshot({ path: info.outputPath(`${preset}-${mode}-pending-wake.png`), scale: 'css' });
  await head.click();
  await expect(wakes.locator('.thread-composer-pending-wake-body')).toContainText('插话');
  await page.screenshot({ path: info.outputPath(`${preset}-${mode}-pending-wake-open.png`), scale: 'css' });
  expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
});
