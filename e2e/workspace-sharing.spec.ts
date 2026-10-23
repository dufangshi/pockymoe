import { test, expect, type Browser, type TestInfo } from '@playwright/test';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';
import { Fixture, approveGrant, browserRequest, ok, screenshot, shareDialog } from './helpers/account-key-fixture';

const THEMES = ['classic', 'plum-pocket', 'terminal'] as const;
/** Every theme, light and dark, at a desktop and a phone width: `prepare` brings
    the page back to the state to capture after each reload. */
async function inEveryTheme(page: import('@playwright/test').Page, info: TestInfo, name: string, prepare: () => Promise<void>) {
  for (const width of [1280, 390]) for (const theme of THEMES) for (const mode of ['light', 'dark']) {
    await page.setViewportSize({ width, height: width > 600 ? 900 : 844 });
    await page.evaluate(([t, m]) => { localStorage.setItem('pockymoe-theme-preset', t!); localStorage.setItem('remote-codex-theme-mode', m!); }, [theme, mode]);
    await page.reload();
    await prepare();
    await shot(page, info, `${name}-${theme}-${mode}-${width}`);
  }
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.evaluate(() => { localStorage.removeItem('pockymoe-theme-preset'); localStorage.removeItem('remote-codex-theme-mode'); });
  await page.reload();
}

// Screenshots for visual review go to E2E_SHOT_DIR when it is set (none otherwise).
async function shot(page: import('@playwright/test').Page, info: TestInfo, name: string) {
  if (process.env.E2E_SHOT_DIR) await page.screenshot({ path: join(process.env.E2E_SHOT_DIR, `${info.project.name}-${name}.png`) });
  else await screenshot(page, info, name);
}

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
// With the safety-code check on (the default), the share waits: the recipient
// sees their code instead of the workspace, the owner types it to approve and
// sign the grant, and the recipient's page opens the workspace. The recipient
// then reaches that workspace's threads and files, starts a thread there, and
// never another workspace or a terminal. With the check off, a share starts at
// once.
test('a signed workspace share waits for the safety code, then reaches its threads and files but never a terminal', async ({ browser }, info) => {
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
    // Nothing is signed yet: the share waits for the recipient's safety code.
    await expect(dialog.getByText(`Workspace ${sharedWs.label} ·`, { exact: false })).toBeVisible();
    await expect(dialog.getByText("Waiting for ws-recipient's safety code", { exact: true })).toBeVisible();
    await expect(owner.page.getByRole('dialog', { name: 'Review device grant', exact: true })).toHaveCount(0);
    await screenshot(owner.page, info, `workspace-share-waiting-dialog-${info.project.name}`);
    const waitingGrants = ok(await browserRequest(owner.page, d1.api + '/transport/grants'), 'Device grants').grants;
    expect(waitingGrants.some((grant: { relayRef: { kind: string } }) => grant.relayRef.kind === 'grant')).toBe(false);
    await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click();

    // The recipient's Sharing tab says it waits, with their code instead of Open.
    await recipient.page.goto(f.base + '/relay-devices?tab=shared');
    const waitingRow = recipient.page.locator('.shared-row').filter({ hasText: sharedWs.label });
    await expect(waitingRow).toContainText('Waiting for ws-owner to confirm');
    await expect(waitingRow.getByRole('button', { name: `Open ${sharedWs.label}`, exact: true })).toHaveCount(0);
    await inEveryTheme(recipient.page, info, 'incoming-waiting', async () => {
      await waitingRow.getByRole('button', { name: new RegExp(`^Workspace ${sharedWs.label}`) }).click();
      await expect(waitingRow.locator('.signed-share-own-code')).toHaveText(recipient.code);
    });
    await waitingRow.getByRole('button', { name: 'Show my code', exact: true }).click();
    await expect(recipient.page).toHaveURL(/\/shared-pending\?kind=grant&id=/);

    // A link into the workspace also leads to the code.
    await recipient.page.goto(`${f.base}/devices/${d1.id}/threads?workspaceId=${sharedWs.id}`);
    await expect(recipient.page).toHaveURL(/\/shared-pending\?kind=grant&id=/);
    await expect(recipient.page.getByRole('heading', { name: 'Waiting for ws-owner to confirm', exact: true })).toBeVisible();
    await expect(recipient.page.getByText(`ws-owner shared the workspace “${sharedWs.label}”`, { exact: false })).toBeVisible();
    await expect(recipient.page.locator('.share-pending-code')).toHaveText(recipient.code);
    await expect(recipient.page.getByRole('button', { name: 'Copy safety code', exact: true })).toBeVisible();
    // The relay refuses everything under the waiting share, and says why.
    const refused = await f.api(d1.api + `/workspaces/${sharedWs.id}/threads`, 'GET', undefined, recipient.cookie);
    expect(refused.status).toBe(403);
    expect(refused.data.code).toBe('share_pending');
    await inEveryTheme(recipient.page, info, 'pending-recipient', async () => {
      await expect(recipient.page.locator('.share-pending-code')).toHaveText(recipient.code);
    });

    // The same share, from the workspace's menu on the device's Workspaces page,
    // whose header names the device.
    await owner.page.goto(`${f.base}/devices/${d1.id}/workspaces`);
    await expect(owner.page.locator('.product-navigation-subtitle')).not.toBeEmpty();
    await owner.page.getByRole('button', { name: `More actions for ${sharedWs.label}`, exact: true }).click();
    await owner.page.getByRole('menuitem', { name: 'Share workspace', exact: true }).click();
    const menuDialog = owner.page.getByRole('dialog');
    await expect(menuDialog.getByText('Choose who can view or collaborate on the threads in this workspace.')).toBeVisible();
    await expect(menuDialog.getByRole('radio', { name: /^This workspace/ })).toBeChecked();
    await expect(menuDialog.getByRole('radio', { name: /^This thread|^Whole device/ })).toHaveCount(0);
    await expect(menuDialog.getByText('ws-recipient', { exact: true })).toBeVisible();
    for (const width of [1280, 390]) for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
      await owner.page.setViewportSize({ width, height: width > 600 ? 720 : 844 });
      await owner.page.evaluate(([t, m]) => {
        localStorage.setItem('pockymoe-theme-preset', t);
        localStorage.setItem('remote-codex-theme-mode', m);
        window.dispatchEvent(new StorageEvent('storage', { key: 'pockymoe-theme-preset' }));
      }, [theme, mode]);
      await owner.page.reload();
      await expect(owner.page.locator('.product-navigation-subtitle')).not.toBeEmpty();
      await screenshot(owner.page, info, `workspace-menu-page-${theme}-${mode}-${width}`);
      await owner.page.getByRole('button', { name: `More actions for ${sharedWs.label}`, exact: true }).click();
      await screenshot(owner.page, info, `workspace-menu-open-${theme}-${mode}-${width}`);
      await owner.page.getByRole('menuitem', { name: 'Share workspace', exact: true }).click();
      await expect(owner.page.getByRole('dialog').getByText('ws-recipient', { exact: true })).toBeVisible();
      await screenshot(owner.page, info, `workspace-menu-share-${theme}-${mode}-${width}`);
    }
    await owner.page.setViewportSize({ width: 1280, height: 720 });
    await owner.page.evaluate(() => { localStorage.removeItem('pockymoe-theme-preset'); localStorage.removeItem('remote-codex-theme-mode'); });

    // The owner's Sharing tab lists the grant as one row waiting for the code.
    await owner.page.goto(f.base + '/relay-devices?tab=shared&dir=out');
    const ownerRow = owner.page.locator('.shared-row').filter({ hasText: sharedWs.label });
    await expect(ownerRow).toBeVisible();
    await expect(ownerRow.getByText('Waiting for safety code')).toBeVisible();
    await expect(owner.page.getByRole('tab', { name: /Sharing/ })).toContainText('1 to review');
    await ownerRow.getByRole('button', { name: new RegExp(sharedWs.label) }).click();
    await expect(ownerRow.getByRole('button', { name: 'Revoke', exact: true })).toBeVisible();
    // The state is said once, with the one action that ends it.
    await expect(ownerRow.getByText("Waiting for ws-recipient's safety code", { exact: true })).toHaveCount(1);
    const codeField = ownerRow.getByLabel('Safety code from ws-recipient');
    await inEveryTheme(owner.page, info, 'pending-owner', async () => {
      await ownerRow.getByRole('button', { name: new RegExp(sharedWs.label) }).click();
      await codeField.scrollIntoViewIfNeeded();
    });
    await ownerRow.getByRole('button', { name: new RegExp(sharedWs.label) }).click();
    // A wrong code explains the risk and approves nothing.
    await codeField.fill('ABCD EFGH JKMN');
    await ownerRow.getByRole('button', { name: 'Check and approve', exact: true }).click();
    await expect(ownerRow.getByRole('alert')).toContainText("does not match ws-recipient's account key");
    await shot(owner.page, info, 'pending-owner-mismatch');
    // The code the recipient sent approves: review, sign, and the relay starts it.
    await codeField.fill(recipient.code.toLowerCase().replaceAll(' ', '-'));
    await ownerRow.getByRole('button', { name: 'Check and approve', exact: true }).click();
    await approveGrant(owner.page, owner.password);
    await expect(ownerRow.getByText('Safety code checked', { exact: true })).toBeVisible();
    await expect(owner.page.getByRole('tab', { name: /Sharing/ })).not.toContainText('to review');
    await shot(owner.page, info, 'approved-owner');
    // The protected device holds a signed grant for exactly that workspace.
    const grants = ok(await browserRequest(owner.page, d1.api + '/transport/grants'), 'Device grants').grants;
    expect(grants.find((grant: { relayRef: { kind: string } }) => grant.relayRef.kind === 'grant')).toMatchObject({
      scope: 'workspace', workspaceId: sharedWs.id, threadId: null, threadAccess: 'control', workspaceAccess: 'read',
      canCreateThreads: true, grantee: { userId: recipient.userId, accountKeyId: recipient.accountKeyId },
    });
    // The recipient's waiting page opens the workspace by itself.
    await expect(recipient.page).toHaveURL(new RegExp(`/devices/${d1.id}/threads\\?workspaceId=${sharedWs.id}$`), { timeout: 20_000 });
    await expect(recipient.page.getByText('Signed thread 1').first()).toBeVisible();

    // Controlled by someone else now, the device names that share as why its
    // agents cannot reach the owner's other devices.
    await owner.page.goto(f.base + '/relay-devices');
    const controlMark = owner.page.getByRole('button', { name: 'Shared for control', exact: true });
    await controlMark.click();
    const reasons = owner.page.getByRole('dialog', { name: 'Shared for control' });
    await expect(reasons).toContainText(`Workspace “${sharedWs.label}”`);
    await expect(reasons).toContainText('with ws-recipient · Collaborator, New threads');
    await shot(owner.page, info, 'shared-control-reasons');
    await owner.page.keyboard.press('Escape');
    await owner.page.goto(f.base + '/relay-devices?tab=shared&dir=out');
    await owner.page.locator('.shared-row').filter({ hasText: sharedWs.label }).getByRole('button', { name: new RegExp(sharedWs.label) }).click();
    for (const view of ['By device', 'By person']) {
      await owner.page.getByRole('button', { name: view, exact: true }).click();
      await expect(owner.page.locator('.shared-row').filter({ hasText: sharedWs.label })).toBeVisible();
    }
    await owner.page.getByRole('button', { name: 'List', exact: true }).click();

    // The recipient opens the workspace from Sharing on the Devices page.
    await recipient.page.goto(f.base + '/relay-devices');
    await recipient.page.getByRole('tab', { name: /Sharing/ }).click();
    await expect(recipient.page).toHaveURL(/tab=shared/);
    const row = recipient.page.locator('.shared-row').filter({ hasText: sharedWs.label });
    await expect(row).toBeVisible();
    await expect(row).toContainText('from ws-owner');
    await row.getByRole('button', { name: `Open ${sharedWs.label}`, exact: true }).click();
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

    // The recipient gives the workspace back: asked first, then it is gone and
    // the relay refuses its routes.
    await recipient.page.goto(f.base + '/relay-devices?tab=shared');
    const shared = recipient.page.locator('.shared-row').filter({ hasText: sharedWs.label });
    await shared.getByRole('button', { name: new RegExp(`^Workspace ${sharedWs.label}`) }).click();
    await shared.getByRole('button', { name: 'Leave', exact: true }).click();
    const confirm = recipient.page.getByRole('dialog', { name: 'Leave this share?' });
    await expect(confirm).toContainText(`${sharedWs.label}`);
    await expect(confirm).toContainText('ws-owner has to share it again');
    await screenshot(recipient.page, info, `workspace-share-leave-confirm-${info.project.name}`);
    await confirm.getByRole('button', { name: 'Leave', exact: true }).click();
    await expect(confirm).toHaveCount(0);
    await expect(recipient.page.locator('.shared-row').filter({ hasText: sharedWs.label })).toHaveCount(0);
    expect((await call(`/workspaces/${sharedWs.id}/threads`)).status).toBeGreaterThanOrEqual(401);

    // With the check off, a new share starts at once: the owner signs while
    // sharing, and the recipient opens it without a code.
    await owner.page.goto(f.base + '/relay-account');
    const check = owner.page.getByRole('checkbox', { name: 'Require safety code check for people I share with', exact: true });
    await expect(check).toBeChecked();
    await inEveryTheme(owner.page, info, 'settings-sharing', async () => {
      await owner.page.locator('#sharing-settings-heading').scrollIntoViewIfNeeded();
    });
    await check.uncheck();
    await expect(owner.page.getByText('Saved', { exact: true })).toBeVisible();
    await expect(owner.page.getByText(/Off: new shares start at once/)).toBeVisible();
    await shot(owner.page, info, 'settings-sharing-off');
    const privateDialog = await shareDialog(owner.page, `${f.base}/devices/${d1.id}/threads/${privateId}`);
    await privateDialog.getByPlaceholder('username or email').fill('ws-recipient');
    await privateDialog.getByRole('button', { name: 'Share this thread', exact: true }).click();
    await approveGrant(owner.page, owner.password);
    await expect(privateDialog.getByText('Safety code checked', { exact: true })).toBeVisible();
    expect((await call(`/threads/${privateId}`)).status).toBe(200);

    // A share the protected device does not hold (as one made before the device
    // was protected): its details say the recipient cannot open it, once, and
    // one action signs it onto the device.
    const threadGrant = ok(await browserRequest(owner.page, d1.api + '/transport/grants'), 'Device grants').grants
      .find((grant: { relayRef: { kind: string }; threadId: string }) => grant.relayRef.kind === 'share' && grant.threadId === privateId);
    ok(await browserRequest(owner.page, d1.api + `/transport/grants/${threadGrant.grantId}`, 'DELETE'), 'Remove the device grant');
    expect((await call(`/threads/${privateId}`)).status).toBe(403);
    await owner.page.goto(f.base + '/relay-devices?tab=shared&dir=out');
    const missingRow = owner.page.locator('.shared-row').filter({ hasText: 'Private notes' });
    await expect(missingRow.getByText("Can't open: not signed on device")).toBeVisible();
    await inEveryTheme(owner.page, info, 'missing-owner', async () => {
      await missingRow.getByRole('button', { name: /Private notes/ }).click();
      await expect(missingRow.getByText("ws-recipient can't open this now", { exact: true })).toHaveCount(1);
    });
    await missingRow.getByRole('button', { name: /Private notes/ }).click();
    await missingRow.getByRole('button', { name: 'Sign onto device', exact: true }).click();
    await approveGrant(owner.page, owner.password);
    await expect(missingRow.getByText('Safety code checked', { exact: true })).toBeVisible();
    await expect.poll(async () => (await call(`/threads/${privateId}`)).status).toBe(200);
  } finally {
    await f.close();
  }
});
