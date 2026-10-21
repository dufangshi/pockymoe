import { test, expect, type Browser, type TestInfo } from '@playwright/test';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';
import { Fixture, approveGrant, browserRequest, ok, screenshot, shareDialog } from './helpers/account-key-fixture';

// No auth material enters traces, videos or automatic failure screenshots.
test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

const freePort = () => new Promise<number>(done => {
  const server = createServer();
  server.listen(0, '127.0.0.1', () => { const port = (server.address() as { port: number }).port; server.close(() => done(port)); });
});

// The account-key fixture's relay normally takes E2E_API_PORT, which the
// Playwright Supervisor already uses here; this relay gets its own port.
async function isolatedFixture(browser: Browser, info: TestInfo) {
  const port = await freePort();
  class Isolated extends Fixture { override readonly base = `http://localhost:${port}`; }
  const dataRoot = resolve(process.env.E2E_WORKSPACE_ROOT ?? '.scratch');
  await mkdir(dataRoot, { recursive: true });
  const root = await mkdtemp(join(dataRoot, 'workspace-share-e2e-'));
  await mkdir(join(root, 'home'));
  const f = new Isolated(root, browser, info);
  f.start('relay', { PORT: String(port), POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
    POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_PUBLIC_BASE_URL: f.base,
    POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist') });
  await expect.poll(() => fetch(f.base + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
  return f;
}

// The owner shares one workspace of a protected device from a thread's dialog.
// The recipient reaches that workspace's threads and files, starts a thread
// there, and never another workspace or a terminal.
test('a signed workspace share reaches its threads and files but never a terminal', async ({ browser }, info) => {
  test.setTimeout(300_000);
  const f = await isolatedFixture(browser, info);
  try {
    const owner = await f.account('ws-owner'), recipient = await f.account('ws-recipient');
    const d1 = await f.device(owner, 1);
    // A second workspace with its own conversation, created before protection.
    const privatePath = join(f.root, 'private-workspace');
    await mkdir(privatePath);
    await writeFile(join(privatePath, 'secret.md'), '# private\n');
    const privateWs = ok(await f.api(d1.api + '/workspaces', 'POST', { label: 'Private', absPath: privatePath }, owner.cookie), 'Private workspace');
    const privateThread = ok(await f.api(d1.api + '/threads/start', 'POST', { workspaceId: privateWs.id, model: 'fake', title: 'Private notes' }, owner.cookie), 'Private thread');
    const privateId = privateThread.thread?.id ?? privateThread.id;
    const sharedWs = ok(await f.api(d1.api + `/threads/${d1.thread}`, 'GET', undefined, owner.cookie), 'Shared thread').workspace;
    await writeFile(join(sharedWs.absPath, 'notes.md'), '# shared\n');
    await f.bind(owner, d1);

    const dialog = await shareDialog(owner.page, d1.url);
    await expect(dialog.getByRole('radio', { name: /^This thread/ })).toBeChecked();
    await expect(dialog.getByRole('checkbox', { name: /Can create new threads/ })).toHaveCount(0);
    await dialog.getByRole('radio', { name: /^This workspace/ }).check();
    await expect(dialog.getByText(`Every thread in ${sharedWs.label}, with the permissions below.`)).toBeVisible();
    await dialog.getByRole('checkbox', { name: /Can create new threads/ }).check();
    await dialog.getByPlaceholder('username or email').fill('ws-recipient');
    await dialog.getByRole('radio', { name: 'Collaborator', exact: true }).check();
    await dialog.getByRole('radio', { name: 'Read files', exact: true }).check();
    await expect(dialog.getByText('People you share with never get a terminal.')).toBeVisible();
    await screenshot(owner.page, info, `workspace-share-dialog-${info.project.name}`);
    const created = owner.page.waitForRequest(request => request.url().endsWith('/relay/grants') && request.method() === 'POST');
    await dialog.getByRole('button', { name: 'Share workspace', exact: true }).click();
    expect((await created).postDataJSON()).toMatchObject({
      scope: 'workspace', workspaceId: sharedWs.id, workspaceLabel: sharedWs.label, targetIdentifier: 'ws-recipient',
      threadAccess: 'control', workspaceAccess: 'read', canCreateThreads: true,
    });
    await approveGrant(owner.page, owner.password, true);
    await expect(dialog.getByText(`Workspace ${sharedWs.label} ·`, { exact: false })).toBeVisible();
    // The protected device holds a signed grant for exactly that workspace.
    const grants = ok(await browserRequest(owner.page, d1.api + '/transport/grants'), 'Device grants').grants;
    expect(grants.find((grant: { relayRef: { kind: string } }) => grant.relayRef.kind === 'grant')).toMatchObject({
      scope: 'workspace', workspaceId: sharedWs.id, threadId: null, threadAccess: 'control', workspaceAccess: 'read',
      canCreateThreads: true, grantee: { userId: recipient.userId },
    });
    await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click();

    // The recipient opens the workspace from the device's card on Devices.
    await recipient.page.goto(f.base + '/relay-devices');
    await recipient.page.getByRole('tab', { name: /Devices with me/ }).click();
    const card = recipient.page.locator('.shared-access-card').filter({ hasText: sharedWs.label });
    await expect(card).toBeVisible();
    await card.getByRole('button', { name: /Open/ }).click();
    await expect(recipient.page).toHaveURL(new RegExp(`/devices/${d1.id}/threads\\?workspaceId=${sharedWs.id}$`));
    await expect(recipient.page.getByText('Signed thread 1').first()).toBeVisible();
    await expect(recipient.page.getByText('Private notes')).toHaveCount(0);
    await screenshot(recipient.page, info, `workspace-share-recipient-list-${info.project.name}`);

    // Device and relay both confine the share: the workspace's threads and files only, no terminal.
    const call = (path: string, method = 'GET', body?: unknown) => browserRequest(recipient.page, d1.api + path, method, body);
    expect((await call(`/threads/${d1.thread}`)).status).toBe(200);
    expect(ok(await call(`/workspaces/${sharedWs.id}/threads`), 'Shared workspace threads').map((t: { id: string }) => t.id)).toEqual([d1.thread]);
    expect((await call(`/workspaces/${sharedWs.id}/files/preview?path=notes.md`)).status).toBe(200);
    for (const [method, path] of [
      ['GET', `/threads/${privateId}`], ['GET', '/threads'], ['GET', '/workspaces'],
      ['GET', `/workspaces/${privateWs.id}/threads`], ['GET', `/workspaces/${privateWs.id}/files/tree`],
      ['POST', `/threads/${d1.thread}/shell`], ['GET', `/threads/${d1.thread}/shell`], ['POST', `/threads/${d1.thread}/commands`],
      ['POST', '/threads/start'],
    ] as const) {
      const status = (await call(path, method, method === 'POST' ? { workspaceId: sharedWs.id } : undefined)).status;
      expect(status, `${method} ${path}`).toBeGreaterThanOrEqual(401);
      expect(status, `${method} ${path}`).toBeLessThan(404);
    }
    expect((await call(`/workspaces/${sharedWs.id}/files/preview?path=${encodeURIComponent('../private-workspace/secret.md')}`)).status).not.toBe(200);

    // A conversation in the shared workspace has no terminal anywhere.
    await recipient.page.goto(d1.url);
    await expect(recipient.page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
    await expect(recipient.page.getByRole('button', { name: /^Terminal/ })).toHaveCount(0);
    await expect(recipient.page.getByRole('button', { name: 'Switch to shell', exact: true })).toHaveCount(0);
    await screenshot(recipient.page, info, `workspace-share-recipient-thread-${info.project.name}`);

    // New threads start only in the shared workspace, through its own route.
    await recipient.page.goto(`${f.base}/devices/${d1.id}/threads/new?workspaceId=${sharedWs.id}`);
    const create = recipient.page.getByRole('button', { name: /^Create thread/i });
    await expect(create).toBeEnabled({ timeout: 30_000 });
    await screenshot(recipient.page, info, `workspace-share-new-thread-${info.project.name}`);
    const start = recipient.page.waitForResponse(response => response.url().includes(`/api/workspaces/${sharedWs.id}/threads`) && response.request().method() === 'POST');
    await create.click();
    expect((await start).status()).toBe(200);
    await expect(recipient.page).toHaveURL(new RegExp(`/devices/${d1.id}/threads/[0-9a-f-]{36}$`));
  } finally {
    await f.close();
  }
});
