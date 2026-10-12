import { expect, type Browser, type BrowserContext, type Page, type TestInfo } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash, createPublicKey, verify, randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

// ARIA error prompts include password values and binding secrets.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';

export type Result = { status: number; data: any; encrypted?: boolean };
export type Device = { id: string; db: string; cli: string; api: string; thread: string; url: string };
export type Account = { page: Page; context: BrowserContext; cookie: string; cookies: Awaited<ReturnType<BrowserContext['cookies']>>; code: string; userId: string; recovery: string; accountKeyId: string; publicKey: string; password: string; browserId: string; username?: string };

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
  if (blocked) {
    // A custom account portal currently sits outside the underlying Radix
    // focus scope. Record the defect, then dispatch a normal input event so
    // protocol coverage can continue without patching application code.
    expect.soft(false, 'ACCOUNT_UNLOCK_MODAL: underlying Radix dialog traps focus and prevents the separate account unlock portal from accepting a password. Test-only native input and click dispatch follow.').toBe(true);
    await dialog.getByLabel('Password', { exact: true }).evaluate((input, value) => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }, password);
  }
  if (blocked) await button.evaluate(button => (button as HTMLButtonElement).click());
  else await button.click();
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
    // assigned relay port, including documents fulfilled through route.fetch.
    await context.grantPermissions(['local-network-access'], { origin: this.base });
    this.contexts.push(context);
    // Work around only the diagnosed Argon2 CSP blocker; all application and
    // protocol responses still come from the real relay and Supervisors.
    await context.route('**/*', async route => {
      if (!route.request().isNavigationRequest()) return route.continue();
      try {
        const response = await route.fetch();
        const headers = response.headers(), csp = headers['content-security-policy'];
        if (csp && !csp.includes("'wasm-unsafe-eval'")) headers['content-security-policy'] = csp.replace('script-src ', "script-src 'wasm-unsafe-eval' ");
        await route.fulfill({ response, headers });
      } catch {
        // Raw route.fetch errors contain cookie headers. Preserve a safe error.
        if (context.pages().some(page => !page.isClosed())) throw new Error('Test document interception failed');
      }
    });
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
    await unlock(page, owner.password);
    const dialog = page.getByRole('dialog', { name: 'Protect this device', exact: true });
    await expect(dialog.locator('pre')).toBeVisible();
    const command = (await dialog.locator('pre').textContent())!.trim();
    // Section 11's Unix command puts the one-time secret in the CLI child's
    // environment. Parse its quoted arguments without running an arbitrary
    // shell or inheriting that secret into either Supervisor process.
    const prefix = /^POCKYMOE_ENROLL_SECRET='([A-Za-z0-9_-]{43})'\s+/.exec(command);
    const executable = prefix ? command.slice(prefix[0].length) : command;
    expect(/^pockymoe\s+bind\s/.test(executable), 'Devices displays a local bind command').toBe(true);
    const args = (executable.match(/'[^']*'|"[^"]*"|\S+/g) ?? []).slice(1)
      .map(argument => argument.replace(/^(['"])(.*)\1$/, '$2'));
    const commandEnv: Record<string, string> = prefix ? { POCKYMOE_ENROLL_SECRET: prefix[1] } : {};
    if (prefix) this.secrets.push(prefix[1]);
    else {
      this.secrets.push(args[args.indexOf('--enroll') + 1]);
      expect.soft(false, 'ENROLLMENT_SECRET_ARGV: displayed bind command must use POCKYMOE_ENROLL_SECRET instead of --enroll. Section 11 CLI/UI integration fix pending; execute the baseline command only to cover enrollment.').toBe(true);
    }
    if (capture) await screenshot(page, this.info, 'v2-device-enrolling');
    expect((await this.cli([...args, '--database', device.db], commandEnv)).code, 'Run real UI bind command on isolated DB').toBe(0);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    // Monitor runs before an already-online device has executed the command.
    // Record a premature pause and use the real Retry enrollment action.
    await page.bringToFront();
    const retry = page.getByRole('button', { name: 'Retry enrollment', exact: true });
    await expect.poll(async () => await row.getByText('Protected', { exact: true }).isVisible() || await retry.isVisible(), { timeout: 40_000 }).toBe(true);
    if (await retry.isVisible()) {
      const diagnosis = await page.getByRole('alert').allTextContents();
      this.events.push('enrollment premature pause: ' + diagnosis.join(' '));
      expect.soft(false, 'ENROLLMENT_AUTOMATIC: monitor pauses before the Protect command is executed on an online device; Enrollment requires a reload and password re-entry. ' + diagnosis.join(' ')).toBe(true);
      // Reload resets the stale pre-bind descriptor and the paused monitor.
      await page.reload();
      await unlock(page, owner.password);
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

export async function fixture(browser: Browser, info: TestInfo) {
  await mkdir(resolve('.scratch'), { recursive: true });
  const root = await mkdtemp(resolve('.scratch/account-key-e2e-'));
  await mkdir(join(root, 'home'));
  const f = new Fixture(root, browser, info);
  f.start('relay', { PORT: new URL(f.base).port, POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
    POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_PUBLIC_BASE_URL: f.base,
    POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist') });
  await expect.poll(() => fetch(f.base + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
  const response = await fetch(f.base);
  const csp = response.headers.get('content-security-policy') ?? '';
  expect.soft(csp.includes("'wasm-unsafe-eval'") || csp.includes("'unsafe-eval'"),
    'ACCOUNT_KDF_CSP: real relay CSP blocks Argon2 WebAssembly and registration. Fixture adds only wasm-unsafe-eval to document headers so independent trust scenarios can run.').toBe(true);
  return f;
}
