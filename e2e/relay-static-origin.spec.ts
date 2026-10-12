import { expect, test } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

// Real relay + real Web fetches, with temporary storage and synthetic credentials.
// No runtime, external account or live Supervisor is used.
test('separate static origin logs in with a relay cookie and loads devices', async ({ page, context }) => {
  const relayPort = Number(process.env.E2E_RELAY_PORT);
  const webPort = Number(process.env.E2E_WEB_PORT);
  expect(Number.isInteger(relayPort) && relayPort > 0, 'Set assigned E2E_RELAY_PORT').toBe(true);
  expect(Number.isInteger(webPort) && webPort > 0 && webPort !== relayPort, 'Set assigned E2E_WEB_PORT').toBe(true);
  const relay = `http://localhost:${relayPort}`;
  const web = `http://localhost:${webPort}`;
  const root = await mkdtemp(join(tmpdir(), 'pockymoe-static-origin-'));
  const password = randomBytes(24).toString('hex');
  const processes: ChildProcess[] = [];
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) =>
    !/^(POCKYMOE|REMOTE_CODEX|VITE)_/.test(name)));
  function start(command: string, args: string[], extra: Record<string, string>) {
    const child = spawn(command, args, { cwd: resolve('.'), env: { ...env, ...extra }, stdio: 'ignore', detached: process.platform !== 'win32' });
    processes.push(child);
    return child;
  }
  async function waitFor(url: string, child: ChildProcess) {
    await expect.poll(async () => {
      if (child.exitCode !== null) throw new Error(`Fixture process exited with ${child.exitCode}`);
      return fetch(url, { signal: AbortSignal.timeout(1500) }).then((r) => r.status).catch(() => 0);
    }).toBe(200);
  }
  try {
    const relayProcess = start(resolve('target/debug/pockymoe'), ['relay'], {
      POCKYMOE_ADMIN_USERNAME: 'smoke-admin',
      POCKYMOE_ADMIN_PASSWORD: randomBytes(24).toString('hex'),
      POCKYMOE_RELAY_HOST: '127.0.0.1',
      POCKYMOE_RELAY_PORT: String(relayPort),
      POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true',
      POCKYMOE_PUBLIC_BASE_URL: relay,
      POCKYMOE_WEB_ORIGIN: web,
    });
    await waitFor(`${relay}/healthz`, relayProcess);
    const vite = start('corepack', ['pnpm', '--filter', '@pockymoe/supervisor-web', 'exec', 'vite', '--host', 'localhost', '--port', String(webPort), '--strictPort'], {
      VITE_POCKYMOE_RELAY_ORIGIN: relay,
    });
    await waitFor(web, vite);
    const registration = await fetch(`${relay}/relay/auth/register`, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ username: 'static-smoke-user', email: 'smoke@example.test', password }),
    });
    expect(registration.status).toBe(200);
    // The Node fixture intentionally does not give its session to the browser.
    await registration.body?.cancel();
    const redirect = await fetch(`${relay}/devices`, { redirect: 'manual' });
    expect(redirect.status).toBe(307);
    expect(redirect.headers.get('location')).toBe(`${web}/devices`);
    expect(redirect.headers.has('service-worker-allowed')).toBe(false);
    const apiRequests: string[] = [];
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (url.pathname.startsWith('/relay/')) apiRequests.push(url.origin);
    });
    await page.goto(`${web}/relay-portal?returnTo=%2Frelay-devices`);
    await page.getByLabel('Email or username', { exact: true }).fill('static-smoke-user');
    await page.getByLabel('Password', { exact: true }).fill(password);
    const login = page.waitForResponse((response) => response.url() === `${relay}/relay/auth/login` && response.request().method() === 'POST');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    const loginResponse = await login;
    expect(loginResponse.status()).toBe(200);
    expect(await loginResponse.headerValue('access-control-allow-origin')).toBe(web);
    expect(await loginResponse.headerValue('access-control-allow-credentials')).toBe('true');
    await expect(page).toHaveURL(/\/relay-devices$/);
    const created = await page.evaluate(async (relayOrigin) => {
      const response = await fetch(`${relayOrigin}/relay/devices`, {
        method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name: 'Cross-origin smoke device' }),
      });
      await response.body?.cancel();
      return response.status;
    }, relay);
    expect(created).toBe(200);
    const portal = page.waitForResponse((response) => response.url() === `${relay}/relay/portal` && response.request().method() === 'GET');
    await page.reload();
    const portalResponse = await portal;
    expect(portalResponse.status()).toBe(200);
    await expect(page.getByText('Cross-origin smoke device', { exact: true }).first()).toBeVisible();
    const cookies = await context.cookies(relay);
    const session = cookies.find((cookie) => cookie.name === 'remote_codex_relay_session');
    expect(Boolean(session?.httpOnly && session.secure && session.sameSite === 'Lax')).toBe(true);
    expect(apiRequests.length).toBeGreaterThan(2);
    expect(apiRequests.every((origin) => origin === relay)).toBe(true);
    expect(new URL(page.url()).origin).toBe(web);
    console.log('Observed: cross-origin login 200; relay HttpOnly/Secure/SameSite=Lax cookie; authenticated portal 200 after reload; device visible on Web origin; all /relay requests targeted relay origin.');
  } finally {
    for (const child of processes.reverse()) {
      const exited = new Promise<void>((done) => child.once('exit', () => done()));
      if (child.pid && process.platform !== 'win32') {
        try { process.kill(-child.pid, 'SIGTERM'); } catch { /* process group already exited */ }
      } else child.kill('SIGTERM');
      if (child.exitCode === null) await exited;
    }
    await rm(root, { recursive: true, force: true });
  }
});
