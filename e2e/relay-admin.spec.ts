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

test('relay admin console: overview, paged users with details, a tunnel disconnect and the admin action log', async ({ browser }, info) => {
  test.skip(info.project.name !== 'desktop-chromium', 'The console flow runs on desktop');
  test.setTimeout(120_000);
  const dataRoot = resolve(process.env.E2E_WORKSPACE_ROOT ?? '.scratch');
  await mkdir(dataRoot, { recursive: true });
  const root = await mkdtemp(join(dataRoot, 'relay-admin-e2e-'));
  const port = await new Promise<number>((done) => {
    const server = createServer();
    server.listen(0, '127.0.0.1', () => {
      const value = (server.address() as { port: number }).port;
      server.close(() => done(value));
    });
  });
  const base = `http://localhost:${port}`;
  const password = randomBytes(24).toString('hex');
  const deviceToken = `rcd_${randomBytes(20).toString('hex')}`;
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  let relay: ChildProcess | undefined;
  let tunnel: WebSocket | undefined;
  const logs: string[] = [];
  try {
    await mkdir(join(root, 'home'));
    relay = spawn(resolve('target/debug/pockymoe'), ['relay'], {
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
    await context.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
    const page = await context.newPage();
    page.on('pageerror', (error) => logs.push(`browser exception: ${error.message}`));

    // Sign in and land on the overview.
    await page.goto(`${base}/relay-admin`);
    await page.getByLabel('Username', { exact: true }).fill('console-admin');
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    const nav = page.getByRole('navigation', { name: 'Admin sections' });
    await expect(nav.getByRole('button', { name: /^Overview/ })).toHaveAttribute('aria-current', 'page');
    await expect(page.getByText('accounts without an account key', { exact: true })).toBeVisible();

    // Users (30 people and the admin): the first page holds 25, the second the
    // rest; a row opens its details in the drawer.
    await nav.getByRole('button', { name: /^Users/ }).click();
    const users = page.getByRole('region', { name: 'Users' }).getByRole('table');
    await expect(page.getByText('1–25 of 31', { exact: true })).toBeVisible();
    await expect(users.getByRole('row')).toHaveCount(26);
    await page.getByRole('button', { name: 'Next page', exact: true }).click();
    await expect(page.getByText('26–31 of 31', { exact: true })).toBeVisible();
    await expect(page).toHaveURL(/tab=users/);
    await page.getByRole('button', { name: 'Open person-28', exact: true }).first().click();
    const person = page.getByRole('dialog', { name: 'person-28' });
    await expect(person.getByText('person-28@example.test')).toBeVisible();
    await expect(page).toHaveURL(/[?&]id=/);
    await page.keyboard.press('Escape');
    await expect(person).toHaveCount(0);

    // Devices: the online device's drawer disconnects its tunnel after a
    // confirmation; the relay closes the socket.
    await nav.getByRole('button', { name: /^Devices/ }).click();
    await page.getByRole('button', { name: 'Open console-test-device', exact: true }).first().click();
    const device = page.getByRole('dialog', { name: 'console-test-device' });
    await device.getByRole('button', { name: 'Disconnect tunnel', exact: true }).click();
    const confirm = page.getByRole('dialog', { name: 'Disconnect console-test-device?' });
    await confirm.getByRole('button', { name: 'Disconnect tunnel', exact: true }).click();
    await closed;
    await expect(device.getByRole('button', { name: 'Disconnect tunnel', exact: true })).toBeDisabled();
    await page.keyboard.press('Escape');

    // The admin action log names the action, its target and the admin.
    await nav.getByRole('button', { name: /^Sessions and security/ }).click();
    await page.getByRole('tab', { name: 'Admin actions', exact: true }).click();
    const log = page.getByRole('tabpanel', { name: 'Admin actions' }).getByRole('table');
    const entry = log.getByRole('row').filter({ hasText: 'Disconnected a tunnel' });
    await expect(entry).toHaveCount(1);
    await expect(entry).toContainText('console-test-device');
    await expect(entry).toContainText('console-admin');
    expect(logs.filter((line) => line.startsWith('browser exception')), 'No uncaught browser errors').toEqual([]);
    await context.close();
  } finally {
    tunnel?.terminate();
    if (relay && relay.exitCode === null) {
      await new Promise<void>((done) => {
        const timer = setTimeout(() => relay!.kill('SIGKILL'), 2_000);
        relay!.once('exit', () => {
          clearTimeout(timer);
          done();
        });
        relay!.kill('SIGTERM');
      });
    }
    if (info.errors.length) await info.attach('relay-log', { body: logs.join('').split(password).join('[REDACTED]'), contentType: 'text/plain' });
    await rm(root, { recursive: true, force: true });
  }
});
