import { test, expect } from '@playwright/test';
import WebSocket from 'ws';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';

// An isolated relay serving the built Web UI, seeded with fictional accounts and
// one fake device tunnel. The admin console reads it through its real API.
test.use({ trace: 'off', video: 'off', screenshot: 'off' });

// The relay binary; a build in another target directory can be named here.
const RELAY_BIN = process.env.E2E_RELAY_BIN ?? resolve('target/debug/pockymoe');

async function freePort() {
  return new Promise<number>((done) => {
    const server = createServer();
    server.listen(0, '127.0.0.1', () => {
      const value = (server.address() as { port: number }).port;
      server.close(() => done(value));
    });
  });
}

async function stopRelay(relay: ChildProcess | undefined) {
  if (!relay || relay.exitCode !== null) return;
  await new Promise<void>((done) => {
    const timer = setTimeout(() => relay.kill('SIGKILL'), 2_000);
    relay.once('exit', () => {
      clearTimeout(timer);
      done();
    });
    relay.kill('SIGTERM');
  });
}

test('relay admin console: overview, paged users with details, a tunnel disconnect and the admin action log', async ({ browser }, info) => {
  test.skip(info.project.name !== 'desktop-chromium', 'The console flow runs on desktop');
  test.setTimeout(120_000);
  const dataRoot = resolve(process.env.E2E_WORKSPACE_ROOT ?? '.scratch');
  await mkdir(dataRoot, { recursive: true });
  const root = await mkdtemp(join(dataRoot, 'relay-admin-e2e-'));
  const port = await freePort();
  const base = `http://localhost:${port}`;
  const password = randomBytes(24).toString('hex');
  const deviceToken = `rcd_${randomBytes(20).toString('hex')}`;
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  let relay: ChildProcess | undefined;
  let tunnel: WebSocket | undefined;
  const logs: string[] = [];
  try {
    await mkdir(join(root, 'home'));
    relay = spawn(RELAY_BIN, ['relay'], {
      env: {
        ...env,
        HOME: join(root, 'home'),
        HOST: '127.0.0.1',
        PORT: String(port),
        POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
        POCKYMOE_PUBLIC_BASE_URL: base,
        POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'),
        POCKYMOE_ADMIN_USERNAME: 'console-admin',
        POCKYMOE_ADMIN_PASSWORD: password,
        POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    relay.stdout?.on('data', (data) => logs.push(String(data)));
    relay.stderr?.on('data', (data) => logs.push(String(data)));
    await expect.poll(() => fetch(`${base}/healthz`).then((r) => r.status).catch(() => 0)).toBe(200);

    // 30 accounts (more than one page) and one device, written to the
    // disposable relay database only.
    const tokenHash = createHash('sha256').update(deviceToken).digest('base64url');
    const seed = `
import sqlite3, sys, json, uuid, datetime as dt
p = json.load(sys.stdin)
c = sqlite3.connect(p['db'], timeout=5)
now = dt.datetime.now(dt.timezone.utc)
iso = lambda d: d.strftime('%Y-%m-%dT%H:%M:%S.000Z')
owner = None
for n in range(1, 31):
    i = str(uuid.uuid4()); owner = owner or i
    c.execute('INSERT INTO relay_users(id,email,username,role,enabled,last_seen_at,created_at,password_salt,password_hash) VALUES (?,?,?,?,?,?,?,?,?)',
      (i, 'person-%02d@example.test' % n, 'person-%02d' % n, 'user', 1, iso(now - dt.timedelta(hours=n)), iso(now - dt.timedelta(days=40 - n)), '', ''))
c.execute('INSERT INTO relay_devices(id,owner_user_id,name,token,token_hash,token_preview,created_at) VALUES (?,?,?,?,?,?,?)',
  (str(uuid.uuid4()), owner, 'console-test-device', None, p['hash'], 'rcd_...test', iso(now - dt.timedelta(days=3))))
c.commit()
`;
    await new Promise<void>((done, fail) => {
      const child = spawn('python3', ['-c', seed], { stdio: ['pipe', 'ignore', 'pipe'] });
      child.stderr.on('data', (data) => logs.push(String(data)));
      child.stdin.end(JSON.stringify({ db: join(root, 'relay', 'relay-store.sqlite'), hash: tokenHash }));
      child.once('exit', (code) => (code === 0 ? done() : fail(new Error('Seeding the disposable relay failed'))));
    });

    // A fake device tunnel: heartbeats, and a version on /healthz.
    tunnel = new WebSocket(`${base.replace('http', 'ws')}/supervisor/tunnel?deviceToken=${deviceToken}`);
    const closed = new Promise<void>((done) => tunnel!.once('close', () => done()));
    tunnel.on('message', (data) => {
      const message = JSON.parse(String(data));
      if (message.type !== 'relay.request') return;
      const body = message.payload?.path === '/healthz' ? { status: 'ok', runningVersion: '0.1.0' } : { ok: true };
      tunnel!.send(JSON.stringify({ type: 'relay.response', requestId: message.requestId, payload: { statusCode: 200, headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) } }));
    });
    await new Promise<void>((done, fail) => {
      tunnel!.once('open', () => done());
      tunnel!.once('error', fail);
    });
    tunnel.send(JSON.stringify({ type: 'relay.heartbeat', timestamp: new Date().toISOString(), portMappings: [] }));
    await expect.poll(async () => (await (await fetch(`${base}/healthz`)).json()).connectedSupervisors).toBe(1);

    const context = await browser.newContext(info.project.use);
    // This browser chose the Terminal theme pack for the rest of the app.
    await context.addInitScript(() => {
      localStorage.setItem('remote-codex.locale', 'en');
      if (!localStorage.getItem('pockymoe-theme-preset')) localStorage.setItem('pockymoe-theme-preset', 'terminal');
    });
    const page = await context.newPage();
    page.on('pageerror', (error) => logs.push(`browser exception: ${error.message}`));
    const preset = () => page.evaluate(() => document.documentElement.dataset.themePreset);

    // The admin sign-in shows Classic whatever the chosen pack; the way to
    // the user sign-in brings the pack back, and coming back pins Classic again.
    await page.goto(`${base}/relay-admin`);
    await expect(page.getByRole('heading', { name: 'Relay admin sign-in' })).toBeVisible();
    await expect.poll(preset).toBe('classic');
    await page.getByRole('link', { name: 'User sign-in', exact: true }).click();
    await expect(page).toHaveURL(/\/relay-portal/);
    await expect.poll(preset).toBe('terminal');
    await expect(page.getByRole('link', { name: 'Admin console', exact: true })).toBeVisible();
    await page.getByRole('link', { name: 'Admin console', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Relay admin sign-in' })).toBeVisible();
    await expect.poll(preset).toBe('classic');
    expect(await page.evaluate(() => localStorage.getItem('pockymoe-theme-preset'))).toBe('terminal');

    // Sign in and land on the overview.
    await page.getByLabel('Username', { exact: true }).fill('console-admin');
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    const nav = page.getByRole('navigation', { name: 'Admin sections' });
    await expect(nav.getByRole('button', { name: /^Overview/ })).toHaveAttribute('aria-current', 'page');
    await expect(page.getByText('accounts without an account key', { exact: true })).toBeVisible();

    // One time range for the overview, kept in the URL: every chart over
    // time follows it; cards of the present say "Now".
    await page.getByRole('group', { name: 'Time range' }).getByRole('button', { name: '30 days', exact: true }).click();
    await expect(page).toHaveURL(/range=30d/);
    await expect(page.getByRole('region', { name: 'New accounts' }).getByText(/^Per day · \d+ in 30 days$/)).toBeVisible();
    await expect(page.getByRole('region', { name: 'Device versions' }).getByText('Now', { exact: true })).toBeVisible();
    await page.getByRole('group', { name: 'Time range' }).getByRole('button', { name: 'Custom', exact: true }).click();
    await page.getByRole('dialog', { name: 'Custom range' }).getByRole('button', { name: 'Apply', exact: true }).click();
    await expect(page).toHaveURL(/range=custom&from=\d{4}-\d{2}-\d{2}&to=\d{4}-\d{2}-\d{2}/);
    await expect(page.getByRole('heading', { name: 'New accounts' })).toBeVisible();
    await page.getByRole('group', { name: 'Time range' }).getByRole('button', { name: '24 hours', exact: true }).click();

    // Users (30 people and the admin): the first page holds 25, the second the
    // rest; a row opens its details in the drawer.
    await nav.getByRole('button', { name: /^Users/ }).click();
    const users = page.getByRole('region', { name: 'Users' }).getByRole('table');
    await expect(page.getByText('1–25 of 31', { exact: true })).toBeVisible();
    await expect(users.getByRole('row')).toHaveCount(26);
    await page.getByRole('button', { name: 'Next page', exact: true }).click();
    await expect(page.getByText('26–31 of 31', { exact: true })).toBeVisible();
    await expect(page).toHaveURL(/tab=users/);
    // Sorting by a header asks the relay and is kept in the URL.
    await users.getByRole('columnheader', { name: /Registered/ }).getByRole('button').click();
    await expect(page).toHaveURL(/sort=created&dir=desc/);
    await expect(users.getByRole('columnheader', { name: /Registered/ })).toHaveAttribute('aria-sort', 'descending');
    await expect(page.getByText('1–25 of 31', { exact: true })).toBeVisible();
    await expect(users.getByRole('row').nth(1)).toContainText('console-admin');
    // A column filter: people without an account key (the admin has none to set).
    await page.getByRole('group', { name: 'Filter users' }).getByRole('button', { name: /^Account key/ }).click();
    await page.getByRole('menu', { name: 'Account key' }).getByRole('menuitemradio', { name: /Not set/ }).click();
    await expect(page).toHaveURL(/key=missing/);
    await expect(page.getByText('30 matches', { exact: true })).toBeVisible();
    // The column chooser shows Email, and the table remembers it after a reload.
    await page.getByRole('button', { name: 'Columns', exact: true }).click();
    await page.getByRole('menu', { name: 'Columns of Users' }).getByRole('menuitemcheckbox', { name: 'Email' }).click();
    await page.keyboard.press('Escape');
    await expect(users.getByRole('columnheader', { name: /Email/ })).toBeVisible();
    await page.reload();
    await expect(page.getByRole('region', { name: 'Users' }).getByRole('table').getByRole('columnheader', { name: /Email/ })).toBeVisible();
    await expect(page.getByText('30 matches', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
    await expect(page).not.toHaveURL(/key=missing/);
    // Still newest first (the sort stays): person-28 is on the first page.
    await expect(page.getByText('1–25 of 31', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Open person-28', exact: true }).first().click();
    const person = page.getByRole('dialog', { name: 'person-28' });
    await expect(person.getByText('person-28@example.test')).toBeVisible();
    await expect(page).toHaveURL(/[?&]id=/);
    await page.keyboard.press('Escape');
    await expect(person).toHaveCount(0);

    // Devices: a connection filter, then the online device's drawer
    // disconnects its tunnel after a confirmation; the relay closes the socket.
    await nav.getByRole('button', { name: /^Devices/ }).click();
    await page.getByRole('group', { name: 'Filter devices' }).getByRole('button', { name: /^Connection/ }).click();
    await page.getByRole('menu', { name: 'Connection' }).getByRole('menuitemradio', { name: /Offline/ }).click();
    await expect(page).toHaveURL(/state=offline/);
    await expect(page.getByText('0 matches', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
    // The device row names its owner and links to both (names such as "mac"
    // repeat across people); the owner's email is the link's title.
    const owner = page.getByRole('region', { name: 'Devices' }).getByRole('button', { name: 'Open owner person-01 (person-01@example.test)', exact: true });
    await expect(owner).toHaveAttribute('title', 'person-01@example.test');
    await owner.click();
    await expect(page.getByRole('dialog', { name: 'person-01' })).toBeVisible();
    await page.keyboard.press('Escape');
    await nav.getByRole('button', { name: /^Devices/ }).click();
    await page.getByRole('button', { name: 'Open console-test-device of person-01', exact: true }).first().click();
    const device = page.getByRole('dialog', { name: 'console-test-device' });
    await expect(device.getByText('person-01 (person-01@example.test)', { exact: false })).toBeVisible();
    await device.getByRole('button', { name: 'Disconnect tunnel', exact: true }).click();
    const confirm = page.getByRole('dialog', { name: 'Disconnect console-test-device?' });
    await confirm.getByRole('button', { name: 'Disconnect tunnel', exact: true }).click();
    await closed;
    await expect(device.getByRole('button', { name: 'Disconnect tunnel', exact: true })).toBeDisabled();
    await page.keyboard.press('Escape');

    // The sidebar folds into an icon rail, keeps the names as accessible
    // names, and stays folded after a reload.
    await page.getByRole('button', { name: 'Collapse sidebar', exact: true }).click();
    await expect(page.locator('.admin-console')).toHaveClass(/is-collapsed/);
    await expect(nav.getByRole('button', { name: /^Performance/ })).toBeVisible();
    await page.reload();
    await expect(page.getByRole('button', { name: 'Expand sidebar', exact: true })).toHaveAttribute('aria-expanded', 'false');

    // Performance in depth, also over a custom window.
    await nav.getByRole('button', { name: /^Performance/ }).click();
    await expect(page.getByRole('heading', { name: 'Errors by kind' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Latency percentiles' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Traffic per device' })).toBeVisible();
    await page.getByRole('group', { name: 'Time range' }).getByRole('button', { name: 'Custom', exact: true }).click();
    const custom = page.getByRole('dialog', { name: 'Custom range' });
    await custom.getByRole('button', { name: 'Apply', exact: true }).click();
    await expect(page).toHaveURL(/range=custom&from=\d{4}-\d{2}-\d{2}&to=\d{4}-\d{2}-\d{2}/);
    await expect(page.getByRole('heading', { name: 'Request volume' })).toBeVisible();
    await page.getByRole('button', { name: 'Expand sidebar', exact: true }).click();

    // Server: the relay's host, each figure with a status in words, the
    // thresholds behind them and history charts over the page's range.
    await nav.getByRole('button', { name: /^Server/ }).click();
    const now = page.getByRole('region', { name: 'The server now' });
    await expect(now).toBeVisible();
    for (const tile of ['CPU', 'Memory', 'Disk', 'Database', 'Store lock', 'Runtime delay']) await expect(now.getByText(tile, { exact: true })).toBeVisible();
    await expect(now.getByText(/^(Normal|High|Critical)$/).first()).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Disk and database' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Relay load' })).toBeVisible();
    await page.getByText(/What turns amber or red/).click();
    await expect(page.getByText('high at 85% · critical at 95%').first()).toBeVisible();
    expect(await page.locator('main').textContent()).not.toContain('person-');

    // The admin action log names the action, its target and the admin.
    await nav.getByRole('button', { name: /^Sessions and security/ }).click();
    await page.getByRole('tab', { name: 'Admin actions', exact: true }).click();
    const log = page.getByRole('tabpanel', { name: 'Admin actions' }).getByRole('table');
    const entry = log.getByRole('row').filter({ hasText: 'Disconnected a tunnel' });
    await expect(entry).toHaveCount(1);
    await expect(entry).toContainText('console-test-device');
    await expect(entry).toContainText('person-01');
    await expect(entry).toContainText('console-admin');
    expect(logs.filter((line) => line.startsWith('browser exception')), 'No uncaught browser errors').toEqual([]);
    await context.close();
  } finally {
    tunnel?.terminate();
    await stopRelay(relay);
    if (info.errors.length) await info.attach('relay-log', { body: logs.join('').split(password).join('[REDACTED]'), contentType: 'text/plain' });
    await rm(root, { recursive: true, force: true });
  }
});

test('Google sign-up asks for the relay invite code and keeps it out of the provider round trip', async ({ browser }, info) => {
  test.skip(info.project.name !== 'desktop-chromium', 'The sign-up flow runs on desktop');
  test.setTimeout(90_000);
  const dataRoot = resolve(process.env.E2E_WORKSPACE_ROOT ?? '.scratch');
  await mkdir(dataRoot, { recursive: true });
  const root = await mkdtemp(join(dataRoot, 'relay-invite-e2e-'));
  const port = await freePort();
  const base = `http://localhost:${port}`;
  const password = randomBytes(24).toString('hex');
  const code = `invite-${randomBytes(6).toString('hex')}`;
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  let relay: ChildProcess | undefined;
  const logs: string[] = [];
  try {
    await mkdir(join(root, 'home'));
    relay = spawn(RELAY_BIN, ['relay'], {
      env: {
        ...env,
        HOME: join(root, 'home'),
        HOST: '127.0.0.1',
        PORT: String(port),
        POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
        POCKYMOE_PUBLIC_BASE_URL: base,
        POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'),
        POCKYMOE_ADMIN_USERNAME: 'console-admin',
        POCKYMOE_ADMIN_PASSWORD: password,
        POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
        // A fictional provider: the browser never leaves for it (routed below).
        POCKYMOE_GOOGLE_OAUTH_CLIENT_ID: 'e2e-client',
        POCKYMOE_GOOGLE_OAUTH_CLIENT_SECRET: 'e2e-secret',
        POCKYMOE_GOOGLE_OAUTH_AUTHORIZE_URL: 'https://accounts.e2e.invalid/authorize',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    relay.stdout?.on('data', (data) => logs.push(String(data)));
    relay.stderr?.on('data', (data) => logs.push(String(data)));
    await expect.poll(() => fetch(`${base}/healthz`).then((r) => r.status).catch(() => 0)).toBe(200);

    const context = await browser.newContext(info.project.use);
    await context.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
    const page = await context.newPage();
    page.on('pageerror', (error) => logs.push(`browser exception: ${error.message}`));
    const provider: string[] = [];
    await page.route('https://accounts.e2e.invalid/**', (route) => {
      provider.push(route.request().url());
      return route.fulfill({ contentType: 'text/html', body: '<p>provider</p>' });
    });

    // The admin turns on Google sign-in and sets an invite code.
    await page.goto(`${base}/relay-admin?tab=settings`);
    await page.getByLabel('Username', { exact: true }).fill('console-admin');
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByText('For password and Google or GitHub sign-up alike. Changes apply at once.')).toBeVisible();
    // Google is on: its credentials are configured.
    await expect(page.getByRole('switch', { name: 'Google', exact: true })).toHaveAttribute('aria-checked', 'true');
    await page.getByLabel('New invite code', { exact: true }).fill(code);
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Required to create an account, with a password or with Google or GitHub. Existing accounts sign in without it.')).toBeVisible();
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();

    // A newcomer: the code comes first, and Google needs it too.
    await page.goto(`${base}/relay-portal`);
    await page.getByRole('button', { name: 'Create account', exact: true }).click();
    await page.getByRole('button', { name: 'Continue with Google', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Enter the invite code first');
    await page.getByLabel('Registration code', { exact: true }).fill('not-the-code');
    await page.getByRole('button', { name: 'Continue with Google', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('The invite code is not valid.');
    expect(provider).toEqual([]);
    await page.getByLabel('Registration code', { exact: true }).fill(code);
    await page.getByRole('button', { name: 'Continue with Google', exact: true }).click();
    await expect.poll(() => provider.length).toBe(1);
    const target = new URL(provider[0]!);
    expect(target.searchParams.get('client_id')).toBe('e2e-client');
    expect(target.searchParams.get('state')).toBeTruthy();
    expect(provider[0]).not.toContain(code);
    expect(logs.join('')).not.toContain(code);
    expect(logs.filter((line) => line.startsWith('browser exception')), 'No uncaught browser errors').toEqual([]);
    await context.close();
  } finally {
    await stopRelay(relay);
    if (info.errors.length) await info.attach('relay-log', { body: logs.join('').split(password).join('[REDACTED]'), contentType: 'text/plain' });
    await rm(root, { recursive: true, force: true });
  }
});
