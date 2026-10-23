import { test, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import { pathToFileURL } from 'node:url';

test('workbench shares images publicly and grants device access without leaving the thread', async ({ browser }, testInfo) => {
  const root = await mkdtemp(resolve('.local/workbench-sharing-'));
  const processes: ChildProcess[] = [];
  const freePort = () => new Promise<number>(done => { const server = createServer(); server.listen(0, '127.0.0.1', () => { const port = (server.address() as { port: number }).port; server.close(() => done(port)); }); });
  const rp = await freePort(), sp = await freePort();
  const base = `http://127.0.0.1:${rp}`;
  const password = randomBytes(24).toString('hex');
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  const start = (command: string, extra: Record<string, string>) => {
    const proc = spawn(resolve('target/debug/pockymoe'), [command], { env: { ...environment, HOST: '127.0.0.1', POCKYMOE_ADMIN_USERNAME: 'testadmin', POCKYMOE_ADMIN_PASSWORD: password, POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'), POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'), POCKYMOE_E2E_FAKE_RUNTIME: '1', ...extra }, stdio: 'ignore' });
    processes.push(proc);
  };
  const api = async (path: string, token?: string, body?: unknown) => {
    const response = await fetch(`${base}${path}`, { method: body ? 'POST' : 'GET', headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body ? { 'content-type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}), signal: AbortSignal.timeout(10000) });
    expect(response.ok, `${path}: ${response.status}`).toBeTruthy();
    return response.json();
  };
  const context = await browser.newContext();
  try {
    start('relay', { PORT: String(rp), POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'), POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'), POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'), POCKYMOE_PUBLIC_BASE_URL: base });
    await expect.poll(() => fetch(`${base}/healthz`).then(r => r.status).catch(() => 0)).toBe(200);
    for (const username of ['owner', 'reviewer']) await api('/relay/auth/register', undefined, { username, email: `${username}@example.test`, password });
    const owner = (await api('/relay/auth/login', undefined, { username: 'owner', password })).token;
    // This spec shares through the relay API and tests access, not the
    // safety-code check (workspace-sharing.spec.ts covers it): turn it off.
    expect((await fetch(`${base}/relay/account/preferences`, { method: 'PATCH', headers: { authorization: `Bearer ${owner}`, 'content-type': 'application/json' }, body: JSON.stringify({ requireSafetyCode: false }) })).ok).toBe(true);
    const device = await api('/relay/devices', owner, { name: 'Isolated UI device' });
    start('relay-supervisor', { PORT: String(sp), POCKYMOE_RELAY_SUPERVISOR_PORT: String(sp), POCKYMOE_RELAY_SERVER_URL: base, POCKYMOE_RELAY_AGENT_TOKEN: device.token, POCKYMOE_DATABASE_PATH: join(root, 'supervisor.sqlite'), POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces') });
    await expect.poll(async () => (await api('/healthz')).connectedSupervisors).toBe(1);
    const deviceApi = `/relay/devices/${device.device.id}/api`;
    const absPath = join(root, 'workspace');
    await mkdir(absPath);
    const workspace = await api(`${deviceApi}/workspaces`, owner, { absPath, label: 'Sharing review' });
    const started = await api(`${deviceApi}/threads/start`, owner, { workspaceId: workspace.id, model: 'fake', title: 'Image sharing review' });
    const id = started.id ?? started.thread.id;
    await mkdir(join(absPath, '.temp/threads', id), { recursive: true });
    await writeFile(join(absPath, '.temp/threads', id, 'image.png'), Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=', 'base64'));
    await api(`${deviceApi}/threads/${id}/prompt`, owner, { prompt: `Review this attachment [PHOTO ./.temp/threads/${id}/image.png]` });
    await expect.poll(async () => (await api(`${deviceApi}/threads/${id}`, owner)).thread.status).toBe('idle');
    // The reported conversation contains these examples in an assistant reply.
    // They are text, not uploaded images, and must not be fetched as attachments.
    const example = 'Syntax examples: `[PHOTO …]` and `[PHOTO ./.temp/threads/…/image.png]`.';
    const db = new DatabaseSync(join(root, 'supervisor.sqlite'));
    try {
      const rows = db.prepare('SELECT id,item_json FROM thread_history_items WHERE thread_id=?').all(id) as { id: string; item_json: string }[];
      for (const row of rows) {
        const item = JSON.parse(row.item_json);
        if (item.kind === 'agentMessage') {
          item.text += `\n\n${example}`;
          db.prepare('UPDATE thread_history_items SET item_json=? WHERE id=?').run(JSON.stringify(item), row.id);
        }
      }
    } finally { db.close(); }
    await context.addCookies([{ name: 'remote_codex_relay_session', value: owner, url: base }]);
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    const page = await context.newPage();
    const contrast = async (selector: string) => page.locator(selector).evaluate(element => {
      const style = getComputedStyle(element);
      const luminance = (color: string) => {
        const channels = color.match(/[\d.]+/g)!.slice(0, 3).map(Number).map(c => { const s = c / 255; return s <= .04045 ? s / 12.92 : ((s + .055) / 1.055) ** 2.4; });
        return channels[0]! * .2126 + channels[1]! * .7152 + channels[2]! * .0722;
      };
      const a = luminance(style.color), b = luminance(style.backgroundColor);
      return (Math.max(a, b) + .05) / (Math.min(a, b) + .05);
    });
    let releaseCreate!: () => void;
    const createGate = new Promise<void>(resolve => { releaseCreate = resolve; });
    await page.route('**/relay/thread-links', async route => { if (route.request().method() === 'POST') await createGate; await route.continue(); });
    await page.addInitScript(() => localStorage.setItem('remote-codex-theme-mode', 'dark'));
    await page.goto(`${base}/devices/${device.device.id}/threads/${id}`);
    await page.getByRole('button', { name: 'Share as link', exact: true }).click();
    await expect(page.getByRole('checkbox', { name: /Keep updated/ })).not.toBeChecked();
    await expect(page.getByRole('textbox', { name: 'Public share URL' })).toHaveCount(0);
    await page.getByRole('button', { name: 'Create & copy link', exact: true }).click();
    try {
      await expect(page.getByRole('button', { name: 'Creating link…' })).toBeVisible();
      expect(await contrast('.thread-public-link-create')).toBeGreaterThanOrEqual(4.5);
    } finally { releaseCreate(); }
    const link = page.getByRole('textbox', { name: 'Public share URL' });
    await expect(link).toBeVisible();
    const url = await link.inputValue();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(url);
    const createButton = page.getByRole('button', { name: 'Create & copy link', exact: true });
    await expect(createButton).toBeEnabled();
    expect(await contrast('.thread-public-link-create')).toBeGreaterThanOrEqual(4.5);
    await createButton.hover();
    expect(await contrast('.thread-public-link-create')).toBeGreaterThanOrEqual(4.5);
    await createButton.focus();
    expect(await contrast('.thread-public-link-create')).toBeGreaterThanOrEqual(4.5);
    await page.screenshot({ path: `output/playwright/share-link-contrast-${testInfo.project.name}.png` });
    const anonymous = await browser.newContext();
    try {
      const publicPage = await anonymous.newPage();
      await publicPage.goto(url);
      await expect(publicPage.getByText('Review this attachment', { exact: true })).toBeVisible();
      await expect(publicPage.getByText('[PHOTO …]', { exact: true })).toBeVisible();
      await expect(publicPage.getByText('[PHOTO ./.temp/threads/…/image.png]', { exact: true })).toBeVisible();
      await expect(publicPage.locator('img')).toHaveAttribute('src', /^data:image\/png;base64,/);
      await expect.poll(() => publicPage.locator('img').evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(1);
    } finally { await anonymous.close(); }
    await page.getByRole('button', { name: 'Close dialog', exact: true }).click();
    await page.getByRole('button', { name: 'Sharing permissions', exact: true }).click();
    await expect(page.getByRole('radio', { name: /^This thread/ })).toBeChecked();
    const scope = page.getByRole('radio', { name: /^Whole device/ });
    await expect(scope).not.toBeChecked();
    await scope.check();
    await page.getByPlaceholder('username or email').fill('reviewer');
    await expect(page.locator('.matter-actions-dialog')).toHaveAttribute('data-theme-effective', 'dark');
    await expect(page.locator('.thread-export-dialog-panel')).toHaveCSS('background-color', 'rgb(15, 19, 23)');
    await page.screenshot({ path: `output/playwright/sharing-dark-${testInfo.project.name}.png` });
    const grant = page.waitForRequest(request => request.url().endsWith('/relay/grants') && request.method() === 'POST');
    await page.getByRole('button', { name: 'Share device', exact: true }).click();
    expect((await grant).postDataJSON()).toMatchObject({ scope: 'device', targetIdentifier: 'reviewer', threadAccess: 'read', workspaceAccess: 'none', canCreateThreads: false });
    await expect(page.getByText('reviewer', { exact: true })).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`/threads/${id}$`));
    const reviewer = (await api('/relay/auth/login', undefined, { username: 'reviewer', password })).token;
    expect((await api(`${deviceApi}/threads/${id}`, reviewer)).thread.id).toBe(id);
    await page.getByRole('button', { name: 'Revoke', exact: true }).click();
    await expect(page.getByText('reviewer', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Close dialog', exact: true }).click();
    await page.getByRole('button', { name: 'Notifications', exact: true }).click();
    await expect(page.locator('.matter-notification-summary').first()).toContainText('ok: Review this attachment');
    await expect(page.locator('.matter-notifications')).toContainText('Image sharing review');

    // A downloaded file has no React runtime. Its image viewer must work offline.
    await page.reload();
    await page.getByRole('button', { name: 'Download transcript', exact: true }).click();
    const downloaded = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Export HTML', exact: true }).click();
    const htmlPath = testInfo.outputPath('image-transcript.html');
    await (await downloaded).saveAs(htmlPath);
    const offline = await browser.newContext({ offline: true });
    try {
      const file = await offline.newPage();
      await file.goto(pathToFileURL(htmlPath).href);
      await file.getByRole('button', { name: /Enlarge image/ }).first().click();
      const viewer = file.getByRole('dialog', { name: 'Image preview' });
      await expect(viewer).toBeVisible();
      await expect.poll(() => viewer.locator('img').evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
      await viewer.getByRole('button', { name: 'Actual size' }).click();
      await expect(viewer).toHaveAttribute('data-zoom', 'true');
      await file.keyboard.press('Escape');
      await expect(viewer).not.toBeVisible();
      await expect(file.getByRole('button', { name: /Enlarge image/ }).first()).toBeFocused();
    } finally { await offline.close(); }

    // Exclude an existing middle turn, then publish a future turn with the owner
    // browser closed. Fixed links must not change, and live links must not leak it.
    await api(`${deviceApi}/threads/${id}/prompt`, owner, { prompt: 'Private middle turn' });
    await expect.poll(async () => (await api(`${deviceApi}/threads/${id}`, owner)).thread.status).toBe('idle');
    await page.reload();
    await page.getByRole('button', { name: 'Share as link', exact: true }).click();
    await page.getByLabel('Choose turns', { exact: true }).check();
    await page.getByRole('checkbox', { name: 'Share turn 2', exact: true }).uncheck();
    await page.getByRole('checkbox', { name: /Keep updated/ }).check();
    await page.screenshot({ path: 'output/playwright/selective-live-share.png' });
    await page.getByRole('button', { name: 'Create & copy link', exact: true }).click();
    const liveUrl = page.locator('.thread-export-dialog-box').filter({ hasText: 'Live ·' }).getByRole('textbox', { name: 'Public share URL' });
    await expect(liveUrl).toBeVisible();
    const liveAddress = await liveUrl.inputValue();
    await page.close();
    const readers = await browser.newContext();
    try {
      const livePage = await readers.newPage(), fixedPage = await readers.newPage();
      await livePage.goto(liveAddress);
      await fixedPage.goto(url);
      await expect(livePage.getByText('Review this attachment', { exact: true })).toBeVisible();
      await api(`${deviceApi}/threads/${id}/prompt`, owner, { prompt: 'Future public turn' });
      await expect(livePage.getByText('ok: Future public turn', { exact: true })).toBeVisible();
      await expect(livePage.getByText('Private middle turn', { exact: true })).toHaveCount(0);
      await fixedPage.reload();
      await expect(fixedPage.getByText('Review this attachment', { exact: true })).toBeVisible();
      await expect(fixedPage.getByText('Future public turn', { exact: true })).toHaveCount(0);
      const linkId = new URL(liveAddress).pathname.split('/').pop();
      const revoked = await fetch(`${base}/relay/public-links/${linkId}`, { method: 'DELETE', headers: { authorization: `Bearer ${owner}` } });
      expect(revoked.status).toBe(204);
      await expect(livePage.getByText('This share link is unavailable or has been revoked.')).toBeVisible();
      await expect(livePage.getByText('Future public turn', { exact: true })).toHaveCount(0);
    } finally { await readers.close(); }
  } finally {
    await context.close();
    await Promise.all(processes.map(proc => new Promise<void>(done => { if (proc.exitCode !== null || proc.signalCode !== null) return done(); proc.once('exit', () => done()); proc.kill('SIGTERM'); })));
  }
});
