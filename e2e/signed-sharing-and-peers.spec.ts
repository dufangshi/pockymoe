import { test, expect, type Browser, type BrowserContext, type Page, type TestInfo } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

// Pair fragments, cookies and synced virtual passkeys must never enter traces.
// This spec owns the relay on E2E_API_PORT and two Supervisors on 16272/16273.
// Use the isolated config recorded in .scratch/wave2-playwright.md, without the
// root config's unrelated local Supervisor/Vite servers occupying those ports.
test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

type Result = { status: number; data: any; encrypted?: boolean };
type Device = { id: string; db: string; cli: string; api: string; thread: string; url: string };
type Account = { page: Page; context: BrowserContext; auth: Awaited<ReturnType<typeof authenticator>>; cookie: string; cookies: Awaited<ReturnType<BrowserContext['cookies']>>; credentials: any[]; records: any[]; code: string; userId: string };

async function authenticator(page: Page) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: { protocol: 'ctap2', transport: 'internal', hasResidentKey: true,
      hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
  });
  return { cdp, authenticatorId };
}
async function browserRequest(page: Page, path: string, method = 'GET', body?: unknown): Promise<Result> {
  // Use the real Service Worker and the browser's real signed session.
  return page.evaluate(async ({ path, method, body }) => {
    const response = await fetch(path, { method, credentials: 'same-origin',
      headers: { 'content-type': 'application/json' }, signal: AbortSignal.timeout(15_000),
      ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
    const text = await response.text();
    return { status: response.status, data: text ? JSON.parse(text) : null };
  }, { path, method, body });
}
async function screenshot(page: Page, info: TestInfo, name: string) {
  await mkdir(resolve('.scratch/screenshots'), { recursive: true });
  await page.screenshot({ path: resolve(`.scratch/screenshots/${info.project.name}-${name}.png`) });
}
function ok(result: Result, label: string) {
  // Auth/device bodies contain tokens, so assertion diagnostics use status only.
  expect(result.status, label).toBeGreaterThanOrEqual(200);
  expect(result.status, label).toBeLessThan(300);
  return result.data;
}
function sockets(page: Page) {
  const channels: { closed: boolean; frames: string[]; opened: boolean; closedAt?: number }[] = [];
  page.on('websocket', ws => {
    if (!new URL(ws.url()).pathname.endsWith('/ws')) return;
    const channel = { closed: false, frames: [] as string[], opened: true, closedAt: undefined as number | undefined };
    channels.push(channel);
    ws.on('framereceived', frame => channel.frames.push(String(frame.payload)));
    ws.on('close', () => { channel.closed = true; channel.closedAt = Date.now(); });
  });
  return channels;
}
async function sendPrompt(page: Page, marker: string) {
  await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill(`Reply with exactly ${marker}`);
  await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  await expect(page.getByText(marker, { exact: true })).toBeVisible();
}
async function unlock(page: Page, info: TestInfo, capture = false) {
  const dialog = page.getByRole('dialog', { name: 'Unlock your devices', exact: true });
  await expect(dialog).toBeVisible();
  const button = dialog.getByRole('button', { name: 'Unlock with passkey', exact: true });
  await expect(button).toBeEnabled();
  if (capture) { await expect(button).toBeInViewport({ ratio: 1 }); await screenshot(page, info, 'recipient-unlock'); }
  await button.click();
  await expect(dialog).toHaveCount(0);
}
async function shareDialog(page: Page, url: string) {
  await page.goto(url);
  await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
  await page.locator('[aria-controls="matter-thread-tools"]').click();
  await page.getByRole('button', { name: 'Sharing permissions', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Share this thread', exact: true })).toBeVisible();
  return dialog;
}

class Fixture {
  readonly binary = resolve('target/debug/pockymoe');
  readonly base = `http://localhost:${process.env.E2E_API_PORT ?? 19991}`;
  readonly password = randomBytes(24).toString('hex');
  readonly secrets = [this.password];
  readonly processes: ChildProcess[] = [];
  readonly contexts: BrowserContext[] = [];
  readonly logs: string[] = [];
  readonly events: string[] = [];
  readonly env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  constructor(readonly root: string, readonly browser: Browser, readonly info: TestInfo) {}
  start(mode: string, extra: Record<string, string>) {
    const secret = randomBytes(32).toString('hex'); this.secrets.push(secret);
    const child = spawn(this.binary, [mode], { env: { ...this.env,
      // Even the relay and every CLI get an isolated HOME. Never inherit the
      // managed thread's live URL, token, CLI config or runtime database.
      HOME: join(this.root, 'home'), HOST: '127.0.0.1', POCKYMOE_E2E_FAKE_RUNTIME: '1',
      POCKYMOE_ADMIN_USERNAME: 'sharing-admin', POCKYMOE_ADMIN_PASSWORD: this.password,
      POCKYMOE_SESSION_SECRET: secret, POCKYMOE_RELAY_SESSION_SECRET: secret, ...extra },
      stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout?.on('data', data => this.logs.push(`[${mode}] ${String(data)}`));
    child.stderr?.on('data', data => this.logs.push(`[${mode}] ${String(data)}`));
    this.processes.push(child);
  }
  async cli(args: string[]) {
    return new Promise<{ code: number | null; output: string; data: any }>((done, fail) => {
      let output = '';
      const child = spawn(this.binary, args, { env: { ...this.env, HOME: join(this.root, 'home') }, stdio: ['ignore', 'pipe', 'pipe'] });
      child.stdout.on('data', data => { output += String(data); });
      child.stderr.on('data', data => { output += String(data); });
      child.once('error', () => fail(new Error('Isolated CLI failed to start')));
      child.once('exit', code => { let data; try { data = JSON.parse(output); } catch { /* error/QR */ } done({ code, output, data }); });
    });
  }
  async api(path: string, method = 'GET', body?: unknown, cookie?: string): Promise<Result> {
    const response = await fetch(this.base + path, { method, headers: { 'content-type': 'application/json', origin: this.base, ...(cookie ? { cookie } : {}) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(10_000) });
    const text = await response.text(), data = text ? JSON.parse(text) : null;
    this.events.push(`${method} ${path.split('?')[0]} ${response.status}${data?.code ? ` code=${data.code}` : ''}`);
    return { status: response.status, data, encrypted: response.headers.get('x-rcd-encrypted') === '1' };
  }
  async context(account?: Account) {
    const context = await this.browser.newContext(this.info.project.use);
    this.contexts.push(context);
    await context.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
    if (account) await context.addCookies(account.cookies);
    context.on('response', response => {
      const path = new URL(response.url()).pathname;
      if (path.startsWith('/relay/')) this.events.push(`browser ${response.request().method()} ${path} ${response.status()} encrypted=${response.headers()['x-rcd-encrypted'] === '1'}`);
    });
    const page = await context.newPage(), auth = await authenticator(page);
    if (account) for (const credential of account.credentials) await auth.cdp.send('WebAuthn.addCredential', { authenticatorId: auth.authenticatorId, credential });
    return { context, page, auth };
  }
  async account(username: string): Promise<Account> {
    ok(await this.api('/relay/auth/register', 'POST', { username, email: `${username}@example.test`, password: this.password }), 'Register real relay account');
    const { context, page, auth } = await this.context();
    await page.goto(this.base + '/relay-portal?returnTo=%2Frelay-account');
    await page.getByLabel('Email or username', { exact: true }).fill(username);
    await page.getByLabel('Password', { exact: true }).fill(this.password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Security', exact: true })).toBeVisible();
    const cookies = await context.cookies(this.base);
    const session = cookies.find(c => c.name === 'remote_codex_relay_session');
    expect(!!session, 'Real sign-in must create a relay cookie').toBe(true); this.secrets.push(session!.value);
    const cookie = `remote_codex_relay_session=${session!.value}`;
    const userId = ok(await this.api('/relay/auth/session', 'GET', undefined, cookie), 'Account session').user.id;
    await context.addInitScript(id => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, `relay:${id}`])}`,
      JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })), userId);
    await page.getByRole('button', { name: 'Add passkey', exact: true }).click();
    await page.getByRole('textbox', { name: 'Passkey name', exact: true }).fill(`${username} passkey`);
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect(page.getByText(`${username} passkey`, { exact: true })).toBeVisible();
    const records = ok(await this.api('/relay/account/passkeys/public', 'GET', undefined, cookie), 'Public passkeys').passkeys;
    expect(records).toHaveLength(1);
    const credentials = (await auth.cdp.send('WebAuthn.getCredentials', { authenticatorId: auth.authenticatorId })).credentials;
    expect(credentials.length, 'One virtual passkey was registered').toBe(1); this.secrets.push(credentials[0].privateKey);
    const code = createHash('sha256').update('pockymoe-grantee-v1\n' + records.map((r: any) => r.credentialId).sort().join('\n')).digest('base64url').slice(0, 12).match(/.{4}/g)!.join(' ');
    await expect(page.getByRole('heading', { name: 'Passkey safety code', exact: true })).toBeVisible();
    await expect(page.locator('code').filter({ hasText: code })).toHaveText(code);
    return { context, page, auth, cookie, cookies, credentials, records, code, userId };
  }
  async device(owner: Account, count: number): Promise<Device> {
    const created = ok(await this.api('/relay/devices', 'POST', { name: `Signed device ${count}` }, owner.cookie), 'Create test device');
    this.secrets.push(created.token);
    const id = created.device.id, db = join(this.root, `device-${count}.sqlite`), workspacePath = join(this.root, `workspace-${count}`);
    const port = String(16271 + count);
    this.start('relay-supervisor', { PORT: port, POCKYMOE_RELAY_SUPERVISOR_PORT: port,
      POCKYMOE_RELAY_SERVER_URL: this.base, POCKYMOE_RELAY_AGENT_TOKEN: created.token,
      POCKYMOE_DATABASE_PATH: db, DATABASE_URL: db,
      POCKYMOE_WORKSPACE_ROOT: workspacePath, WORKSPACE_ROOT: workspacePath });
    await expect.poll(async () => (await this.api('/healthz')).data.connectedSupervisors).toBe(count);
    await mkdir(workspacePath, { recursive: true });
    const api = `/relay/devices/${id}/api`;
    const workspace = ok(await this.api(api + '/workspaces', 'POST', { label: `Signed workspace ${count}`, absPath: workspacePath }, owner.cookie), 'Seed workspace');
    const result = ok(await this.api(api + '/threads/start', 'POST', { workspaceId: workspace.id, model: 'fake', title: `Signed thread ${count}` }, owner.cookie), 'Seed fake-runtime thread');
    const thread = result.thread?.id ?? result.id;
    return { id, db, cli: db.replace(/\.sqlite$/, '.cli.json'), api, thread, url: `${this.base}/devices/${id}/threads/${thread}` };
  }
  async pair(owner: Account, device: Device, alreadyUnlocked = false) {
    // Initialize the transport identity as in the Wave 1 fixture, before asking
    // the CLI to read its sidecar. Lazy-sidecar coverage belongs to that spec.
    ok(await this.api(device.api + '/transport/key?challenge=wave2-bootstrap', 'GET', undefined, owner.cookie), 'Initialize transport identity');
    const result = await this.cli(['pair', '--database', device.db, '--web-origin', this.base]);
    expect(result.code, 'Pair offer creation').toBe(0);
    const url = result.output.match(/http:\/\/localhost:\d+\/pair#[^\s\x1b]+/)?.[0];
    expect(!!url, 'Pair URL exists; secret stays in memory').toBe(true);
    this.secrets.push(url!, new URLSearchParams(new URL(url!).hash.slice(1)).get('secret')!);
    await owner.auth.cdp.send('Page.navigate', { url: url! }).catch(() => { throw new Error('Pair navigation failed'); });
    await expect(owner.page.getByRole('button', { name: 'Pair with passkey', exact: true })).toBeVisible();
    expect(new URL(owner.page.url()).hash).toBe('');
    await owner.page.getByRole('button', { name: 'Pair with passkey', exact: true }).click();
    if (!alreadyUnlocked) {
      await expect(owner.page.getByText('Pairing saved. Unlock with your passkey to finish connecting.', { exact: true })).toBeVisible();
      await owner.page.getByRole('button', { name: 'Unlock with passkey', exact: true }).click();
    }
    await expect(owner.page.getByText('Device paired. Owner passkey protection is enabled.', { exact: true })).toBeVisible();
    await owner.page.getByRole('link', { name: 'Open device', exact: true }).click();
    await expect(owner.page.getByText(`Signed workspace ${device.cli.includes('device-2') ? 2 : 1}`, { exact: true })).toBeVisible();
    const trust = ok(await browserRequest(owner.page, device.api + '/transport/trust'), 'Owner signed trust read');
    expect(trust).toMatchObject({ paired: true, strict: true, deviceId: device.id });
  }
  async relayRow(id: string, access: 'read' | 'control' | 'restore') {
    // Only the disposable relay DB is modified; never read a running device DB.
    const script = 'import json,sqlite3,sys\np=json.load(sys.stdin)\nc=sqlite3.connect(p["db"],timeout=5)\nsql="UPDATE relay_shares SET revoked_at=NULL WHERE id=?" if p["access"]=="restore" else "UPDATE relay_shares SET thread_access=? WHERE id=?"\nargs=(p["id"],) if p["access"]=="restore" else (p["access"],p["id"])\nassert c.execute(sql,args).rowcount==1\nc.commit()\nc.close()\n';
    await new Promise<void>((done, fail) => {
      const child = spawn('python3', ['-c', script], { stdio: ['pipe', 'ignore', 'ignore'] });
      child.stdin.end(JSON.stringify({ db: join(this.root, 'relay', 'relay-store.sqlite'), id, access }));
      child.once('error', () => fail(new Error('Test relay DB update failed')));
      child.once('exit', code => code === 0 ? done() : fail(new Error('Test relay DB update failed')));
    });
  }
  async close() {
    await Promise.all(this.contexts.map(context => context.close()));
    await Promise.all(this.processes.map(child => new Promise<void>(done => {
      if (child.exitCode !== null || child.signalCode !== null) return done();
      const timer = setTimeout(() => child.kill('SIGKILL'), 2_000);
      child.once('exit', () => { clearTimeout(timer); done(); }); child.kill('SIGTERM');
    })));
    const evidence = this.secrets.reduce((text, secret) => text.split(secret).join('[REDACTED]'), [...this.events, ...this.logs].join('\n'))
      .replace(/http[^\s]*\/pair#[^\s]*/g, '[REDACTED PAIR URL]').replace(/(secret=)[^&\s]+/g, '$1[REDACTED]');
    await writeFile(resolve(`.scratch/signed-sharing-${this.info.project.name}.log`), evidence);
    if (this.info.errors.length) await this.info.attach('sanitized-protocol-log', { body: evidence, contentType: 'text/plain' });
    await rm(this.root, { recursive: true, force: true });
  }
}

test('signed sharing, invite pins, revocation and owner-certified peers', async ({ browser }, info) => {
  test.setTimeout(240_000);
  const mobile = info.project.name === 'mobile-chromium';
  await mkdir(resolve('.scratch'), { recursive: true });
  const root = await mkdtemp(resolve('.scratch/signed-sharing-'));
  await mkdir(join(root, 'home'));
  const f = new Fixture(root, browser, info);
  try {
    f.start('relay', { PORT: new URL(f.base).port, POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true',
      POCKYMOE_PUBLIC_BASE_URL: f.base, POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist') });
    await expect.poll(() => fetch(f.base + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
    const owner = await f.account('signed-owner'), recipient = await f.account('signed-recipient');
    expect(recipient.records[0].credentialId).not.toBe(owner.records[0].credentialId);
    const d1 = await f.device(owner, 1);
    await f.pair(owner, d1);
    await owner.page.goto(d1.url);
    await sendPrompt(owner.page, 'SIGNED_HISTORY');
    const dialog = await shareDialog(owner.page, d1.url);
    await dialog.getByPlaceholder('username or email').fill('signed-recipient');
    await expect(dialog.getByRole('radio', { name: 'View only', exact: true })).toBeChecked();
    await dialog.getByRole('button', { name: 'Share this thread', exact: true }).click();
    await expect(dialog.getByText('signed-recipient', { exact: true })).toBeVisible();
    const portal = ok(await f.api('/relay/portal', 'GET', undefined, owner.cookie), 'Owner shares');
    const share = portal.sharedByMe.find((row: any) => row.threadId === d1.thread && row.targetUserId === recipient.userId);
    expect(!!share, 'Real share dialog created relay row').toBe(true);
    const grantResult = await browserRequest(owner.page, d1.api + '/transport/grants');
    const grants = ok(grantResult, 'Owner grant listing').grants;
    const grant = grants.find((row: any) => row.relayRef.kind === 'share' && row.relayRef.id === share.id);
    expect.soft(!!grant, 'SIGNED_SHARE_GRANT: share dialog must create a device grant, not only a relay row').toBe(true);
    if (!grant) throw new Error('SIGNED_SHARE_GRANT blocks recipient and revocation scenarios; no API-created grant substitutes for the real dialog');
    expect(grant).toMatchObject({ version: 1, scope: 'thread', threadId: d1.thread, threadAccess: 'read', grantee: { userId: recipient.userId } });
    expect(grant.grantee.credentials.map((r: any) => r.credentialId)).toEqual(recipient.records.map((r: any) => r.credentialId));
    await expect(dialog.locator('code').filter({ hasText: recipient.code })).toHaveText(recipient.code);
    await expect(dialog.getByText(/Unverified/)).toBeVisible();
    await dialog.getByRole('button', { name: 'I compared the safety code', exact: true }).click();
    await expect(dialog.getByText(/· Verified/)).toBeVisible();
    if (mobile) await screenshot(owner.page, info, 'signed-share-dialog');
    await owner.context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: f.base });
    await dialog.getByRole('button', { name: 'Copy invite link', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Invite link copied', exact: true })).toBeVisible();
    const invite = await owner.page.evaluate(() => navigator.clipboard.readText());
    const fragment = new URLSearchParams(new URL(invite).hash.slice(1));
    expect(new URL(invite).pathname).toBe('/shared');
    expect(fragment.get('device')).toBe(d1.id); expect(fragment.get('thread')).toBe(d1.thread);
    expect(fragment.get('identity')).toMatch(/^[A-Za-z0-9_-]{43}$/);
    await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click();

    // Recipient uses their separate account and independently registered passkey.
    const recipientChannels = sockets(recipient.page);
    await recipient.page.goto(d1.url);
    await unlock(recipient.page, info, mobile);
    await expect.soft(recipient.page.getByText('SIGNED_HISTORY', { exact: true }), 'RECIPIENT_READ: signed read grant must resume history after recipient unlock').toBeVisible({ timeout: 5_000 });
    // Keep diagnosis visible, then permit the remaining independent checks.
    if (!(await recipient.page.getByText('SIGNED_HISTORY', { exact: true }).isVisible())) await recipient.page.reload();
    await expect(recipient.page.getByText('SIGNED_HISTORY', { exact: true })).toBeVisible();
    await expect.poll(() => recipientChannels.some(c => c.frames.some(frame => frame.includes('"encrypted"')))).toBe(true);
    await expect(recipient.page.getByRole('button', { name: /Encrypted shared connection · identity trusted on first use$/ }).first()).toBeVisible();
    const read = await browserRequest(recipient.page, d1.api + `/threads/${d1.thread}`);
    expect(read.status).toBe(200); expect(read.data.thread.id).toBe(d1.thread);

    // Fresh browser opens the owner-copied invitation before its first handshake.
    const fresh = await f.context(recipient);
    if (mobile) {
      await fresh.context.clearCookies();
      await fresh.page.goto(invite);
      await expect(fresh.page.getByRole('heading', { name: 'Open shared access', exact: true })).toBeVisible();
      await expect(fresh.page.getByRole('link', { name: 'Sign in', exact: true })).toBeVisible();
      expect.soft(await fresh.page.locator('main').textContent(),
        'INVITE_COPY: shared invitation must not describe device pairing or a pairing secret').not.toContain('pairing secret');
      await screenshot(fresh.page, info, 'shared-invite');
      await fresh.context.addCookies(recipient.cookies);
      await fresh.page.reload();
    } else await fresh.page.goto(invite);
    await unlock(fresh.page, info);
    await expect(fresh.page.getByText('SIGNED_HISTORY', { exact: true })).toBeVisible();
    const pinned = fresh.page.getByRole('button', { name: /Encrypted shared connection · invite identity pinned$/ }).first();
    await expect.soft(pinned, 'INVITE_PIN: invitation recipient must be labeled pinned rather than TOFU').toBeVisible({ timeout: 5_000 });
    if (mobile) return; // Mobile provides only the requested visual QA.

    const tampered = await f.context(recipient);
    const bad = new URL(invite); const badFragment = new URLSearchParams(bad.hash.slice(1));
    badFragment.set('identity', 'A'.repeat(43)); bad.hash = badFragment.toString();
    await tampered.page.goto(bad.href);
    await expect.soft(tampered.page.getByRole('alert'), 'INVITE_MISMATCH: tampered identity fragment must block the connection').toContainText('The device identity does not match', { timeout: 5_000 });
    expect(await tampered.page.getByRole('textbox', { name: 'Prompt', exact: true }).count()).toBe(0);

    // Defense in depth: relay refuses a read-only prompt before device dispatch.
    const relayDenied = await browserRequest(recipient.page, d1.api + `/threads/${d1.thread}/prompt`, 'POST', { prompt: 'READ_ONLY_RELAY_REFUSAL' });
    expect(relayDenied.status).toBe(403);
    f.events.push(`read-only relay denial code=${relayDenied.data.code}`);
    // Compromise only the relay row. A newly accepted relay write still must be
    // refused by the unchanged read grant on the paired device.
    await f.relayRow(share.id, 'control');
    const deviceDenied = await browserRequest(recipient.page, d1.api + `/threads/${d1.thread}/prompt`, 'POST', { prompt: 'RELAY_WIDENED_REFUSAL' });
    expect.soft(deviceDenied.status, 'RELAY_WIDENING: paired device must refuse relay-only control').toBe(403);
    expect.soft(deviceDenied.data.code, 'RELAY_WIDENING: refusal must originate from the device grant ACL').toBe('grant_denied');
    await owner.page.goto(f.base + '/relay-devices');
    await owner.page.getByRole('tab', { name: /^Threads by me/ }).click();
    await expect.soft(owner.page.getByText('Device permissions differ from this share', { exact: true }),
      'RECONCILIATION: Shared access must flag relay/device permission mismatch').toBeVisible({ timeout: 5_000 });
    await f.relayRow(share.id, 'read');
    const editing = await shareDialog(owner.page, d1.url);
    await editing.getByRole('button', { name: 'Edit permissions for signed-recipient', exact: true }).click();
    await editing.getByRole('radio', { name: 'Collaborator', exact: true }).check();
    await editing.getByRole('button', { name: 'Save permissions', exact: true }).click();
    await expect.poll(async () => ok(await browserRequest(owner.page, d1.api + '/transport/grants'), 'Updated grants').grants.find((g: any) => g.grantId === grant.grantId)?.threadAccess).toBe('control');
    await editing.getByRole('button', { name: 'Close dialog', exact: true }).click();
    await recipient.page.reload();
    await expect(recipient.page.getByRole('button', { name: 'Send Prompt', exact: true })).toBeEnabled();
    await expect.poll(() => recipientChannels.filter(c => !c.closed).some(c => c.frames.some(frame => frame.includes('"encrypted"')))).toBe(true);
    const active = recipientChannels.filter(c => !c.closed).at(-1)!;
    const beforeReply = active.frames.length;
    await sendPrompt(recipient.page, 'RECIPIENT_CONTROL_REPLY');
    expect(active.frames.slice(beforeReply).some(frame => frame.includes('"encrypted"')), 'Recipient control reply arrives over encrypted WebSocket').toBe(true);
    expect(recipientChannels.some(c => c.frames.some(frame => /SIGNED_HISTORY|RECIPIENT_CONTROL_REPLY/.test(frame))), 'Relay cannot read history or control replies').toBe(false);

    const revoking = await shareDialog(owner.page, d1.url);
    const revokedAt = Date.now();
    await revoking.getByRole('button', { name: 'Revoke', exact: true }).click();
    await expect(revoking.getByText('signed-recipient', { exact: true })).toHaveCount(0);
    expect(ok(await browserRequest(owner.page, d1.api + '/transport/grants'), 'Revoked grants').grants.some((g: any) => g.grantId === grant.grantId)).toBe(false);
    // Restore only the relay ACL so neither HTTP denial nor WS shutdown can be
    // explained solely by the relay row's disappearance.
    await f.relayRow(share.id, 'restore');
    const revokedRead = await browserRequest(recipient.page, d1.api + `/threads/${d1.thread}`);
    f.events.push(`revoked next read status=${revokedRead.status} code=${revokedRead.data.code}`);
    expect.soft(revokedRead.status, 'REVOCATION_READ: restored relay ACL cannot restore device access').toBeGreaterThanOrEqual(400);
    expect.soft(['grant_denied', 'owner_session_required'].includes(revokedRead.data.code),
      `REVOCATION_CODE: next read returned ${revokedRead.data.code}; task expects grant_denied/owner_session_required`).toBe(true);
    await expect.configure({ soft: true }).poll(() => active.closed, { timeout: Math.max(100, 5_000 - (Date.now() - revokedAt)), message: 'REVOCATION_WS: existing secure channel must close within ~5 seconds' }).toBe(true);
    expect.soft(active.closedAt !== undefined && active.closedAt - revokedAt <= 5_000,
      'REVOCATION_WS_DEADLINE: channel close timestamp must be within ~5 seconds of the UI revoke').toBe(true);
    f.events.push(`revoked channel close elapsedMs=${active.closedAt === undefined ? 'not-closed' : active.closedAt - revokedAt}`);
    await revoking.getByRole('button', { name: 'Close dialog', exact: true }).click();
    const afterRevoke = active.frames.length;
    await sendPrompt(owner.page, 'AFTER_REVOKE_PRIVATE');
    expect.soft(active.frames.slice(afterRevoke).some(frame => frame.includes('"encrypted"')), 'REVOCATION_EVENTS: revoked open channel must not receive owner events').toBe(false);
    expect(await recipient.page.getByText('AFTER_REVOKE_PRIVATE', { exact: true }).count()).toBe(0);

    // Peer CLI uses only each disposable Supervisor's generated machine config,
    // with --from tying the send to a real local thread (same as live peer E2E).
    const d2 = await f.device(owner, 2);
    await f.pair(owner, d2, true);
    for (const device of [d1, d2]) expect((await f.cli(['--cli-config', device.cli, 'device', 'access', 'on'])).code, 'Enable peer access locally').toBe(0);
    const args = (source: Device, target: Device, subject: string) => ['--cli-config', source.cli, '--from', source.thread,
      'thread', 'send', `${target.id}/${target.thread}`, '--kind', 'status', '--subject', subject, '--text', subject];
    const sent = await f.cli(args(d1, d2, 'CERTIFIED_PEER_DELIVERY'));
    expect.soft(sent.code, 'PEER_CERTIFICATE: same-passkey paired devices must permit a local CLI peer send').toBe(0);
    expect.soft(sent.data?.delivery, 'PEER_DELIVERY: certified message is delivered as passive inbox mail').toBe('inbox');
    const inbox = await f.cli(['--cli-config', d2.cli, 'inbox', '--thread', d2.thread]);
    expect(inbox.code).toBe(0);
    const delivered = inbox.data.messages.find((m: any) => m.subject === 'CERTIFIED_PEER_DELIVERY');
    expect.soft(delivered, 'PEER_ATTRIBUTION: target inbox records device and source thread').toMatchObject({ fromDeviceId: d1.id, fromThreadId: d1.thread, replyTo: `${d1.id}/${d1.thread}` });
    expect((await f.cli(['pair', '--database', d2.db, '--reset', '--yes'])).code).toBe(0);
    const warm = await f.cli(args(d1, d2, 'RESET_TARGET_REFUSAL'));
    expect.soft(warm.code, 'PEER_RESET_CACHED: cached target descriptor must not authorize an unpaired target after reset').not.toBe(0);
    expect.soft(warm.output.includes('peer_target_unverified'), 'PEER_RESET_CACHED: reset target must report peer_target_unverified').toBe(true);
    // Independently verify a fresh handshake, even if cached-certificate reuse
    // is defective. This is an explicit test-only cache reset, not a repair.
    expect((await f.cli(['--cli-config', d1.cli, 'device', 'trust', d2.id, '--reset'])).code).toBe(0);
    const cold = await f.cli(args(d1, d2, 'UNPAIRED_TARGET_REFUSAL'));
    expect.soft(cold.code, 'PEER_RESET_FRESH: paired caller must refuse unpaired target').not.toBe(0);
    expect.soft(cold.output.includes('peer_target_unverified'), 'PEER_RESET_FRESH: fresh handshake must reject missing certificate').toBe(true);
    const unpairedCaller = await f.cli(args(d2, d1, 'UNPAIRED_CALLER_REFUSAL'));
    expect.soft(unpairedCaller.code, 'PEER_CALLER: strict target must refuse unsigned unpaired caller').not.toBe(0);
    expect.soft(unpairedCaller.output.includes('peer_certificate_required'), 'PEER_CALLER: refusal must be peer_certificate_required').toBe(true);
    f.events.push(`peer certified=${sent.code} reset-cached=${warm.code} reset-cached-delivery=${warm.data?.delivery ?? 'none'} reset-fresh=${cold.code} unpaired-caller=${unpairedCaller.code}`);
  } finally { await f.close(); }
});
