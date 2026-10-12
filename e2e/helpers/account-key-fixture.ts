import { expect, type Browser, type BrowserContext, type Page, type TestInfo } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash, createPublicKey, verify, randomBytes, randomUUID } from 'node:crypto';
import { createRequire } from 'node:module';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

// ARIA error prompts include password values and binding secrets.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';

export type Result = { status: number; data: any; encrypted?: boolean };
export type Device = { id: string; db: string; cli: string; api: string; thread: string; url: string };
export type Account = { page: Page; context: BrowserContext; cookie: string; cookies: Awaited<ReturnType<BrowserContext['cookies']>>; code: string; userId: string; recovery: string; accountKeyId: string; publicKey: string; password: string; browserId: string; username?: string };

export async function retainBrowserRequest(f: Fixture, account: Account) {
  // This same-origin page runs no product JS. Retain the non-extractable key in
  // its browser realm before the UI removes its IndexedDB reference on revoke.
  const page = await account.context.newPage();
  await page.goto(f.base + '/healthz');
  const browser = await page.evaluateHandle(async ({ origin, userId }) => {
    const db = await new Promise<IDBDatabase>((done, fail) => {
      const request = indexedDB.open('pockymoe-account-v2');
      request.onsuccess = () => done(request.result); request.onerror = () => fail(request.error);
    });
    try {
      return await new Promise<any>((done, fail) => {
        const request = db.transaction('account').objectStore('account').get(`${origin}\nbrowser:${userId}`);
        request.onsuccess = () => done(request.result); request.onerror = () => fail(request.error);
      });
    } finally { db.close(); }
  }, { origin: f.base, userId: account.userId });
  expect(await browser.evaluate((value, id) => value?.browserId === id && value.privateKey?.type === 'private' && value.privateKey.extractable === false, account.browserId),
    'Retain the actual non-extractable browser key before revocation').toBe(true);
  // Resolve the existing Web workspace dependency without changing package or
  // product code. The request goes through native Node fetch, bypassing both the
  // product's local revocation guard and the browser's Service Worker.
  const { CipherSuite, DhkemP256HkdfSha256, HkdfSha256, Aes256Gcm } = createRequire(resolve('apps/supervisor-web/package.json'))('@hpke/core');
  const suite = new CipherSuite({ kem: new DhkemP256HkdfSha256(), kdf: new HkdfSha256(), aead: new Aes256Gcm() });
  return async (device: Device, path: string, liveRelayCookie: string): Promise<Result> => {
    const descriptor = ok(await f.api(device.api + '/transport/key?challenge=' + randomUUID(), 'GET', undefined, liveRelayCookie), 'Retained-key target descriptor');
    const wirePath = '/api' + path;
    const requestId = `${randomUUID()}.${Date.now()}`;
    const aad = `rcd-http-v1\n${descriptor.keyId}\n${requestId}\nGET\n${wirePath}\n`;
    const sender = await suite.createSenderContext({
      recipientPublicKey: await suite.kem.deserializePublicKey(Buffer.from(descriptor.publicKey, 'base64url')),
      info: Buffer.from('remote-codex/relay/v1'),
    });
    const metadata = Buffer.from(JSON.stringify({ headers: {}, query: '' }));
    const packet = Buffer.alloc(4 + metadata.length); packet.writeUInt32BE(metadata.length); metadata.copy(packet, 4);
    const sealed = Buffer.from(await sender.seal(packet, Buffer.from(aad)));
    const input = `pockymoe-request-v1\n${aad}\n${createHash('sha256').update(sealed).digest('base64url')}`;
    const signature = await browser.evaluate(async (value, input) => {
      const signature = new Uint8Array(await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, value.privateKey, new TextEncoder().encode(input)));
      return btoa(String.fromCharCode(...signature)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    }, input);
    const response = await fetch(f.base + device.api + path, { headers: {
      cookie: liveRelayCookie, 'x-rcd-key': descriptor.keyId, 'x-rcd-request': requestId,
      'x-rcd-enc': Buffer.from(sender.enc).toString('base64url'), 'x-rcd-sealed': sealed.toString('base64url'),
      'x-rcd-session': account.browserId, 'x-rcd-signature': signature,
    }, signal: AbortSignal.timeout(10_000) });
    const ciphertext = await response.arrayBuffer();
    if (response.headers.get('x-rcd-encrypted') !== '1') {
      return { status: response.status, data: JSON.parse(Buffer.from(ciphertext).toString()), encrypted: false };
    }
    const responseKey = await crypto.subtle.importKey('raw', new Uint8Array(await sender.export(Buffer.from('remote-codex/http-response/v1'), 32)).buffer, 'AES-GCM', false, ['decrypt']);
    const clear = Buffer.from(await crypto.subtle.decrypt({ name: 'AES-GCM', iv: new Uint8Array(12), additionalData: new TextEncoder().encode(aad + '\nresponse') }, responseKey, ciphertext));
    const length = clear.readUInt32BE(0), result = JSON.parse(clear.subarray(4, 4 + length).toString());
    const data = JSON.parse(clear.subarray(4 + length).toString());
    f.events.push(`retained browser request status=${result.status} code=${data?.code ?? 'none'} encrypted=true`);
    return { status: result.status, data, encrypted: true };
  };
}

export async function browserRequest(page: Page, path: string, method = 'GET', body?: unknown): Promise<Result> {
  // Use the real Service Worker and the browser's real certificate and signature.
  return page.evaluate(async ({ path, method, body }) => {
    const response = await fetch(path, { method, credentials: 'same-origin',
      headers: { 'content-type': 'application/json' }, signal: AbortSignal.timeout(15_000),
      ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
    const text = await response.text();
    return { status: response.status, data: text ? JSON.parse(text) : null };
  }, { path, method, body });
}
export async function screenshot(page: Page, info: TestInfo, name: string) {
  await mkdir(resolve('.scratch/screenshots'), { recursive: true });
  await page.screenshot({ mask: [page.locator('pre'), page.locator('code').filter({ hasText: /^[0-9A-HJKMNP-TV-Z]{4}(-[0-9A-HJKMNP-TV-Z]{4}){7}$/ })], path: resolve(`.scratch/screenshots/${info.project.name}-${name}.png`) });
}
export function ok(result: Result, label: string) {
  // Auth/device bodies contain tokens, so assertion diagnostics use status only.
  expect(result.status, label).toBeGreaterThanOrEqual(200);
  expect(result.status, label).toBeLessThan(300);
  return result.data;
}
export function sockets(page: Page) {
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
export async function sendPrompt(page: Page, marker: string) {
  const prompt = page.locator('[role="textbox"][aria-label="Prompt"][contenteditable]:not([contenteditable="false"])');
  await expect(prompt).toBeVisible();
  await prompt.fill(`Reply with exactly ${marker}`);
  await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  await expect(page.getByText(marker, { exact: true })).toBeVisible();
}
export async function unlock(page: Page, password: string) {
  const dialog = page.getByRole('dialog', { name: 'Unlock this browser', exact: true });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel('Password', { exact: true }).fill(password);
  const button = dialog.getByRole('button', { name: 'Unlock', exact: true });
  const blocked = !await button.isEnabled() || await button.evaluate(button => {
    const rect = button.getBoundingClientRect();
    const hit = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2);
    return hit !== button && !button.contains(hit);
  });
  // The account unlock portal must accept real typing and clicks even above
  // another open Radix dialog; no synthetic input dispatch is used.
  expect.soft(blocked, 'ACCOUNT_UNLOCK_MODAL: an underlying Radix dialog must not block the account unlock portal').toBe(false);
  await button.click();
  await expect(dialog).toHaveCount(0);
}
export async function approveGrant(page: Page, password: string, unverified = false) {
  const review = page.getByRole('dialog', { name: 'Review device grant', exact: true });
  await expect(review).toBeVisible();
  if (unverified) await review.getByRole('checkbox').check();
  await review.getByRole('button', { name: 'Confirm to sign these permissions.', exact: true }).click();
  if (await page.getByRole('dialog', { name: 'Unlock this browser', exact: true }).isVisible()) await unlock(page, password);
  await expect(review).toHaveCount(0);
}

export async function shareDialog(page: Page, url: string) {
  await page.goto(url);
  await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
  await page.locator('[aria-controls="matter-thread-tools"]').click();
  await page.getByRole('button', { name: 'Sharing permissions', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Share this thread', exact: true })).toBeVisible();
  return dialog;
}

export class Fixture {
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
  async cli(args: string[], commandEnv: Record<string, string> = {}) {
    return new Promise<{ code: number | null; output: string; data: any }>((done, fail) => {
      let output = '';
      const child = spawn(this.binary, args, { env: { ...this.env, HOME: join(this.root, 'home'), ...commandEnv }, stdio: ['ignore', 'pipe', 'pipe'] });
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
    // Chromium's local-network permission also guards ws://localhost on the
    // assigned relay port.
    await context.grantPermissions(['local-network-access'], { origin: this.base });
    this.contexts.push(context);
    // Documents keep the relay's real CSP; Argon2id WebAssembly must run under it.
    await context.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
    if (account) await context.addCookies(account.cookies);
    context.on('response', response => {
      const path = new URL(response.url()).pathname;
      if (path.startsWith('/relay/')) this.events.push(`browser ${response.request().method()} ${path} ${response.status()} encrypted=${response.headers()['x-rcd-encrypted'] === '1'}`);
    });
    const page = await context.newPage();
    page.on('pageerror', error => this.logs.push('browser exception: ' + error.message));
    page.on('console', message => { if (message.type() === 'error') this.logs.push('browser error: ' + message.text()); });
    return { context, page };
  }
  async login(account: Account, returnTo = '/relay-account'): Promise<Account> {
    const { page, context } = await this.context();
    await page.goto(this.base + '/relay-portal?returnTo=' + encodeURIComponent(returnTo));
    await page.getByLabel('Email or username', { exact: true }).fill(account.userId.includes('@') ? account.userId : account.username ?? account.userId);
    await page.getByLabel('Password', { exact: true }).fill(account.password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Security', exact: true })).toBeVisible();
    return this.accountMetadata({ ...account, page, context });
  }
  async account(username: string, capture = false): Promise<Account & { username: string }> {
    const { context, page } = await this.context();
    await page.goto(this.base + '/relay-portal?returnTo=%2Frelay-account');
    await page.getByRole('tab', { name: 'Create account', exact: true }).click();
    await page.getByLabel('Email', { exact: true }).fill(`${username}@example.test`);
    await page.getByLabel('Username', { exact: true }).fill(username);
    await page.getByLabel('Password', { exact: true }).fill(this.password);
    if (capture) await screenshot(page, this.info, 'v2-register');
    await page.getByRole('button', { name: 'Create account', exact: true }).click();
    const confirm = page.getByLabel('Re-type the last group to confirm you saved it', { exact: true });
    await expect(confirm).toBeVisible();
    const recovery = (await page.locator('code').textContent())!.trim(); this.secrets.push(recovery);
    expect(/^[0-9A-HJKMNP-TV-Z]{4}(-[0-9A-HJKMNP-TV-Z]{4}){7}$/.test(recovery), 'Recovery format').toBe(true);
    await expect(page.getByRole('button', { name: 'Continue', exact: true })).toBeDisabled();
    if (capture) await screenshot(page, this.info, 'v2-recovery-confirmation');
    await confirm.fill(recovery.slice(-4));
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Security', exact: true })).toBeVisible();
    return this.accountMetadata({ page, context, recovery, password: this.password, username } as Account & { username: string });
  }
  async accountMetadata<T extends Account>(account: T): Promise<T> {
    const cookies = await account.context.cookies(this.base);
    const session = cookies.find(c => c.name === 'remote_codex_relay_session');
    expect(!!session, 'Real sign-in relay cookie').toBe(true); this.secrets.push(session!.value);
    const cookie = `remote_codex_relay_session=${session!.value}`;
    const userId = ok(await this.api('/relay/auth/session', 'GET', undefined, cookie), 'Account session').user.id;
    const value = ok(await this.api('/relay/account/vault', 'GET', undefined, cookie), 'Encrypted vault');
    const vault = value.vault ?? value;
    const accountKeyId = vault.accountKeyId, publicKey = vault.accountPublicKey;
    const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
    const hash = createHash('sha256').update('pockymoe-acct-safety-v1\n' + accountKeyId).digest();
    let bits = 0, acc = 0, encoded = '';
    for (const byte of hash) { acc = (acc << 8) | byte; bits += 8; while (bits >= 5) { bits -= 5; encoded += alphabet[(acc >>> bits) & 31]; } }
    const code = encoded.slice(0, 12).match(/.{4}/g)!.join(' ');
    await account.context.addInitScript(id => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, `relay:${id}`])}`,
      JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })), userId);
    await expect(account.page.locator('code').filter({ hasText: code })).toHaveText(code);
    const browserId = await account.page.locator('.owner-trust-row').filter({ hasText: 'This browser' }).locator('[title]').getAttribute('title');
    expect(!!browserId, 'Current browser certificate is listed').toBe(true);
    const records = ok(await this.api('/relay/account/browsers', 'GET', undefined, cookie), 'Public browser certificates').browsers;
    const certificate = records.find((row: any) => row.certificate.browserId === browserId)?.certificate;
    expect(!!certificate && certificate.expiresAt === undefined, 'Browser certificate has no expiry').toBe(true);
    const input = `pockymoe-acct-browser-v1\n${certificate.accountKeyId}\n${certificate.browserId}\n${certificate.publicKey}\n${certificate.createdAt}`;
    expect(verify('sha256', Buffer.from(input), { key: createPublicKey({ key: Buffer.from(publicKey, 'base64url'), format: 'der', type: 'spki' }), dsaEncoding: 'ieee-p1363' }, Buffer.from(certificate.signature, 'base64url')), 'Independent browser certificate signature verification').toBe(true);
    return Object.assign(account, { cookies, cookie, userId, code, accountKeyId, publicKey, browserId: browserId! });
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
    await expect.poll(async () => (await this.api(`/relay/devices/${id}/api/version`, 'GET', undefined, owner.cookie)).status).toBe(200);
    await mkdir(workspacePath, { recursive: true });
    const api = `/relay/devices/${id}/api`;
    const workspace = ok(await this.api(api + '/workspaces', 'POST', { label: `Signed workspace ${count}`, absPath: workspacePath }, owner.cookie), 'Seed workspace');
    const result = ok(await this.api(api + '/threads/start', 'POST', { workspaceId: workspace.id, model: 'fake', title: `Signed thread ${count}` }, owner.cookie), 'Seed fake-runtime thread');
    const thread = result.thread?.id ?? result.id;
    return { id, db, cli: db.replace(/\.sqlite$/, '.cli.json'), api, thread, url: `${this.base}/devices/${id}/threads/${thread}` };
  }
  async bind(owner: Account, device: Device, capture = false) {
    const page = owner.page;
    await page.goto(this.base + '/relay-devices');
    const row = page.locator('article').filter({ has: page.getByText(`Signed device ${device.db.match(/device-(\d+)/)![1]}`, { exact: true }) });
    await expect(row.getByText('Not protected · Relay trusted', { exact: true })).toBeVisible();
    if (capture) await screenshot(page, this.info, 'v2-device-unprotected');
    await row.getByRole('button', { name: 'Protect this device', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Protect this device', exact: true });
    // Runtimes that support remote binding offer it first; this fixture exercises
    // the local command, which the panel prepares only after an explicit choice.
    const local = dialog.getByRole('button', { name: 'Show local bind command', exact: true });
    const unlockDialog = page.getByRole('dialog', { name: 'Unlock this browser', exact: true });
    await expect(local.or(unlockDialog)).toBeVisible();
    if (await local.isVisible()) await local.click();
    await unlock(page, owner.password);
    await expect(dialog.locator('pre')).toBeVisible();
    const command = (await dialog.locator('pre').textContent())!.trim();
    // Section 11's Unix command puts the one-time secret in the CLI child's
    // environment. Parse its quoted arguments without running an arbitrary
    // shell or inheriting that secret into either Supervisor process.
    const prefix = /^POCKYMOE_ENROLL_SECRET='([A-Za-z0-9_-]{43})'\s+/.exec(command);
    expect(!!prefix, 'ENROLLMENT_SECRET_ARGV: displayed bind command passes the secret only through POCKYMOE_ENROLL_SECRET').toBe(true);
    this.secrets.push(prefix![1]);
    const executable = command.slice(prefix![0].length);
    expect(/^pockymoe\s+bind\s/.test(executable), 'Devices displays a local bind command').toBe(true);
    const args = (executable.match(/'[^']*'|"[^"]*"|\S+/g) ?? []).slice(1)
      .map(argument => argument.replace(/^(['"])(.*)\1$/, '$2'));
    expect(args.some(argument => argument.startsWith('--enroll')), 'Enrollment secret is never a process argument').toBe(false);
    if (capture) await screenshot(page, this.info, 'v2-device-enrolling');
    expect((await this.cli([...args, '--database', device.db], { POCKYMOE_ENROLL_SECRET: prefix![1] })).code, 'Run real UI bind command on isolated DB').toBe(0);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    // The monitor keeps polling an already-online device until the command runs,
    // then completes enrollment without a reload or a second password entry.
    await page.bringToFront();
    const retry = page.getByRole('button', { name: 'Retry enrollment', exact: true });
    await expect.poll(async () => await row.getByText('Protected', { exact: true }).isVisible() || await retry.isVisible(), { timeout: 40_000 }).toBe(true);
    if (await retry.isVisible()) {
      const diagnosis = await page.getByRole('alert').allTextContents();
      this.events.push('enrollment premature pause: ' + diagnosis.join(' '));
      expect.soft(false, 'ENROLLMENT_AUTOMATIC: monitor paused before the Protect command was executed on an online device. ' + diagnosis.join(' ')).toBe(true);
    }
    await expect(row.getByText('Protected', { exact: true })).toBeVisible({ timeout: 40_000 });
    if (capture) await screenshot(page, this.info, 'v2-device-protected');
    await page.goto(device.url);
    await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
    const trust = ok(await browserRequest(page, device.api + '/transport/trust'), 'Signed trust');
    expect(trust).toMatchObject({ bound: true, strict: true, deviceId: device.id });
    const c = trust.certificate;
    expect(!!c, 'Device certificate installed').toBe(true);
    const input = `pockymoe-acct-device-v1\n${c.accountKeyId}\n${c.deviceId}\n${c.identityKey}\n${c.createdAt}`;
    const valid = verify('sha256', Buffer.from(input), { key: createPublicKey({ key: Buffer.from(owner.publicKey, 'base64url'), format: 'der', type: 'spki' }), dsaEncoding: 'ieee-p1363' }, Buffer.from(c.signature, 'base64url'));
    expect(valid, 'Independent device certificate verification').toBe(true);
    return trust;
  }
  async sql(script: string, params: Record<string, unknown> = {}) {
    return new Promise<any>((done, fail) => {
      let output = '';
      const child = spawn('python3', ['-c', 'import json,sqlite3,sys,hashlib,uuid,datetime\np=json.load(sys.stdin)\nc=sqlite3.connect(p["db"],timeout=5)\n' + script + '\nc.commit()\nc.close()\n'], { stdio: ['pipe', 'pipe', 'ignore'] });
      child.stdin.end(JSON.stringify({ db: join(this.root, 'relay', 'relay-store.sqlite'), ...params }));
      child.stdout.on('data', data => { output += String(data); });
      child.once('error', () => fail(new Error('Disposable relay DB fixture failed')));
      child.once('exit', code => { if (code !== 0) fail(new Error('Disposable relay DB fixture failed')); else done(output ? JSON.parse(output) : null); });
    });
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
    // Playwright's automatic ARIA error context also exposes password input
    // values. Clear secret-bearing DOM before the runner snapshots a close.
    await Promise.allSettled(this.contexts.map(context => context.unrouteAll({ behavior: 'ignoreErrors' })));
    for (const context of this.contexts) for (const page of context.pages()) await page.evaluate(() => {
      document.querySelectorAll('input').forEach(input => { input.value = ''; });
      document.querySelectorAll('pre, code').forEach(node => { node.textContent = '[REDACTED]'; });
    }).catch(() => {});
    await Promise.allSettled(this.contexts.map(context => context.close()));
    await Promise.all(this.processes.map(child => new Promise<void>(done => {
      if (child.exitCode !== null || child.signalCode !== null) return done();
      const timer = setTimeout(() => child.kill('SIGKILL'), 2_000);
      child.once('exit', () => { clearTimeout(timer); done(); }); child.kill('SIGTERM');
    })));
    const evidence = this.secrets.reduce((text, secret) => text.split(secret).join('[REDACTED]'), [...this.events, ...this.logs].join('\n'))
      .replace(/http[^\s]*\/pair#[^\s]*/g, '[REDACTED PAIR URL]').replace(/(secret=)[^&\s]+/g, '$1[REDACTED]');
    await writeFile(resolve(`.scratch/${this.info.title.includes("sharing") ? "signed-sharing" : "account-trust"}-${this.info.project.name}.log`), evidence);
    if (this.info.errors.length) await this.info.attach('sanitized-protocol-log', { body: evidence, contentType: 'text/plain' });
    await rm(this.root, { recursive: true, force: true });
  }
}

export async function fixture(browser: Browser, info: TestInfo, options: { previews?: boolean } = {}) {
  await mkdir(resolve('.scratch'), { recursive: true });
  const root = await mkdtemp(resolve('.scratch/account-key-e2e-'));
  await mkdir(join(root, 'home'));
  const f = new Fixture(root, browser, info);
  f.start('relay', { PORT: new URL(f.base).port, POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
    POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_PUBLIC_BASE_URL: f.base,
    ...(options.previews ? { POCKYMOE_PORT_PREVIEW_BASE_URL: `http://preview.localhost:${new URL(f.base).port}` } : {}),
    POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist') });
  await expect.poll(() => fetch(f.base + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
  const response = await fetch(f.base);
  const csp = response.headers.get('content-security-policy') ?? '';
  expect.soft(csp.includes("'wasm-unsafe-eval'"),
    'ACCOUNT_KDF_CSP: the real relay CSP must permit Argon2id WebAssembly for registration and login').toBe(true);
  expect(csp, 'Relay CSP relaxes only WebAssembly compilation').not.toMatch(/script-src[^;]*'unsafe-(eval|inline)'/);
  return f;
}
