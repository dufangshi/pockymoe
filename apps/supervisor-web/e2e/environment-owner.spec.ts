import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { progressStorageKey } from '../src/components/onboarding/tourStorage';

test('real owner browser approves actual CLI proposal and original thread continues in Dokploy', async ({ page, baseURL }) => {
  const run = JSON.parse(readFileSync(process.env.POCKY_ENV_RUN_MANIFEST!, 'utf8'));
  const input = JSON.parse(readFileSync(path.join(run.runRoot, 'owner-ui-input.json'), 'utf8'));
  const credentials = JSON.parse(readFileSync(path.join(run.runRoot, 'private/owner.json'), 'utf8'));
  const login = await page.request.post('/api/auth/login', { data: { username: 'fixture-owner', password: credentials.password } });
  expect(login.status(), 'Synthetic owner login HTTP status').toBe(200);
  await page.addInitScript(() => { localStorage.setItem('remote-codex-locale', 'en'); localStorage.setItem('pockymoe.phoneDock', 'off'); });
  await page.addInitScript(key => localStorage.setItem(key, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })), progressStorageKey('local:owner', new URL(baseURL!).origin));
  await page.goto('/threads/' + input.threadId);
  const shellResponse = await page.request.post(`/api/threads/${input.threadId}/shell`, { data: { cols: 80, rows: 24, label: 'Host guardian acceptance' } });
  expect(shellResponse.status()).toBe(200);
  const shellId = (await shellResponse.json()).activeShellId;
  await page.evaluate(async shellId => {
    const ws = new WebSocket(location.origin.replace('http:', 'ws:') + '/ws');
    const messages: Record<string, unknown>[] = [];
    const state = { ws, messages, shellId, viewerId: '' };
    (window as unknown as { guardianProbe: typeof state }).guardianProbe = state;
    ws.addEventListener('message', event => { const value = JSON.parse(event.data); messages.push(value); if (value.type === 'shell.connected') state.viewerId = value.payload.viewerId; });
    await new Promise<void>(resolve => ws.addEventListener('open', () => resolve(), { once: true }));
    ws.send(JSON.stringify({ type: 'shell.attach', shellId, cols: 80, rows: 24 }));
  }, shellId);
  await expect.poll(() => page.evaluate(() => (window as unknown as { guardianProbe: { viewerId: string } }).guardianProbe.viewerId)).not.toBe('');
  const inputShell = async (data: string) => page.evaluate(data => { const s = (window as unknown as { guardianProbe: { ws: WebSocket; shellId: string; viewerId: string } }).guardianProbe; s.ws.send(JSON.stringify({ type: 'shell.input', shellId: s.shellId, viewerId: s.viewerId, data })); }, data);
  await page.evaluate(() => { const s = (window as unknown as { guardianProbe: { ws: WebSocket; shellId: string; viewerId: string } }).guardianProbe; s.ws.send(JSON.stringify({ type: 'shell.resize', shellId: s.shellId, viewerId: s.viewerId, cols: 101, rows: 33 })); });
  await inputShell('stty size > host-pty-size.txt\n');
  await expect.poll(() => { try { return readFileSync(path.join(input.project, 'host-pty-size.txt'), 'utf8').trim(); } catch { return ''; } }).toBe('33 101');
  await inputShell("python3 -c 'import pathlib,time; p=pathlib.Path(\"host-pty-progress\"); n=0\nwhile True:\n n+=1;p.write_text(str(n));time.sleep(.05)'\n");
  await expect.poll(() => { try { return Number(readFileSync(path.join(input.project, 'host-pty-progress'), 'utf8')); } catch { return 0; } }).toBeGreaterThan(0);
  await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
  await page.getByRole('button', { name: 'Isolated environments', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Isolated environments', exact: true });
  const card = dialog.locator(`[data-request-id="${input.requestId}"]`);
  await expect(card.getByText('Awaiting your approval', { exact: true })).toBeVisible();
  await expect(card.getByTitle(input.project).first()).toBeVisible();
  await expect(card.getByText(input.digest, { exact: true })).toBeVisible();
  const decision = page.waitForResponse(r => new URL(r.url()).pathname === `/api/environment-requests/${input.requestId}/approve` && r.request().method() === 'POST');
  await card.getByRole('button', { name: 'Approve and migrate when idle' }).click();
  const response = await decision; expect(response.status()).toBe(200);
  expect(response.request().postDataJSON()).toEqual({ digest: input.digest });
  await expect(card.getByText('Approved, waiting for the current turn to exit', { exact: true })).toBeVisible();
  // The real old turn remains active until this public fixture marker. No API
  // write is mocked, and no credential contents are passed to the worker.
  const { writeFileSync } = await import('node:fs'); writeFileSync(path.join(input.project, 'allow-host-finish'), '');
  await expect.poll(async () => (await (await page.request.get(`/api/environment-requests/${input.requestId}`)).json()).state, { timeout: 100000 }).toBe('ready');
  await expect.poll(() => { try { return JSON.parse(readFileSync(path.join(input.project, 'continuation-evidence.json'), 'utf8')).sameThread; } catch { return false; } }).toBe(true);
  await expect(card.getByText('Migrated to isolation', { exact: true })).toBeVisible();
  const progress = readFileSync(path.join(input.project, 'host-pty-progress'), 'utf8');
  await inputShell('touch old-pty-must-not-write\n');
  await expect.poll(() => page.evaluate(() => (window as unknown as { guardianProbe: { messages: { type?: string }[] } }).guardianProbe.messages.some(value => value.type === 'shell.error'))).toBe(true);
  expect(readFileSync(path.join(input.project, 'host-pty-progress'), 'utf8')).toBe(progress);
  const { existsSync } = await import('node:fs'); expect(existsSync(path.join(input.project, 'old-pty-must-not-write'))).toBe(false);
  await page.evaluate(() => (window as unknown as { guardianProbe: { ws: WebSocket } }).guardianProbe.ws.close());
  // Returning to Host is another owner decision. The old selector must not
  // silently drop a completed request's durable isolation fence.
  await expect.poll(async () => (await (await page.request.get(`/api/threads/${input.threadId}`)).json()).thread.status).toBe('idle');
  const oldSelector = await page.request.put(`/api/threads/${input.threadId}/environment`, { data: { environmentId: null } });
  expect(oldSelector.ok()).toBe(false);
  await card.getByRole('button', { name: 'Restore Host execution…' }).click();
  const recovery = page.getByRole('dialog', { name: 'Abandon migration and restore Host?' });
  const restoreResponse = page.waitForResponse(r => new URL(r.url()).pathname === `/api/environment-requests/${input.requestId}/recoverHost` && r.request().method() === 'POST');
  await recovery.getByRole('button', { name: 'Destroy computation and restore Host' }).click();
  expect((await restoreResponse).status()).toBe(200);
  await expect(card.getByText('Cancelled', { exact: true })).toBeVisible();
  const recovered = await (await page.request.get(`/api/environment-requests/${input.requestId}`)).json();
  const environments = await (await page.request.get('/api/environments')).json();
  expect(environments.environments.find((value: { id: string }) => value.id === recovered.environmentId).state).toBe('destroyed');
  expect(existsSync(path.join(input.project, 'continuation-evidence.json'))).toBe(true);
});
