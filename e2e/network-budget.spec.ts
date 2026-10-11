import { randomUUID } from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

// Counts what an open thread page fetches while idle, while a turn runs and
// while the tab is hidden. Live state must come from WebSocket events, not
// from re-polling summaries, thread lists, capabilities or /healthz.
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;

type Bucket = { count: number; bytes: number };

function endpointKey(url: URL, threadIds: string[]) {
  let pathname = url.pathname;
  for (const [index, id] of threadIds.entries()) pathname = pathname.replaceAll(id, `<thread${index}>`);
  const view = url.searchParams.get('view');
  return `${pathname}${view ? `?view=${view}` : ''}${url.searchParams.has('includeAgentThreads') ? '?includeAgentThreads' : ''}`;
}

function trackNetwork(page: Page, threadIds: string[]) {
  let buckets = new Map<string, Bucket>();
  let wsBytes = 0;
  let startedAt = Date.now();
  let timeline: string[] = [];
  page.on('requestfinished', async (request) => {
    const url = new URL(request.url());
    if (!url.pathname.startsWith('/api/') && url.pathname !== '/healthz') return;
    const key = endpointKey(url, threadIds);
    timeline.push(`${((Date.now() - startedAt) / 1000).toFixed(1)}s ${key}`);
    const target = buckets;
    const bucket = target.get(key) ?? { count: 0, bytes: 0 };
    bucket.count += 1;
    target.set(key, bucket);
    const sizes = await request.sizes().catch(() => null);
    bucket.bytes += (sizes?.responseBodySize ?? 0) + (sizes?.responseHeadersSize ?? 0);
  });
  page.on('websocket', (socket) => {
    socket.on('framereceived', (frame) => { wsBytes += typeof frame.payload === 'string' ? Buffer.byteLength(frame.payload) : frame.payload.length; });
    socket.on('framesent', (frame) => { wsBytes += typeof frame.payload === 'string' ? Buffer.byteLength(frame.payload) : frame.payload.length; });
  });
  return {
    reset() { buckets = new Map(); wsBytes = 0; startedAt = Date.now(); timeline = []; },
    elapsedMs: () => Date.now() - startedAt,
    async snapshot() {
      await new Promise(resolve => setTimeout(resolve, 300)); // let response sizes settle
      const endpoints = Object.fromEntries([...buckets.entries()].sort().map(([key, b]) => [key, { ...b }]));
      const total = Object.values(endpoints).reduce((sum, b) => ({ count: sum.count + b.count, bytes: sum.bytes + b.bytes }), { count: 0, bytes: 0 });
      return { seconds: Math.round((Date.now() - startedAt) / 100) / 10, endpoints, total, wsBytes, timeline: [...timeline] };
    },
  };
}

type Snapshot = Awaited<ReturnType<ReturnType<typeof trackNetwork>['snapshot']>>;
const count = (snapshot: Snapshot, prefix: string) =>
  Object.entries(snapshot.endpoints).filter(([key]) => key.startsWith(prefix)).reduce((sum, [, b]) => sum + b.count, 0);

async function waitForIdle(request: APIRequestContext, threadId: string) {
  await expect.poll(async () => {
    const detail = await (await request.get(`${base}/api/threads/${threadId}?view=summary&limit=1`)).json();
    return detail.thread.activeTurnId === null && detail.thread.status !== 'running' ? detail.thread.status : 'running';
  }, { timeout: 60_000 }).not.toBe('running');
}

async function setHidden(page: Page, hidden: boolean) {
  await page.evaluate((value) => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (value ? 'hidden' : 'visible') });
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => value });
    document.dispatchEvent(new Event('visibilitychange'));
  }, hidden);
}

test('an open thread stays within its request budget while idle, running and hidden', async ({ page, request }, testInfo) => {
  test.setTimeout(240_000);
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `network-${randomUUID()}`);
  await fs.mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Network budget' } })).json();
  const start = async (title: string) => {
    const response = await request.post(`${base}/api/threads/start`, {
      data: { workspaceId: workspace.id, title, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' },
    });
    expect(response.ok()).toBeTruthy();
    const value = await response.json();
    return (value.thread ?? value) as { id: string };
  };
  const main = await start('Budget main');
  const other = await start('Budget other');
  expect((await request.post(`${base}/api/threads/${main.id}/prompt`, { data: { prompt: 'hello' } })).ok()).toBeTruthy();
  await waitForIdle(request, main.id);

  const tally = trackNetwork(page, [main.id, other.id]);
  await page.goto(`/threads/${main.id}`);
  const later = page.getByRole('button', { name: 'Later', exact: true });
  if (await later.isVisible({ timeout: 3000 }).catch(() => false)) await later.click();
  const tabs = page.getByRole('navigation', { name: 'Workspace threads' });
  const mainStatus = tabs.locator(`a[href="/threads/${main.id}"] [role="img"]`).first();
  const otherStatus = tabs.locator(`a[href="/threads/${other.id}"] [role="img"]`).first();
  await expect(page.locator('[data-turn-id]').first()).toBeVisible();
  await expect(otherStatus).toBeVisible();
  // Past the first load and a connection's capability settling reads.
  await page.waitForTimeout(8000);
  const load = await tally.snapshot();

  tally.reset();
  await page.waitForTimeout(20_000);
  const idle = await tally.snapshot();

  // The slow fake turn runs 25 s; measure 20 s of it.
  tally.reset();
  expect((await request.post(`${base}/api/threads/${main.id}/prompt`, { data: { prompt: 'inspect this repository' } })).ok()).toBeTruthy();
  await expect(mainStatus).toHaveAttribute('data-status', 'running', { timeout: 5000 });
  await page.waitForTimeout(Math.max(0, 4000 - tally.elapsedMs()));
  // Another thread's status reaches the tabs through its events.
  expect((await request.post(`${base}/api/threads/${other.id}/prompt`, { data: { prompt: 'inspect this repository please' } })).ok()).toBeTruthy();
  await expect(otherStatus).toHaveAttribute('data-status', 'running', { timeout: 5000 });
  await page.waitForTimeout(Math.max(0, 19_700 - tally.elapsedMs()));
  const running = await tally.snapshot();
  // The finished turn and status arrive without a page poll.
  await expect(mainStatus).not.toHaveAttribute('data-status', 'running', { timeout: 15_000 });
  await expect(otherStatus).not.toHaveAttribute('data-status', 'running', { timeout: 15_000 });
  // A short streamed reply renders live and persists.
  const phrase = `budget narration ${randomUUID().slice(0, 8)}`;
  expect((await request.post(`${base}/api/threads/${main.id}/prompt`, { data: { prompt: `Reply with exactly ${phrase}.` } })).ok()).toBeTruthy();
  await expect(page.getByText(phrase, { exact: false }).last()).toBeVisible({ timeout: 10_000 });
  await waitForIdle(request, main.id);
  await expect(mainStatus).not.toHaveAttribute('data-status', 'running', { timeout: 10_000 });

  await page.waitForTimeout(2000);
  tally.reset();
  await setHidden(page, true);
  await page.waitForTimeout(12_000);
  const hidden = await tally.snapshot();
  tally.reset();
  await setHidden(page, false);
  await page.waitForTimeout(2000);
  const returned = await tally.snapshot();

  const report = { project: testInfo.project.name, load, idle, running, hidden, returned };
  console.log(`NETWORK_BUDGET ${JSON.stringify(report)}`);
  await testInfo.attach('network-budget.json', { body: JSON.stringify(report, null, 2), contentType: 'application/json' });

  // Idle (20 s): the topbar metric meter (15 s) and at most one fallback refresh.
  expect(count(idle, '/healthz')).toBe(0);
  expect(count(idle, '/api/threads/<thread0>?view=summary')).toBe(0);
  expect(count(idle, '/api/threads?includeAgentThreads')).toBeLessThanOrEqual(1);
  expect(count(idle, '/api/threads/<thread0>/capabilities')).toBeLessThanOrEqual(1);
  expect(count(idle, '/api/device/metrics')).toBeLessThanOrEqual(2);
  expect(idle.total.count).toBeLessThanOrEqual(4);
  // Running (20 s): refreshes follow turn and status events, not a 3 s poll.
  expect(count(running, '/healthz')).toBe(0);
  expect(count(running, '/api/threads/<thread0>?view=summary')).toBeLessThanOrEqual(3);
  expect(count(running, '/api/threads?includeAgentThreads')).toBeLessThanOrEqual(3);
  expect(count(running, '/api/threads/<thread0>/capabilities')).toBeLessThanOrEqual(3);
  expect(running.total.count).toBeLessThanOrEqual(14);
  // Hidden: nothing is polled; returning refreshes immediately.
  expect(hidden.total.count).toBe(0);
  expect(returned.total.count).toBeGreaterThanOrEqual(1);
});
