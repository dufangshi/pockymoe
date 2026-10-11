import { test, expect, type APIRequestContext, type WebSocketRoute } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

test('cached Claude billing separates total input from the uncached tail on touch screens', async ({ page, request }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale','zh-CN');
    localStorage.setItem('remote-codex-theme-mode','dark');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin,'local:owner'])}`,JSON.stringify({welcomeDismissed:true,completed:[]}));
  });
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const completedAt = new Date().toISOString();
  const total = { inputTokens:15121954,cachedInputTokens:14045452,cacheWriteInputTokens:1076364,cacheWriteOneHourInputTokens:1076364,outputTokens:56095,reasoningOutputTokens:24378,totalTokens:15178049 };
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({type:'supervisor.connected'}));
    socket.onMessage(() => socket.send(JSON.stringify({type:'supervisor.pong'})));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({json:{
    ...detail,totalTurnCount:1,activeSubagents:[],thread:{...detail.thread,status:'idle',activeTurnId:null},
    turns:[{id:'cached-claude',status:'completed',startedAt:new Date(Date.now()-1200000).toISOString(),completedAt,model:'claude-opus-5-5',reasoningEffort:'high',
      tokenUsage:{total,last:total},priceEstimate:{inputUsd:0.000552,cachedInputUsd:2.8090904,cacheWriteInputUsd:8.610912,outputUsd:1.1219,totalUsd:12.5424544,currency:'USD',pricingModelKey:'claude-opus-5-5',pricingTierKey:'standard'},
      items:[{id:'prompt',kind:'userMessage',text:'核对含图片的调用计费'},{id:'reply',kind:'agentMessage',text:'输入合计包含未缓存输入、缓存读取和缓存写入。'}]}],
  }}));
  await page.goto(`/threads/${id}`);
  const price = page.locator('.thread-graph-worked-summary .thread-turn-usage-price');
  await expect(price).toHaveText('$12.5');
  await price.tap();
  const popup = page.locator('[data-slot="tooltip-content"]');
  await expect(popup).toBeVisible();
  for (const name of ['输入合计','未缓存输入','缓存读取','缓存写入','输出','推理']) await expect(popup.getByText(name,{exact:name!=='缓存写入'}).first()).toBeVisible();
  await expect(popup.getByLabel('输入合计：15,121,954 个 token',{exact:true}).first()).toBeVisible();
  await expect(popup.getByLabel('未缓存输入：138 个 token',{exact:true}).first()).toBeVisible();
  await expect(popup.getByLabel('缓存写入：1,076,364 个 token',{exact:true}).first()).toBeVisible();
  await expect(popup.getByLabel('缓存读取：14,045,452 个 token',{exact:true}).first()).toBeVisible();
  const box = (await popup.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x+box.width).toBeLessThanOrEqual(page.viewportSize()!.width+1);
  expect(await popup.evaluate(el => el.scrollWidth-el.clientWidth)).toBeLessThanOrEqual(1);
  await page.screenshot({path:testInfo.outputPath('claude-input-breakdown.png'),scale:'css'});
  await price.tap();
  await expect(popup).not.toBeVisible();
  await price.tap();
  await expect(popup).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(popup).not.toBeVisible();
});

async function createThread(request: APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Reading regression' } })).json();
  const response = await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Reading regression', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(response.ok()).toBeTruthy();
  const value = await response.json();
  return value.id ?? value.thread.id as string;
}

test('interrupted history keeps one turn status without repeated message circles or blank event badges', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const startedAt = new Date(Date.now() - 60_000).toISOString();
  const completedAt = new Date().toISOString();
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({type:'supervisor.connected'}));
    socket.onMessage(() => socket.send(JSON.stringify({type:'supervisor.pong'})));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({json:{
    ...detail, totalTurnCount:1, activeSubagents:[],
    thread:{...detail.thread,status:'idle',activeTurnId:null},
    turns:[{id:'interrupted-history',status:'interrupted',startedAt,completedAt,items:[
      {id:'prompt',kind:'userMessage',text:'Check background recording'},
      {id:'checkpoint-one',kind:'agentMessage',text:'Background progress is saved.',status:'interrupted'},
      {id:'compaction',kind:'contextCompaction',text:'Context compaction',status:'interrupted'},
      {id:'checkpoint-two',kind:'agentMessage',text:'The update will resume this thread.',status:'interrupted'},
      {id:'failure',kind:'generic',text:'A verification command failed.',status:'failed'},
    ]}],
  }}));
  await page.goto(`/threads/${id}`);
  const summary = page.locator('.thread-graph-worked-summary');
  await expect(summary).toContainText('Interrupted');
  await summary.getByRole('button',{name:/Expand turn 1$/}).click();
  await expect(page.getByText('Background progress is saved.',{exact:true})).toBeVisible();
  await expect(page.getByText('The update will resume this thread.',{exact:true})).toBeVisible();
  await expect(page.locator('[data-role="assistant"] .thread-graph-message-status')).toHaveCount(0);
  await expect(page.locator('.thread-graph-event-context')).toContainText('Context compacted');
  await expect(page.locator('.thread-graph-event-context .thread-graph-tool-badge')).toHaveCount(0);
  const failure = page.locator('.thread-graph-event-generic .thread-graph-status-label');
  await expect(failure).toHaveText('Failed');
  expect((await failure.boundingBox())!.width).toBeGreaterThan(15);
  await page.screenshot({path:testInfo.outputPath('clean-history.png'),scale:'css'});
});

test('native task notifications use compact prose and expand without mobile overflow', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const startedAt = new Date(Date.now() - 60_000).toISOString();
  const completedAt = new Date().toISOString();
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem('remote-codex-theme-mode', 'dark');
  });
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    socket.send(JSON.stringify({type:'supervisor.connected'}));
    socket.onMessage(() => socket.send(JSON.stringify({type:'supervisor.pong'})));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({json:{
    ...detail, totalTurnCount:1, activeSubagents:[],
    thread:{...detail.thread,status:'idle',activeTurnId:null},
    turns:[{id:'task-notices',status:'completed',startedAt,completedAt,items:[
      {id:'prompt',kind:'userMessage',text:'检查后台验证结果'},
      {id:'reply',kind:'agentMessage',text:'后台检查已完成，以下是各项任务的结果。'},
      {id:'notice',kind:'generic',origin:'nativeTaskNotification',taskStatus:'completed',status:'interrupted',createdAt:completedAt,
        text:'修复文件树复制行为，重新构建 UI 并运行桌面浏览器回归验证'},
      {id:'failure-notice',kind:'generic',origin:'nativeTaskNotification',taskStatus:'failed',status:'completed',createdAt:completedAt,
        text:'验证移动端布局与文件编辑的交互'},
      {id:'note',kind:'generic',text:'已保存检查结果。',status:'completed'},
    ]}],
  }}));
  await page.goto(`/threads/${id}`);
  await page.locator('.thread-graph-worked-summary button[aria-expanded]').click();
  const notice = page.locator('.thread-graph-task-notice').first();
  await expect(notice.locator('.thread-graph-task-notice-toggle')).toContainText('已唤醒');
  await expect(page.locator('.thread-graph-task-notice.is-failed')).toContainText('后台任务失败');
  await expect(notice.locator('.thread-graph-task-notice-detail')).toHaveCount(0);
  expect(await notice.locator('.thread-graph-task-notice-toggle').evaluate(el => getComputedStyle(el).fontSize)).toBe('12px');
  const noteBody = page.locator('.thread-graph-history-event-note');
  expect(await noteBody.evaluate(el => getComputedStyle(el).fontSize)).toBe('12px');
  expect(await noteBody.evaluate(el => getComputedStyle(el).fontFamily)).toBe(await notice.evaluate(el => getComputedStyle(el).fontFamily));
  for (const row of await page.locator('.thread-graph-task-notice-row').all()) {
    expect(await row.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  }
  await page.screenshot({path:testInfo.outputPath('task-notifications-collapsed.png'),scale:'css'});
  await notice.locator('.thread-graph-task-notice-toggle').click();
  await expect(notice.locator('.thread-graph-task-notice-toggle')).toHaveAttribute('aria-expanded','true');
  await expect(notice.locator('.thread-graph-task-notice-detail')).toContainText('重新构建 UI');
  await expect(notice.locator('.thread-graph-task-notice-detail time')).toHaveAttribute('datetime',completedAt);
  await page.screenshot({path:testInfo.outputPath('task-notifications-expanded.png'),scale:'css'});
});

test('opening a detached thread automatically connects once and hides the healthy indicator', async ({ page, request }) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  let loaded = false;
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, thread: { ...detail.thread, isLoaded: loaded } } }));
  await page.route(`**/api/threads/${id}/resume`, async route => {
    loaded = true;
    await route.fulfill({ json: { ...detail, thread: { ...detail.thread, isLoaded: true } } });
  });
  const connections: string[] = [];
  page.on('request', req => { if (req.url().endsWith(`/api/threads/${id}/resume`)) connections.push(req.url()); });
  await page.goto(`/threads/${id}`);
  await expect.poll(() => loaded).toBe(true);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeEnabled();
  await expect(page.locator('.device-connection-button')).toHaveCount(0);
  expect(connections).toHaveLength(1);
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeEnabled();
  await expect(page.locator('.device-connection-button')).toHaveCount(0);
  expect(connections).toHaveLength(1);
});

test('overlapping live steps stay stable on expansion and idle output keeps freshness dots with hover and tap details', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const startedAt = new Date(Date.now() - 60_000).toISOString();
  const prompt = { id: 'prompt', kind: 'userMessage', text: 'Check progress' };
  const reply = { id: 'reply', kind: 'agentMessage', text: 'Progress report received', updatedAt: startedAt };
  const commands = [
    { id: 'command-one', kind: 'commandExecution', text: 'read inbox', status: 'completed', updatedAt: startedAt },
    { id: 'command-two', kind: 'commandExecution', text: 'read report', status: 'completed', updatedAt: startedAt },
  ];
  let count = 1;
  let completed = false;
  let socket: WebSocketRoute | undefined;
  const turn = { id: 'coalesced-turn', status: 'inProgress', startedAt, items: [prompt, reply] };
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, current => {
    socket = current;
    current.send(JSON.stringify({ type: 'supervisor.connected' }));
    current.onMessage(() => current.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail, totalTurnCount: 1, activeSubagents: [],
    thread: { ...detail.thread, status: completed ? 'idle' : 'running', activeTurnId: completed ? null : turn.id },
    turns: [{ ...turn, status: completed ? 'completed' : 'inProgress', hasDeferredItems: true, deferredItemCount: count }],
  } }));
  await page.route(`**/api/threads/${id}/turns/${turn.id}/detail`, route => route.fulfill({ json: {
    ...turn, items: [prompt, ...commands.slice(0, count), reply], hasDeferredItems: false, deferredItemCount: 0,
  } }));
  const emit = (type: string, payload = {}) => socket!.send(JSON.stringify({ type, threadId: id, timestamp: startedAt, payload }));
  await page.goto(`/threads/${id}`);
  const summary = page.locator('.thread-graph-worked-summary');
  const steps = summary.locator('.thread-execution-step-count');
  await expect(steps).toHaveText('1 steps');
  const progress = page.locator('.thread-progress-indicator');
  await expect(progress).toHaveAttribute('data-progress-freshness', 'stale');
  await expect(page.locator('.thread-progress-age')).toHaveCount(0);
  if (testInfo.project.name === 'mobile-chromium') await progress.tap();
  else await progress.hover();
  const tooltip = page.getByRole('tooltip');
  await expect(tooltip).toContainText(/Last progress · [5-9]\ds ago/);
  await expect(tooltip).toContainText('Last activity ');
  await page.getByRole('textbox', { name: 'Prompt' }).click();
  await expect(tooltip).toHaveCount(0);
  await expect(page.locator('.thread-graph-turn-footer .animate-pulse')).toHaveCount(3);
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toBeVisible();
  emit('thread.item.completed', { turnId: turn.id, item: commands[0] });
  await expect(progress).toHaveAttribute('data-progress-freshness', 'recent');
  await expect(steps).toHaveText('1 steps');
  await summary.getByRole('button', { name: /Expand turn 1$/ }).click();
  await expect(page.getByText('read inbox', { exact: true })).toBeVisible();
  await expect(steps).toHaveText('1 steps');
  await summary.getByRole('button', { name: /Collapse turn 1$/ }).click();
  count = 2;
  emit('thread.item.completed', { turnId: turn.id, item: commands[1] });
  emit('thread.updated');
  await expect(steps).toHaveText('2 steps');
  await summary.getByRole('button', { name: /Expand turn 1$/ }).click();
  await expect(page.getByText('read report', { exact: true })).toBeVisible();
  await expect(steps).toHaveText('2 steps');
  await page.reload();
  await expect(steps).toHaveText('2 steps');
  await expect(progress).toBeVisible();
  emit('thread.turn.token.updated', { turnId: turn.id, tokenUsage: { total: { inputTokens: 100, outputTokens: 10, totalTokens: 110, cachedInputTokens: 0 } } });
  await expect(progress).toHaveAttribute('data-progress-freshness', 'recent');
  await page.clock.install();
  await page.clock.fastForward(7_000);
  await expect(progress).toHaveAttribute('data-progress-freshness', 'quiet');
  await page.clock.fastForward(15_000);
  await expect(progress).toHaveAttribute('data-progress-freshness', 'stale');
  if (process.env.PROGRESS_SCREENSHOT_DIR) {
    await mkdir(process.env.PROGRESS_SCREENSHOT_DIR, { recursive: true });
    if (testInfo.project.name === 'mobile-chromium') await progress.tap();
    else await progress.hover();
    await expect(tooltip).toBeVisible();
    await page.screenshot({ path: path.join(process.env.PROGRESS_SCREENSHOT_DIR, `progress-${testInfo.project.name}.png`), scale: 'css' });
  }
  completed = true;
  emit('thread.turn.completed', { turnId: turn.id, status: 'completed' });
  emit('thread.updated');
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toHaveCount(0);
  await expect(page.locator('.thread-graph-turn-footer')).toHaveCount(0);
});

test('background agents remain visible after the main reply, survive reload, and clear on completion', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const startedAt = new Date(Date.now() - 60_000).toISOString();
  const turn = {
    id: 'background-review-turn', status: 'inProgress', startedAt,
    model: 'claude-opus-5-5', reasoningEffort: 'high',
    tokenUsage: { total: { totalTokens: 3_700_000, inputTokens: 3_695_000, outputTokens: 5_000, cachedInputTokens: 3_600_000 } },
    priceEstimate: { currency: 'USD', totalUsd: 0.86 },
    items: [
      { id: 'prompt', kind: 'userMessage', text: 'Review the results.' },
      { id: 'launch', kind: 'agentToolCall', text: 'Independent review', status: 'completed' },
      { id: 'reply', kind: 'agentMessage', text: 'Main reply done. The independent review is running in the background.' },
    ],
  };
  const agent = { id: 'launch', name: 'Independent review', status: 'running', startedAt,
    completedAt: null, parentToolCallId: 'launch', isBackground: true };
  let background = true;
  let completed = false;
  let socket: WebSocketRoute | undefined;
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, current => {
    socket = current;
    current.send(JSON.stringify({ type: 'supervisor.connected' }));
    current.onMessage(() => current.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail, totalTurnCount: 1, activeSubagents: background ? [agent] : [],
    thread: { ...detail.thread, status: completed ? 'idle' : 'running', activeTurnId: completed ? null : turn.id },
    turns: [{ ...turn, status: completed ? 'completed' : 'inProgress', completedAt: completed ? new Date().toISOString() : null }],
  } }));
  await page.goto(`/threads/${id}`);
  const label = page.locator('.thread-background-agent-status');
  await expect(page.getByText(turn.items[2]!.text, { exact: true })).toBeVisible();
  await expect(label).toHaveText('1 background agent running');
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Subagents · 1 running', exact: true }).click();
  const panel = page.getByRole('dialog', { name: 'Native subagents', exact: true });
  await expect(panel).toContainText('Independent review');
  await expect(panel).toContainText('Running in background');
  await page.getByRole('button', { name: 'Close subagents dialog', exact: true }).click();
  await page.reload();
  await expect(label).toHaveText('1 background agent running');
  await page.setViewportSize({ width: 320, height: 740 });
  await expect(label).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(320);
  const scroll = page.getByTestId('thread-scroll-container');
  expect(await scroll.evaluate(e => e.scrollWidth)).toBe(320);
  const bounds = (await label.boundingBox())!;
  expect(bounds.x).toBeGreaterThanOrEqual(0);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(320);
  await page.screenshot({ path: testInfo.outputPath('background-agent-running.png') });
  background = false;
  expect(socket).toBeDefined();
  socket!.send(JSON.stringify({ type: 'thread.subagents.updated', threadId: id,
    timestamp: new Date().toISOString(), payload: { turnId: turn.id, activeSubagents: [] } }));
  await expect(label).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Subagents · 1 running', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toBeVisible();
  completed = true;
  socket!.send(JSON.stringify({ type: 'thread.turn.completed', threadId: id,
    timestamp: new Date().toISOString(), payload: { turnId: turn.id, status: 'completed' } }));
  socket!.send(JSON.stringify({ type: 'thread.updated', threadId: id, timestamp: new Date().toISOString(), payload: {} }));
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toHaveCount(0);
  await expect(page.locator('.thread-graph-turn-footer')).toHaveCount(0);
});

test('completed work agrees with the composer and the price tooltip has a matching triangle', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const items = [
    { id: 'prompt', kind: 'userMessage', text: 'Check the final status.' },
    { id: 'command', kind: 'commandExecution', text: 'read report', status: 'completed' },
    { id: 'reply', kind: 'agentMessage', text: 'The work is complete.' },
  ];
  const turn = {
    id: 'status-work-turn', status: 'inProgress', startedAt: '2026-10-05T17:00:00Z',
    model: 'gpt-6.1-sol', reasoningEffort: 'high', hasDeferredItems: true, deferredItemCount: 1,
    tokenUsage: { total: { totalTokens: 583158, inputTokens: 582158, cachedInputTokens: 579000, outputTokens: 1000, reasoningOutputTokens: 500 }, generationSpeed: { averageOutputTokensPerSecond: 23.9 } },
    priceEstimate: { currency: 'USD', totalUsd: 0.079, inputUsd: 0.0061, cachedInputUsd: 0.058, outputUsd: 0.0149 },
    items: [items[0], items[2]],
  };
  let completed = false;
  const sockets: WebSocketRoute[] = [];
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
    sockets.push(socket);
    socket.send(JSON.stringify({ type: 'supervisor.connected' }));
    socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
    ...detail, totalTurnCount: 1,
    thread: { ...detail.thread, status: completed ? 'idle' : 'running', activeTurnId: completed ? null : turn.id },
    turns: [{ ...turn, status: completed ? 'completed' : 'inProgress', completedAt: completed ? '2026-10-05T17:01:12Z' : null }],
  } }));
  await page.route(`**/api/threads/${id}/turns/${turn.id}/detail`, route => route.fulfill({ json: {
    ...turn, items, hasDeferredItems: false, deferredItemCount: 0,
  } }));
  await page.goto(`/threads/${id}`);
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toBeVisible();
  const summary = page.locator('.thread-graph-worked-summary');
  await expect(summary.locator('.thread-graph-worked-label')).toHaveText('Working');
  await summary.getByRole('button', { name: /Expand turn 1$/ }).click();
  await expect(page.getByText('read report', { exact: true })).toBeVisible();
  await summary.getByRole('button', { name: /Collapse turn 1$/ }).click();
  completed = true;
  expect(sockets.length).toBeGreaterThan(0);
  for (const socket of sockets) {
    socket.send(JSON.stringify({ type: 'thread.turn.completed', threadId: id, timestamp: '2026-10-05T17:01:12Z', payload: { turnId: turn.id, status: 'completed' } }));
    socket.send(JSON.stringify({ type: 'thread.updated', threadId: id, timestamp: '2026-10-05T17:01:12Z', payload: {} }));
  }
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toHaveCount(0);
  await expect(summary.locator('.thread-graph-worked-label')).toHaveText('Worked for 1m 12s');
  await expect(page.locator('.thread-graph-turn-footer')).toHaveCount(0);
  await expect(page.getByText('The work is complete.', { exact: true })).toBeVisible();
  await summary.locator('.thread-turn-usage-price').hover();
  const tooltip = page.locator('[data-slot="tooltip-content"]');
  await expect(tooltip).toBeVisible();
  await expect(tooltip.locator(':scope > div [aria-label="Uncached input: 3,158 tokens"]')).toBeVisible();
  const arrow = tooltip.locator('[data-slot="tooltip-arrow"]');
  expect(await arrow.evaluate(element => {
    const css = getComputedStyle(element);
    return { width: element.getBoundingClientRect().width, height: element.getBoundingClientRect().height,
      fill: css.fill, surface: getComputedStyle(element.closest('[data-slot="tooltip-content"]')!).backgroundColor,
      background: css.backgroundColor, rotate: css.rotate, transform: css.transform };
  })).toEqual({ width: 10, height: 5, fill: 'rgb(37, 38, 34)', surface: 'rgb(37, 38, 34)', background: 'rgba(0, 0, 0, 0)', rotate: 'none', transform: 'none' });
  const box = (await tooltip.boundingBox())!;
  await page.screenshot({ path: testInfo.outputPath('price-tooltip.png'), clip: { x: box.x - 4, y: box.y - 4, width: box.width + 8, height: box.height + 14 } });
});

test('mobile work summary keeps its step count and cannot scroll the conversation sideways', async ({ page, request }) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const items = [
    { id: 'prompt', kind: 'userMessage', text: 'Review the research results.' },
    { id: 'progress', kind: 'agentMessage', text: 'Checking the final reports.' },
    { id: 'thought', kind: 'reasoning', text: 'Compare the reports independently.' },
    { id: 'command-a', kind: 'commandExecution', text: 'read report A', status: 'completed' },
    { id: 'command-b', kind: 'commandExecution', text: 'read report B', status: 'completed' },
    { id: 'read', kind: 'fileRead', text: '/home/ubuntu/a-long-workspace-path/reports/final-results.md' },
    { id: 'reply', kind: 'agentMessage', text: 'Final report.\n\n' +
      '| Year | Trades | Average holding time | Average position | Annual return | Maximum drawdown | Sharpe | Notes |\n' +
      '| --- | --- | --- | --- | --- | --- | --- | --- |\n' +
      '| 2026 | 1904 | 48 hours | 34 positions | 39.6% | 12.1% | 1.75 | Independently reproduced |\n\n' +
      '`/home/ubuntu/' + 'long-research-workspace-path/'.repeat(10) + 'report.md`\n\n' +
      'Research results remain readable on a narrow phone.\n\n'.repeat(25) },
  ];
  const turn = {
    id: 'mobile-work-turn', status: 'completed', startedAt: '2026-10-05T16:58:02Z', completedAt: '2026-10-05T16:58:50Z',
    model: 'claude-opus-5-5', reasoningEffort: 'max', hasDeferredItems: true, deferredItemCount: 5,
    tokenUsage: { total: { totalTokens: 2500000, inputTokens: 2495000, cachedInputTokens: 2400000, outputTokens: 5000, reasoningOutputTokens: 1000 }, generationSpeed: { averageOutputTokensPerSecond: 131.8 } },
    priceEstimate: { currency: 'USD', totalUsd: 0.61 },
    items: [items[0], items.at(-1)],
  };
  let priceAvailable = true;
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, turns: [{ ...turn, priceEstimate: priceAvailable ? turn.priceEstimate : null }], totalTurnCount: 1 } }));
  await page.route(`**/api/threads/${id}/turns/mobile-work-turn/detail`, route => route.fulfill({ json: { ...turn, items, hasDeferredItems: false, deferredItemCount: 0 } }));
  await page.goto(`/threads/${id}`);
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  await page.getByRole('button', { name: 'Jump to previous turn' }).click();
  const scroll = page.getByTestId('thread-scroll-container');
  const summary = page.locator('.thread-graph-worked-summary');
  const count = summary.locator('.thread-execution-step-count');
  const checkWidth = async () => {
    expect(await scroll.evaluate(e => ({ width: e.clientWidth, content: e.scrollWidth }))).toEqual({ width: page.viewportSize()!.width, content: page.viewportSize()!.width });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(page.viewportSize()!.width);
    for (const selector of ['.thread-execution-step-count', '.thread-turn-usage-effort', '.thread-turn-usage-tokens', '.thread-turn-usage-price, .thread-turn-usage-unavailable', '.thread-turn-token-speed']) {
      const box = await summary.locator(selector).boundingBox();
      expect(box).not.toBeNull();
      expect(box!.x).toBeGreaterThanOrEqual(0);
      expect(box!.x + box!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    }
  };
  await expect(count).toHaveText('5 steps');
  await checkWidth();
  await summary.getByRole('button', { name: /Expand turn 1$/ }).click();
  await expect(summary.getByRole('button', { name: /Collapse turn 1$/ })).toBeVisible();
  await expect(count).toHaveText('5 steps');
  await checkWidth();
  await summary.getByRole('button', { name: /Collapse turn 1$/ }).click();
  await expect(count).toHaveText('5 steps');
  await page.setViewportSize({ width: 320, height: 740 });
  await checkWidth();
  await scroll.evaluate(e => e.scrollTo({ top: 100, left: 100, behavior: 'instant' }));
  expect(await scroll.evaluate(e => e.scrollLeft)).toBe(0);
  expect(await scroll.evaluate(e => e.scrollTop)).toBeGreaterThan(0);
  await page.reload();
  await expect(count).toHaveText('5 steps');
  await checkWidth();
  priceAvailable = false;
  await page.reload();
  await expect(summary.locator('.thread-turn-usage-unavailable')).toBeVisible();
  await checkWidth();
});

test('reading layout stays still with bounded images, visible effort and ten recent notifications', async ({ page, request }, testInfo) => {
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const turns = [{
    id: 'reading-turn', status: 'completed', startedAt: '2026-09-20T10:00:00Z', completedAt: '2026-09-20T10:01:12Z',
    model: 'a-deliberately-long-model-name-for-mobile-layout', reasoningEffort: 'xhigh',
    tokenUsage: { total: { totalTokens: 3500, inputTokens: 1000, outputTokens: 2500, cachedInputTokens: 0, reasoningOutputTokens: 1000 } },
    items: [
      { id: 'reading-prompt', kind: 'userMessage', text: 'Review this image' },
      { id: 'reading-reply', kind: 'agentMessage', text: '![Large test image](https://image.test/reading-test-image.png)\n\n' + 'A paragraph that keeps the conversation scrollable.\n\n'.repeat(30) },
    ],
  }];
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, turns, totalTurnCount: 1 } }));
  await page.route('**/reading-test-image.png', route => route.fulfill({ contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg" width="2400" height="3000"><rect width="2400" height="3000" fill="#3458dc"/></svg>' }));
  await page.route(/\/api\/threads(?:\?.*)?$/, route => route.fulfill({ json: [detail.thread, ...Array.from({ length: 15 }, (_, i) => ({ ...detail.thread, id: `notice-${i}`, title: `Notice ${i}`, lastTurnCompletedAt: new Date(Date.UTC(2026, 8, 19, i)).toISOString() }))] }));
  await page.goto(`/threads/${id}`);
  const scroll = page.getByTestId('thread-scroll-container');
  await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
  const effort = page.locator('.thread-turn-usage-effort');
  await expect(effort).toBeVisible();
  await expect(effort).toHaveText(' · xhigh');
  const img = page.locator('.thread-graph-zoomable-image-trigger > img');
  await expect(img).toBeVisible();
  await expect.poll(() => img.evaluate(e => (e as HTMLImageElement).naturalHeight)).toBe(3000);
  await page.getByRole('button', { name: 'Jump to previous turn' }).click();
  await expect.poll(() => page.locator('[data-timeline-turn]').evaluate(e => Math.abs(e.getBoundingClientRect().top - document.querySelector('[data-testid="thread-scroll-container"]')!.getBoundingClientRect().top - 8))).toBeLessThan(3);
  const size = await img.boundingBox();
  expect(size!.width).toBeLessThanOrEqual(448);
  expect(size!.height).toBeLessThanOrEqual(Math.min(384, page.viewportSize()!.height / 2));
  await scroll.dispatchEvent('wheel', { deltaY: -500 });
  await scroll.evaluate(e => { e.scrollTo({ top: 350, behavior: 'instant' }); });
  const position = () => scroll.evaluate(e => ({ top: e.getBoundingClientRect().top, height: e.clientHeight, scroll: e.scrollTop }));
  await expect.poll(() => scroll.evaluate(e => e.scrollTop)).toBe(350);
  const before = await position();
  await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
  await expect(page.locator('.matter-breadcrumb')).toBeVisible();
  expect(await position()).toEqual(before);
  await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
  expect(await position()).toEqual(before);
  if (testInfo.project.name === 'mobile-chromium') {
    expect(await effort.evaluate(e => {
      const box = e.getBoundingClientRect();
      const parent = e.closest('.thread-turn-usage')!.getBoundingClientRect();
      return box.left >= parent.left && box.right <= parent.right;
    })).toBe(true);
    expect(await page.locator('.matter-thread-tabs').evaluate(e => ({ y: getComputedStyle(e).overflowY, bar: getComputedStyle(e).scrollbarWidth }))).toEqual({ y: 'hidden', bar: 'none' });
  }
  await page.getByRole('button', { name: 'Notifications', exact: true }).click();
  const notifications = page.locator('.matter-notifications > a');
  await expect(notifications).toHaveCount(10);
  await expect(notifications.first()).toContainText('Notice 14');
  await expect(notifications.last()).toContainText('Notice 5');
  await page.getByRole('button', { name: 'Close notification panel' }).click();
  await img.click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath('reading-layout.png') });
});

test('background wake replaces its waiting anchor and continues within the same chat turn', async ({ page, request }, testInfo) => {
  await page.route('**/api/agent-runtimes/*/subscription-usage*',route=>route.fulfill({json:{usage:{provider:'codex',authKind:'subscription',observedAt:new Date().toISOString(),stale:false,windows:[{id:'five_hour',label:'5h',durationMinutes:300,usedPercent:30,resetsAt:null},{id:'weekly',label:'7d',durationMinutes:10080,usedPercent:50,resetsAt:null}]}}}));
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale','zh-CN');
    localStorage.setItem('remote-codex-theme-mode','dark');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin,'local:owner'])}`,JSON.stringify({welcomeDismissed:true,completed:[]}));
  });
  const id=await createThread(request);
  const detail=await (await request.get(`${base}/api/threads/${id}`)).json();
  const startedAt=new Date(Date.now()-60000).toISOString();
  const turnId='background-wake';
  const prompt={id:'prompt',kind:'userMessage',text:'等待发布完成后，检查部署并汇报。',sequence:1};
  const foreground={id:'foreground',kind:'agentMessage',text:'发布任务正在后台运行，完成后我会继续检查。',status:'completed',sequence:2};
  const wait={id:'waiting-anchor',kind:'generic',origin:'nativeBackgroundWait',status:'waiting',text:'',createdAt:startedAt,waitingStartedAt:startedAt,sequence:3};
  let items: object[]=[prompt,foreground,wait];
  let completed=false;
  let socket: WebSocketRoute | undefined;
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, current => {
    socket=current;
    current.send(JSON.stringify({type:'supervisor.connected'}));
    current.onMessage(() => current.send(JSON.stringify({type:'supervisor.pong'})));
  });
  const response=()=>({...detail,totalTurnCount:1,activeSubagents:[],
    thread:{...detail.thread,status:completed?'idle':'running',activeTurnId:completed?null:turnId},
    turns:[{id:turnId,status:completed?'completed':'inProgress',startedAt,completedAt:completed?new Date().toISOString():null,model:'claude-opus-5-5',hasDeferredItems:completed,deferredItemCount:completed?1:0,items:completed?items.filter(item=>(item as {kind:string}).kind!=='commandExecution'):items}]});
  let detailLoads=0;
  await page.route(`**/api/threads/${id}/turns/${turnId}/detail`,route=>{
    detailLoads++;
    return route.fulfill({json:{...response().turns[0],items,hasDeferredItems:false,deferredItemCount:0}});
  });
  await page.route(`**/api/threads/${id}?**`,route=>route.fulfill({json:response()}));
  await page.goto(`/threads/${id}`);
  const usage = page.locator('.thread-subscription-usage');
  await expect(usage).toBeVisible();
  const input = page.locator('[data-testid="chat-composer"] [data-slot="input-group"]');
  const usageBox = (await usage.boundingBox())!;
  const inputBox = (await input.boundingBox())!;
  expect(usageBox.y).toBeGreaterThanOrEqual(inputBox.y + inputBox.height);
  const navigationBox = (await page.locator('.thread-jump-latest-badge').boundingBox())!;
  expect(inputBox.y - navigationBox.y - navigationBox.height).toBeLessThan(18);
  expect(usageBox.height).toBeLessThanOrEqual(16);
  await usage.tap();
  await expect(usage).toHaveAttribute('aria-expanded','true');
  await page.keyboard.press('Escape');
  await expect(usage).toHaveAttribute('aria-expanded','false');
  const waitRow=page.locator('.thread-graph-task-notice');
  await expect(waitRow).toHaveCount(0);
  await expect(page.locator('.thread-graph-turn-footer')).toContainText('等待唤醒');
  await expect(page.locator('.thread-graph-worked-summary')).toHaveCount(1);
  // The waiting marker is durable and remains explicit after a full page reload.
  await page.reload();
  await expect(waitRow).toHaveCount(0);
  await expect(page.locator('.thread-graph-turn-footer')).toContainText('等待唤醒');
  await page.locator('.thread-graph-worked-summary button[aria-expanded]').click();
  await expect(waitRow).toContainText('等待唤醒');
  await page.screenshot({path:testInfo.outputPath('background-waiting.png'),scale:'css'});
  const wake={...wait,origin:'nativeTaskNotification',taskStatus:'completed',status:'completed',text:'GitHub 发布任务已完成',detailText:'所有平台产物和部署检查已通过。',awakenedAt:new Date().toISOString()};
  const checking={id:'checking',kind:'agentMessage',text:'已收到完成通知，正在核对线上版本。',status:'completed',sequence:4};
  const command={id:'verification',kind:'commandExecution',text:'检查已部署版本',status:'completed',sequence:5};
  const final={id:'final',kind:'agentMessage',responsePhase:'final',text:'部署检查完成，线上版本与发布版本一致。',status:'completed',sequence:6};
  const emit=(type:string,payload:object)=>socket!.send(JSON.stringify({type,threadId:id,timestamp:new Date().toISOString(),payload}));
  items=[prompt,foreground,wake,checking,command,final];
  for (const item of [wake,checking,command,final]) emit('thread.item.completed',{turnId,item});
  await expect(waitRow).toHaveCount(1);
  await expect(waitRow).toContainText('已唤醒');
  await expect(page.getByText(final.text,{exact:true})).toBeVisible();
  await expect(page.locator('.thread-graph-worked-summary')).toHaveCount(1);
  await expect(page.locator('.thread-graph-turn-footer')).not.toContainText('等待唤醒');
  await waitRow.locator('.thread-graph-task-notice-toggle').click();
  await expect(waitRow).toContainText(wake.detailText);
  const row=waitRow.locator('.thread-graph-task-notice-row');
  expect(await row.evaluate(el=>el.scrollWidth<=el.clientWidth+1)).toBe(true);
  const markers=await page.locator('.thread-graph-task-notice, [data-role="assistant"]').evaluateAll(els=>els.map(el=>el.textContent));
  expect(markers.findIndex(text=>text?.includes('已唤醒'))).toBeLessThan(markers.findIndex(text=>text?.includes(final.text)));
  await page.screenshot({path:testInfo.outputPath('background-awakened.png'),scale:'css'});
  completed=true;
  emit('thread.turn.completed',{turnId,status:'completed'});
  await page.reload();
  await expect(waitRow).toHaveCount(0);
  await expect(page.getByText(checking.text,{exact:true})).toHaveCount(0);
  await expect(page.getByText(final.text,{exact:true})).toBeVisible();
  expect(detailLoads).toBe(0);
  await page.locator('.thread-graph-worked-summary button[aria-expanded]').click();
  await expect.poll(()=>detailLoads).toBe(1);
  await expect(waitRow).toContainText('已唤醒');
  await expect(page.getByText(final.text,{exact:true})).toBeVisible();
  await expect(page.locator('.thread-graph-turn-footer')).toHaveCount(0);
  const workedToggle = page.locator('.thread-graph-worked-summary button[aria-expanded]');
  await expect(workedToggle).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByText(foreground.text, { exact: true })).toBeVisible();
  await expect(page.getByText(checking.text, { exact: true })).toBeVisible();
  for (let attempt = 0; attempt < 2; attempt++) {
    await workedToggle.click();
    await expect(workedToggle).toHaveAttribute('aria-expanded', 'false');
    await expect(page.getByText(foreground.text, { exact: true })).toHaveCount(0);
    await expect(page.getByText(checking.text, { exact: true })).toHaveCount(0);
    await expect(page.getByText(final.text, { exact: true })).toBeVisible();
    await expect(waitRow).toHaveCount(0);
    if (attempt === 0) await page.screenshot({ path: testInfo.outputPath('background-collapsed.png'), scale: 'css' });
    await workedToggle.click();
    await expect(workedToggle).toHaveAttribute('aria-expanded', 'true');
    await expect(page.getByText(foreground.text, { exact: true })).toBeVisible();
    await expect(page.getByText(checking.text, { exact: true })).toBeVisible();
  }
  expect(detailLoads).toBe(1);
  // A turn can be closed without a final answer (including older lost-wake
  // records). Neither an explicit progress reply nor an untyped legacy reply
  // should escape the folded history just because it is the last text.
  for (const responsePhase of ['commentary', undefined]) {
    const progress = { ...final, text: 'Still waiting on the other platforms', responsePhase };
    items = [prompt, foreground, wake, checking, command, progress];
    await page.reload();
    await expect(workedToggle).toHaveAttribute('aria-expanded', 'false');
    await expect(waitRow).toHaveCount(0);
    await expect(page.getByText(progress.text, { exact: true })).toHaveCount(0);
    await workedToggle.click();
    await expect(waitRow).toContainText('已唤醒');
    await expect(page.getByText(progress.text, { exact: true })).toBeVisible();
  }
  // In a phone split, usage must not lift the conversation switches/navigation
  // away from the prompt (the original overlap was specific to this strip).
  const other = await (await request.post(`${base}/api/threads/start`, {data:{workspaceId:detail.workspace.id,title:'参考会话',provider:'acp',agentId:'codex',model:'ios-e2e-stream',approvalMode:'yolo'}})).json();
  await page.reload();
  await page.getByTestId('workbench-split-trigger').click();
  await page.getByTestId('workbench-thread-picker').locator(`[data-thread-id="${other.id ?? other.thread.id}"]`).click();
  const cluster = page.locator('.thread-jump-latest-cluster.has-pane-switch:visible');
  await expect(cluster).toBeVisible();
  await cluster.locator('[data-side="primary"]').click();
  const activeComposer = page.locator('[data-testid="chat-composer"]:visible');
  const splitInput = (await activeComposer.locator('[data-slot="input-group"]').boundingBox())!;
  await expect(activeComposer.locator('.thread-subscription-usage')).toBeVisible();
  const splitUsage = (await activeComposer.locator('.thread-subscription-usage').boundingBox())!;
  const clusterBox = (await cluster.boundingBox())!;
  expect(splitInput.y - clusterBox.y - clusterBox.height).toBeLessThan(18);
  expect(splitUsage.y).toBeGreaterThanOrEqual(splitInput.y + splitInput.height);
  await page.screenshot({path:testInfo.outputPath('mobile-split-compact-usage.png'),scale:'css'});
});

test('split conversations keep identical composer geometry and surfaces while switching', async ({ page, request, isMobile }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem('remote-codex-theme-mode', 'dark');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  // Only the primary has subscription usage: its presence must not move the input.
  await page.route('**/api/agent-runtimes/*/subscription-usage*', route => route.fulfill({ json: { usage: { provider: 'codex', authKind: 'subscription', observedAt: new Date().toISOString(), stale: false, windows: [{ id: 'five_hour', label: '5h', durationMinutes: 300, usedPercent: 30, resetsAt: null }] } } }));
  const primaryId = await createThread(request);
  const primary = await (await request.get(`${base}/api/threads/${primaryId}`)).json();
  const value = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: primary.workspace.id, title: '另一个模型的会话', provider: 'acp', agentId: 'codex', model: 'default', approvalMode: 'yolo' } })).json();
  const referenceId = value.id ?? value.thread.id;
  await page.goto(`/threads/${primaryId}`);
  await page.getByTestId('workbench-split-trigger').click();
  await page.getByTestId('workbench-thread-picker').locator(`[data-thread-id="${referenceId}"]`).click();
  const pane = (side: string) => page.getByTestId(`${side}-pane`);
  const switchTo = async (side: string) => {
    if (isMobile) await page.locator(`.thread-pane-switch[data-side="${side}"]:visible`).click();
    else await pane(side).locator('[data-testid="chat-composer"] [data-slot="input-group"]').click();
    await expect(pane(side)).toBeVisible();
  };
  const geometry = (side: string) => pane(side).locator('[data-testid="chat-composer"]').evaluate(form => {
    const input = form.querySelector('[data-slot="input-group"]')!;
    const shell = input.parentElement!;
    const host = form.closest('.thread-graph-composer-host')!;
    const cluster = host.querySelector('.thread-jump-latest-cluster')!;
    const rect = input.getBoundingClientRect();
    const strip = cluster.getBoundingClientRect();
    return { y: rect.y, height: rect.height, width: rect.width, formBackground: getComputedStyle(form).backgroundColor, hostBackground: getComputedStyle(host).backgroundColor, shellBackground: getComputedStyle(shell).backgroundColor, layer: getComputedStyle(form.parentElement!).zIndex, gap: rect.top - strip.bottom };
  });
  await switchTo('primary');
  await expect(pane('primary').locator('.thread-subscription-usage')).toBeVisible();
  const left = await geometry('primary');
  const nodes = await page.locator('[data-testid="chat-composer"]').elementHandles();
  for (let attempt = 0; attempt < 3; attempt++) {
    for (const side of ['reference', 'primary']) {
      await switchTo(side);
      const current = await geometry(side);
      expect(Math.abs(current.y - left.y)).toBeLessThanOrEqual(1);
      expect(current.height).toBe(left.height);
      if (isMobile) expect(current.width).toBe(left.width);
      expect(current.formBackground).toBe('rgba(0, 0, 0, 0)');
      expect(current.hostBackground).toBe(left.hostBackground);
      expect(current.shellBackground).toBe(left.shellBackground);
      expect(current.layer).toBe(left.layer);
      if (isMobile) expect(Math.abs(current.gap - left.gap)).toBeLessThanOrEqual(1);
    }
  }
  for (const node of nodes) expect(await node.evaluate(el => el.isConnected)).toBe(true);
  for (const side of ['primary', 'reference']) {
    await switchTo(side);
    await pane(side).getByRole('textbox', { name: '提示词', exact: true }).fill(`${side} 独立草稿`);
  }
  for (const side of ['primary', 'reference']) {
    await switchTo(side);
    await expect(pane(side).getByRole('textbox', { name: '提示词', exact: true })).toHaveText(`${side} 独立草稿`);
    await page.screenshot({ path: testInfo.outputPath(`split-composer-${side}.png`), scale: 'css' });
  }
  if (isMobile) {
    // Browser-only viewport simulation checks the shared avoidance calculation;
    // this is not a claim of Android IME validation.
    await page.evaluate(() => {
      Object.defineProperty(window.visualViewport!, 'height', { configurable: true, value: window.innerHeight - 250 });
      Object.defineProperty(window.visualViewport!, 'offsetTop', { configurable: true, value: 0 });
      window.visualViewport!.dispatchEvent(new Event('resize'));
    });
    await expect.poll(async () => {
      const g = await geometry('reference');
      return g.y + g.height;
    }).toBeLessThan(page.viewportSize()!.height - 250);
    for (const side of ['primary', 'reference']) {
      await switchTo(side);
      await pane(side).getByRole('textbox', { name: '提示词', exact: true }).focus();
      await expect.poll(async () => {
        const g = await geometry(side);
        return g.y + g.height;
      }).toBeLessThan(page.viewportSize()!.height - 250);
    }
  }
});


test('live narration replaces its outside preview during tools and wake, with compact nested steps', async ({ page, request, isMobile }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem('remote-codex-theme-mode', 'light');
    localStorage.setItem('pockymoe-theme-preset', 'plum-pocket');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  const id = await createThread(request);
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const startedAt = new Date(Date.now() - 60_000).toISOString();
  const turnId = 'live-preview';
  const prompt = { id: 'prompt', kind: 'userMessage', text: '检查代码并运行验证。', sequence: 1 };
  const first = { id: 'first', kind: 'agentMessage', text: '我先检查实现，再运行相关验证。', responsePhase: 'commentary', status: 'completed', sequence: 2 };
  const operations = [
    { id: 'skill', kind: 'skillToolCall', text: 'focused-e2e', status: 'completed' },
    ...['rg -n progress src', 'pnpm typecheck', 'pnpm test'].map((text, i) => ({ id: `cmd-${i}`, kind: 'commandExecution', text, status: 'completed' })),
    ...['ThreadTimeline.tsx', 'TimelineTurnRows.tsx', 'history-markdown.css', 'transcript.css'].map((text, i) => ({ id: `read-${i}`, kind: 'fileRead', text: `src/${text}`, status: 'completed' })),
  ].map((item, i) => ({ ...item, sequence: i + 3, createdAt: new Date(Date.parse(startedAt) + (i + 1) * 1000).toISOString() }));
  let items: object[] = [prompt, first, ...operations];
  let completed = false;
  let socket: WebSocketRoute | undefined;
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, current => {
    socket = current;
    current.send(JSON.stringify({ type: 'supervisor.connected' }));
    current.onMessage(() => current.send(JSON.stringify({ type: 'supervisor.pong' })));
  });
  const response = () => ({ ...detail, totalTurnCount: 1, activeSubagents: [],
    thread: { ...detail.thread, status: completed ? 'idle' : 'running', activeTurnId: completed ? null : turnId },
    turns: [{ id: turnId, status: completed ? 'completed' : 'inProgress', startedAt, items }],
  });
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: response() }));
  await page.route(`**/api/threads/${id}/turns/${turnId}/detail`, route => route.fulfill({ json: { ...response().turns[0], hasDeferredItems: false } }));
  const emit = (type: string, payload: object) => socket!.send(JSON.stringify({ type, threadId: id, timestamp: new Date().toISOString(), payload }));
  await page.goto(`/threads/${id}`);
  const turn = page.getByTestId('chat-turn');
  const toggle = turn.locator('.thread-graph-worked-summary button[aria-expanded]');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByText(first.text, { exact: true })).toBeVisible();
  const next = { id: 'next', kind: 'agentMessage', text: '', responsePhase: 'commentary', status: 'inProgress', sequence: 11 };
  emit('thread.item.started', { turnId, item: next });
  await expect(page.getByText(first.text, { exact: true })).toBeVisible();
  emit('thread.output.delta', { turnId, itemId: next.id, sequence: 11, delta: '已定位问题，' });
  await expect(page.getByText('已定位问题，', { exact: true })).toBeVisible();
  await expect(page.getByText(first.text, { exact: true })).toHaveCount(0);
  const progress = { ...next, text: '已定位问题，正在验证修复。', status: 'completed' };
  emit('thread.output.delta', { turnId, itemId: next.id, sequence: 11, delta: '正在验证修复。' });
  emit('thread.item.completed', { turnId, item: progress });
  items = [...items, progress];
  await expect(page.getByText(progress.text, { exact: true })).toHaveCount(1);
  await toggle.click();
  const batch = turn.locator('.thread-graph-history-group-activity').first();
  await batch.locator(':scope > .thread-graph-history-group-card > button').click();
  const children = batch.locator(':scope > .thread-graph-history-group-card > .thread-graph-history-group-list');
  await expect(children).toBeVisible();
  const parentBox = (await batch.boundingBox())!;
  const childrenBox = (await children.boundingBox())!;
  expect(childrenBox.x - parentBox.x).toBeGreaterThanOrEqual(6);
  expect(await children.evaluate(el => parseFloat(getComputedStyle(el).borderLeftWidth))).toBe(1);
  const rows = children.locator(':scope > *');
  await expect(rows).toHaveCount(3);
  const boxes = await rows.evaluateAll(els => els.map(el => { const r = el.getBoundingClientRect(); return { y: r.y, bottom: r.bottom }; }));
  for (let i = 1; i < boxes.length; i++) expect(boxes[i]!.y - boxes[i - 1]!.bottom).toBeLessThanOrEqual(4);
  const subgroupToggle = children.locator('.thread-graph-history-group-toggle').first();
  if (isMobile) expect((await subgroupToggle.boundingBox())!.height).toBeLessThanOrEqual(36);
  await expect(page.getByText(progress.text, { exact: true })).toHaveCount(1);
  expect(await page.getByTestId('thread-scroll-container').evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  await batch.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('compact-nested-steps.png'), scale: 'css' });
  await subgroupToggle.click();
  await expect(children.locator('.matter-command-step').first()).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('nested-command-details.png'), scale: 'css' });
  await toggle.click();
  await expect(page.getByText(first.text, { exact: true })).toHaveCount(0);
  await expect(page.getByText(progress.text, { exact: true })).toBeVisible();
  const later = { id: 'later', kind: 'commandExecution', text: 'run further checks', status: 'running', sequence: 12 };
  emit('thread.item.started', { turnId, item: later });
  await expect(page.getByText(progress.text, { exact: true })).toBeVisible();
  const wait = { id: 'wait', kind: 'other', origin: 'nativeBackgroundWait', status: 'waiting', text: '', sequence: 13 };
  emit('thread.item.completed', { turnId, item: wait });
  await expect(turn.locator('.thread-graph-turn-footer')).toContainText('等待唤醒');
  await expect(page.getByText(progress.text, { exact: true })).toBeVisible();
  emit('thread.item.completed', { turnId, item: { ...wait, origin: 'nativeTaskNotification', taskStatus: 'completed', status: 'completed', text: '验证已完成' } });
  const final = { id: 'final', kind: 'agentMessage', responsePhase: 'final', text: '修复验证通过。', status: 'completed', sequence: 14 };
  emit('thread.item.completed', { turnId, item: final });
  await expect(page.getByText(progress.text, { exact: true })).toHaveCount(0);
  await expect(page.getByText(final.text, { exact: true })).toBeVisible();
  completed = true;
  items = [prompt, first, ...operations, progress, later, { ...wait, origin: 'nativeTaskNotification', taskStatus: 'completed', status: 'completed', text: '验证已完成' }, final];
  emit('thread.turn.completed', { turnId, status: 'completed' });
  await page.reload();
  await expect(page.getByText(final.text, { exact: true })).toBeVisible();
  await expect(page.getByText(progress.text, { exact: true })).toHaveCount(0);
});
