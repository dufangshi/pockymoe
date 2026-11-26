import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type APIRequestContext } from '@playwright/test';

// Issue #171: a queue read that started before Steer must not put the steered
// row back when it returns late. The socket lags behind HTTP (as over the
// relay), so the page reads the queue after queueing; that read is held until
// after the one Steer click has finished.

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const SOCKET_LAG_MS = 300;
const DELIVERY_LAG_MS = 3_000;

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'en');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
});

async function idleThread(request: APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright', `steer-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const ws = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'steer' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, {
    data: { workspaceId: ws.id, title: 'steer once', provider: 'codex', model: 'default', approvalMode: 'yolo' },
  })).json();
  return (started.thread ?? started) as { id: string };
}

test('one Steer click stays steered when an older queue read returns late', async ({ page, request }) => {
  const thread = await idleThread(request);
  let lagSocket = false;
  await page.routeWebSocket(/\/ws(?:\?.*)?$/, (ws) => {
    const server = ws.connectToServer();
    // Every frame keeps its order but arrives late once the lag is on.
    server.onMessage((message) => {
      if (lagSocket) setTimeout(() => ws.send(message), SOCKET_LAG_MS);
      else ws.send(message);
    });
    ws.onMessage((message) => server.send(message));
  });
  const deliveryReads: Promise<void>[] = [];
  await page.route(/[?&]view=delivery/, (route) => {
    deliveryReads.push((async () => {
      const response = await route.fetch();
      await new Promise((resolve) => setTimeout(resolve, DELIVERY_LAG_MS));
      await route.fulfill({ response });
    })());
  });
  const steerPosts: string[] = [];
  page.on('request', (r) => {
    if (r.method() === 'POST' && /\/pending-steers\/[^/]+\/steer$/.test(new URL(r.url()).pathname)) steerPosts.push(r.url());
  });

  await page.goto(`/threads/${thread.id}`);
  const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true });
  await prompt.fill('inspect this repository first');
  await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Stop Current Turn', exact: true })).toBeVisible();

  lagSocket = true;
  await prompt.fill('steer me please');
  await page.getByRole('button', { name: 'Send after this turn', exact: true }).click();
  const queue = page.getByRole('region', { name: 'Queued prompts' });
  await expect(queue).toContainText('steer me please');
  // The late socket receipt made the page read the queue over HTTP.
  await expect.poll(() => deliveryReads.length).toBe(1);

  await queue.getByRole('button', { name: 'Steer', exact: true }).click();
  await expect(queue).toHaveCount(0);
  await expect.poll(async () =>
    (await (await request.get(`${base}/api/threads/${thread.id}`)).json()).pendingSteers
      .filter((row: { delivery: string }) => row.delivery !== 'steer').length,
  ).toBe(0);

  // Let the held read (taken while the row was still queued) reach the page.
  // Without the guard the row comes back here and stays until the turn ends,
  // so a short window after it lands is enough to see it.
  await Promise.all(deliveryReads);
  await page.waitForTimeout(1_000);
  expect(await queue.count()).toBe(0);
  expect(steerPosts).toHaveLength(1);
});
