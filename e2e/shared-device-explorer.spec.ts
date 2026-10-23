import { test, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomBytes } from 'node:crypto';

test.use({ actionTimeout: 15_000 });

test('shared device Explorer honors full, read-only and no filesystem access', async ({ browser }) => {
  await mkdir(resolve('.local'), { recursive: true });
  const root = await mkdtemp(resolve('.local/shared-device-explorer-'));
  const processes: ChildProcess[] = [];
  const logs: string[] = [];
  const freePort = () => new Promise<number>(done => {
    const server = createServer();
    server.listen(0, '127.0.0.1', () => {
      const port = (server.address() as { port: number }).port;
      server.close(() => done(port));
    });
  });
  const rp = await freePort(), sp = await freePort();
  const base = `http://127.0.0.1:${rp}`;
  const password = randomBytes(24).toString('hex');
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  function start(command: string, extra: Record<string, string>) {
    const proc = spawn(resolve('target/debug/pockymoe'), [command], {
      env: { ...env, HOST: '127.0.0.1', POCKYMOE_ADMIN_USERNAME: 'testadmin',
        POCKYMOE_ADMIN_PASSWORD: password, POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_E2E_FAKE_RUNTIME: '1', ...extra },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    for (const stream of [proc.stdout, proc.stderr]) stream!.on('data', chunk => logs.push(String(chunk)));
    processes.push(proc);
  }
  async function api(path: string, token?: string, data?: unknown, method = data ? 'POST' : 'GET') {
    const response = await fetch(base + path, {
      method, headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(data ? { 'content-type': 'application/json' } : {}) },
      ...(data ? { body: JSON.stringify(data) } : {}), signal: AbortSignal.timeout(10_000),
    });
    const text = await response.text();
    expect(response.ok, `${method} ${path}: ${response.status} ${text.slice(0, 200)}`).toBeTruthy();
    return text ? JSON.parse(text) : null;
  }
  async function account(username: string) {
    await api('/relay/auth/register', undefined, { username, email: `${username}@example.test`, password });
    return (await api('/relay/auth/login', undefined, { username, password })).token as string;
  }
  const context = await browser.newContext();
  let releaseIdentity!: () => void;
  const identityReady = new Promise<void>(resolve => { releaseIdentity = resolve; });
  try {
    start('relay', { PORT: String(rp), POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_PUBLIC_BASE_URL: base,
      POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist') });
    await expect.poll(() => fetch(base + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
    const owner = await account('owner'), guest = await account('guest');
    // This spec shares through the relay API and tests access, not the
    // safety-code check (workspace-sharing.spec.ts covers it): turn it off.
    await api('/relay/account/preferences', owner, { requireSafetyCode: false }, 'PATCH');
    const created = await api('/relay/devices', owner, { name: 'Shared Explorer device' });
    const deviceId = created.device.id;
    start('relay-supervisor', { POCKYMOE_RELAY_SUPERVISOR_PORT: String(sp),
      POCKYMOE_RELAY_SERVER_URL: base, POCKYMOE_RELAY_AGENT_TOKEN: created.token,
      POCKYMOE_DATABASE_PATH: join(root, 'device.sqlite'), POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces') });
    await expect.poll(() => api('/healthz').then(r => r.connectedSupervisors)).toBe(1);
    const deviceApi = `/relay/devices/${deviceId}/api`;
    const project = join(root, 'project');
    await mkdir(project);
    await writeFile(join(project, 'shared-note.txt'), 'Shared device file contents');
    const workspace = await api(deviceApi + '/workspaces', owner, { absPath: project, label: 'Shared files' });
    const thread = await api(deviceApi + '/threads/start', owner, { workspaceId: workspace.id, title: 'Shared Explorer', model: 'fake' });
    const threadId = thread.thread?.id ?? thread.id;
    const grant = await api('/relay/grants', owner, { deviceId, targetIdentifier: 'guest', scope: 'device',
      workspaceScope: 'all', threadAccess: 'control', workspaceAccess: 'write', canCreateThreads: true });
    const access = await api(`/relay/access?deviceId=${deviceId}&threadId=${threadId}`, guest);
    expect(access).toMatchObject({ scope: 'device', workspaceId: null, workspaceAccess: 'write' });
    await context.addCookies([{ name: 'remote_codex_relay_session', value: guest, url: base }]);
    const page = await context.newPage();
    let identityRequests = 0;
    // The app authenticates first; delay the workbench's second identity lookup
    // so opening Explorer happens before its persistent profile can be restored.
    await page.route('**/relay/auth/session', async route => {
      if (++identityRequests > 1) await identityReady;
      await route.continue();
    });
    const openThread = async () => {
      await page.goto(`${base}/devices/${deviceId}/threads/${threadId}`);
      return page.getByRole('navigation', { name: 'Workspace tools' }).getByRole('button', { name: 'Toggle Explorer' });
    };
    const toggle = await openThread();
    await expect(toggle).toBeEnabled();
    await expect.poll(() => identityRequests).toBeGreaterThan(1);
    await toggle.click();
    releaseIdentity();
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');
    const explorer = page.getByRole('complementary', { name: 'Explorer', exact: true });
    await expect(explorer).toBeVisible();
    const row = page.getByRole('treeitem', { name: 'shared-note.txt', exact: true });
    await expect(row).toBeVisible();
    await row.click();
    await expect(page.getByText('Shared device file contents', { exact: true })).toBeVisible();
    await row.hover();
    await row.getByRole('button', { name: 'More actions for shared-note.txt' }).click();
    await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
    const rename = page.getByRole('dialog', { name: 'Rename file', exact: true });
    await rename.getByRole('textbox', { name: 'Name' }).fill('renamed-note.txt');
    await rename.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByRole('treeitem', { name: 'renamed-note.txt', exact: true })).toBeVisible();
    expect(await readFile(join(project, 'renamed-note.txt'), 'utf8')).toBe('Shared device file contents');

    await api(`/relay/grants/${grant.id}`, owner, { workspaceAccess: 'read' }, 'PATCH');
    await page.reload();
    const readonlyToggle = page.getByRole('navigation', { name: 'Workspace tools' }).getByRole('button', { name: 'Toggle Explorer' });
    await expect(readonlyToggle).toBeEnabled();
    // The workbench restores the open Explorer profile after authentication.
    // A conditional click during restoration could close the panel it just opened.
    await expect(explorer).toBeVisible();
    await expect(readonlyToggle).toHaveAttribute('aria-expanded', 'true');
    const readonlyRow = page.getByRole('treeitem', { name: 'renamed-note.txt', exact: true });
    await expect(readonlyRow).toBeVisible();
    await readonlyRow.click();
    await expect(page.getByText('Shared device file contents', { exact: true })).toBeVisible();
    await readonlyRow.hover();
    // The mutation menu is entirely absent for filesystem readers.
    await expect(readonlyRow.getByRole('button', { name: 'More actions for renamed-note.txt' })).toHaveCount(0);

    await api(`/relay/grants/${grant.id}`, owner, { workspaceAccess: 'none' }, 'PATCH');
    const noFilesAccess = page.waitForResponse(response => response.url().includes('/relay/access?') && response.ok());
    await page.reload();
    expect(await (await noFilesAccess).json()).toMatchObject({ workspaceAccess: 'none' });
    await expect(page.getByRole('treeitem', { name: 'renamed-note.txt', exact: true })).toHaveCount(0);
    const denied = await fetch(`${base}${deviceApi}/workspaces/${workspace.id}/files/tree`, { headers: { authorization: `Bearer ${guest}` } });
    expect([401, 403]).toContain(denied.status);
  } catch (error) {
    console.error(logs.join(''));
    throw error;
  } finally {
    releaseIdentity();
    await context.close().catch(() => {});
    for (const proc of processes.reverse()) {
      if (proc.exitCode !== null) continue;
      const exited = new Promise<void>(done => proc.once('exit', () => done()));
      proc.kill('SIGTERM');
      await exited;
    }
    await rm(root, { recursive: true, force: true });
  }
});
