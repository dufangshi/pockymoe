import { test, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { createServer } from 'node:net';

test('cross-device recent chats preserve complete agent families and navigation DOM', async ({ browser }) => {
  const root = await mkdtemp(resolve('.local/recent-switch-'));
  const processes: ChildProcess[] = [];
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  const port = () => new Promise<number>(done => { const server = createServer(); server.listen(0, '127.0.0.1', () => { const value = (server.address() as { port: number }).port; server.close(() => done(value)); }); });
  const relay = `http://127.0.0.1:${await port()}`;
  const password = randomBytes(24).toString('hex');
  function start(command: string, extra: Record<string, string>) {
    const child = spawn(resolve('target/debug/pockymoe'), [command], { env: { ...env, HOST: '127.0.0.1', POCKYMOE_E2E_FAKE_RUNTIME: '1', POCKYMOE_ADMIN_USERNAME: 'switchadmin', POCKYMOE_ADMIN_PASSWORD: password, POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'), ...extra }, stdio: 'ignore' });
    processes.push(child);
  }
  async function api(base: string, path: string, method = 'GET', data?: unknown, token?: string) {
    const response = await fetch(base + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, ...(data ? { body: JSON.stringify(data) } : {}) });
    expect(response.ok, `${method} ${path}: ${response.status}`).toBeTruthy();
    return response.json();
  }
  const context = await browser.newContext();
  try {
    start('relay', { PORT: new URL(relay).port, POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'), POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'), POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'), POCKYMOE_PUBLIC_BASE_URL: relay });
    await expect.poll(() => fetch(relay + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
    await api(relay, '/relay/auth/register', 'POST', { username: 'switchuser', email: 'switch@example.test', password });
    const { token } = await api(relay, '/relay/auth/login', 'POST', { username: 'switchuser', password });
    const records: Array<{ deviceId: string; threadId: string; title: string }> = [];
    let savedChild: typeof records[number] | undefined;
    for (const name of ['Mac', 'WSL']) {
      const created = await api(relay, '/relay/devices', 'POST', { name }, token);
      const number = String(await port());
      const local = `http://127.0.0.1:${number}`;
      const workspacePath = join(root, name);
      await mkdir(workspacePath);
      start('relay-supervisor', { PORT: number, POCKYMOE_RELAY_SUPERVISOR_PORT: number, POCKYMOE_RELAY_SERVER_URL: relay, POCKYMOE_RELAY_AGENT_TOKEN: created.token, POCKYMOE_DATABASE_PATH: join(root, `${name}.sqlite`), POCKYMOE_WORKSPACE_ROOT: workspacePath });
      await expect.poll(() => fetch(local + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
      const deviceApi = `${relay}/relay/devices/${created.device.id}`;
      await expect.poll(async () => (await api(relay, '/healthz')).connectedSupervisors).toBe(records.length + 1);
      const workspace = await api(deviceApi, '/api/workspaces', 'POST', { label: name, absPath: workspacePath }, token);
      const thread = await api(deviceApi, '/api/threads/start', 'POST', { workspaceId: workspace.id, title: `${name} conversation`, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' }, token);
      const record = { deviceId: created.device.id, threadId: thread.id ?? thread.thread.id, title: `${name} conversation` };
      records.push(record);
      await api(deviceApi, `/api/threads/${record.threadId}/prompt`, 'POST', { prompt: `Private ${name} prompt: reply hello` }, token);
      await api(relay, '/relay/account/workbench', 'POST', { ...record, workspaceId: workspace.id, workspaceLabel: name }, token);
      if (name === 'WSL') {
        let firstChildId = '';
        await mkdir(workspacePath + '/agents');
        const otherWorkspace = await api(deviceApi, '/api/workspaces', 'POST', { label: 'Agent workspace', absPath: workspacePath + '/agents' }, token);
        for (let index = 0; index < 8; index++) {
          const child = await api(deviceApi, '/api/threads/start', 'POST', {
            workspaceId: index === 7 ? otherWorkspace.id : workspace.id,
            title: `WSL agent ${index}`, provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo',
            parentThreadId: index === 7 ? firstChildId : record.threadId,
          }, token);
          if (index === 0) {
            firstChildId = child.id ?? child.thread.id;
            savedChild = { deviceId: record.deviceId, threadId: firstChildId, title: 'WSL agent 0' };
            await api(relay, '/relay/account/workbench', 'POST', { ...savedChild, workspaceId: workspace.id, workspaceLabel: name }, token);
          }
        }
      }
    }
    // Reproduce a previously persisted shadow reference. Only this bookmark
    // should disappear; both real sessions must remain untouched.
    await api(relay, '/relay/account/workbench', 'POST', { ...records[0], deviceId: records[1]!.deviceId, workspaceLabel: 'Mac' }, token);
    const page = await context.newPage();
    await page.goto(relay + '/relay-portal');
    await page.evaluate(async ({ password }) => {
      const response = await fetch('/relay/auth/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ username: 'switchuser', password }) });
      if (!response.ok) throw new Error('Login failed');
      const value = await response.json();
      localStorage.setItem('remote-codex-relay-mode', 'true');
      localStorage.setItem('remote-codex-relay-token', value.token);
    }, { password });
    const href = (record: typeof records[number]) => `/devices/${record.deviceId}/threads/${record.threadId}`;
    await page.goto(relay + href(records[0]!));
    await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
    await expect.poll(async () => (await api(relay, '/relay/account/workbench', 'GET', undefined, token)).threads.length).toBe(3);
    // The current workspace's threads sit above the recent ones; a family moves
    // between the two lists as the page switches devices.
    const sidebar = page.locator('aside.matter-sidebar');
    const family = sidebar.locator('.matter-thread-section > div').filter({ has: page.locator(`a[href="${href(records[1]!)}"]`) });
    await expect(family.locator('summary')).toHaveText('8 agent threads');
    await expect(sidebar.locator('.matter-thread-section a')).toHaveCount(10);
    await page.reload();
    await expect(family.locator('summary')).toHaveText('8 agent threads');
    await page.evaluate(() => {
      const state = window as unknown as { navNodes: Element[]; blankNavigation: boolean };
      state.navNodes = ['.matter-topbar', '.matter-sidebar', '.matter-thread-tabs'].map(selector => document.querySelector(selector)!);
      state.blankNavigation = false;
      new MutationObserver(() => {
        if (!document.querySelector('.matter-sidebar .matter-thread-section a') || !document.querySelector('.matter-thread-tabs a')) state.blankNavigation = true;
      }).observe(document.querySelector('.matter-workbench')!, { childList: true, subtree: true });
    });
    for (const record of [records[1]!, records[0]!, records[1]!, records[0]!]) {
      await sidebar.locator(`a[href="${href(record)}"]`).click();
      await expect(page).toHaveURL(relay + href(record));
      await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
      await expect(family.locator('summary')).toHaveText('8 agent threads');
      await expect(page.getByRole('navigation', { name: 'Workspace threads', exact: true }).locator('a[aria-current="page"]')).toHaveAttribute('href', href(record));
      const name = record.title.split(' ')[0];
      await expect(page.getByText(`Private ${name} prompt: reply hello`, { exact: true })).toBeVisible();
      await expect(page.getByText(`Private ${name === 'Mac' ? 'WSL' : 'Mac'} prompt: reply hello`, { exact: true })).toHaveCount(0);
      expect(await page.evaluate(() => {
        const state = window as unknown as { navNodes: Element[]; blankNavigation: boolean };
        return !state.blankNavigation && state.navNodes.every(node => node.isConnected);
      })).toBe(true);
      await expect.poll(async () => (await api(relay, '/relay/account/workbench', 'GET', undefined, token)).threads.map((r: typeof record) => `${r.deviceId}:${r.threadId}`).sort()).toEqual([...records, savedChild!].map(r => `${r.deviceId}:${r.threadId}`).sort());
    }
    await family.locator('summary').click();
    await expect(family.getByRole('link', { name: /WSL agent 7/ })).toBeVisible();
    await family.locator(`a[href="${href(savedChild!)}"]`).click();
    await expect(page).toHaveURL(relay + href(savedChild!));
    await expect(family.locator('summary')).toHaveText('8 agent threads');
  } finally {
    await context.close();
    for (const child of processes.reverse()) { child.kill('SIGTERM'); await new Promise<void>(done => { if (child.exitCode !== null) done(); else child.once('exit', () => done()); }); }
    await rm(root, { recursive: true, force: true });
  }
});
