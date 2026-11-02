import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { randomBytes, createHash } from 'node:crypto';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';

test.use({ trace: 'off', actionTimeout: 15_000 });
test('relay consent requires a browser choice before account selection and supports cancellation', async ({ page, context }) => {
  await mkdir(resolve('.scratch/consent-review'), { recursive: true });
  const root = await mkdtemp(resolve('.scratch/consent-review/browser-'));
  const provider = createServer((request, response) => { if (request.method !== 'GET') { response.writeHead(405); response.end(); return; } response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' }); response.end('<h1>Mock provider account selection</h1>'); });
  await new Promise<void>(done => provider.listen(0, '127.0.0.1', done));
  const providerBase = `http://127.0.0.1:${(provider.address() as import('node:net').AddressInfo).port}`;
  const reservation = createServer();
  await new Promise<void>(done => reservation.listen(0, '127.0.0.1', done));
  const relayPort = (reservation.address() as import('node:net').AddressInfo).port;
  await new Promise<void>(done => reservation.close(() => done()));
  const base = `http://localhost:${relayPort}`;
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  const relayProcess = spawn(resolve('target/debug/pockymoe'), ['relay'], { stdio: 'ignore', env: {
    ...env, HOST: '127.0.0.1', PORT: String(relayPort), POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
    POCKYMOE_ADMIN_USERNAME: 'consent-test-admin', POCKYMOE_ADMIN_PASSWORD: randomBytes(24).toString('hex'),
    POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'), POCKYMOE_PUBLIC_BASE_URL: base,
    POCKYMOE_WEB_ORIGIN: 'https://app.example.com', POCKYMOE_TRUSTED_PROXY_IPS: '127.0.0.1',
    POCKYMOE_GOOGLE_OAUTH_CLIENT_ID: 'synthetic-client', POCKYMOE_GOOGLE_OAUTH_CLIENT_SECRET: 'synthetic-secret',
    POCKYMOE_GITHUB_OAUTH_CLIENT_ID: 'synthetic-client', POCKYMOE_GITHUB_OAUTH_CLIENT_SECRET: 'synthetic-secret',
    POCKYMOE_GOOGLE_OAUTH_AUTHORIZE_URL: `${providerBase}/google`, POCKYMOE_GITHUB_OAUTH_AUTHORIZE_URL: `${providerBase}/github`,
  } });
  const proof = randomBytes(32).toString('hex');
  const proofHash = createHash('sha256').update(proof).digest('base64url');
  const prepare = async (provider: string) => {
    const response = await context.request.post(`${base}/relay/auth/mobile/prepare`, { headers: { origin: 'https://app.example.com' }, data: { provider, proofHash } });
    expect(response.ok()).toBeTruthy(); return response.json();
  };
  try {
    await expect.poll(() => fetch(`${base}/healthz`).then(r => r.status).catch(() => 0)).toBe(200);
    const start = await prepare('google');
    const screenshots = resolve('.scratch/consent-review/screenshots'); await mkdir(screenshots, { recursive: true });
    // Consent is deliberately Classic/system-mode on the API origin; selecting a
    // Web theme cannot alter the warning's meaning or hide the explicit choice.
    for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark'] as const) {
      await page.emulateMedia({ colorScheme: mode });
      await page.goto(base + start.path);
      await page.evaluate(pack => localStorage.setItem('pockymoe-theme-preset', pack), pack);
      await expect(page.getByRole('heading', { name: 'Pockymoe 手机 App 正在请求登录' })).toBeVisible();
      await expect(page.getByText('如果不是你刚刚发起的，请关闭此页')).toBeVisible();
      for (const width of [393, 1440]) {
        await page.setViewportSize({ width, height: 900 });
        await expect(page.getByRole('button', { name: '继续', exact: true })).toBeVisible();
        await expect(page.getByRole('button', { name: '取消', exact: true })).toBeVisible();
        expect(page.url()).toBe(base + start.path);
        await page.screenshot({ path: join(screenshots, `consent-${pack}-${mode}-${width}.png`) });
      }
    }
    const continued = page.waitForResponse(response => response.url() === `${base}/relay/auth/mobile/consent` && response.request().method() === 'POST');
    await page.getByRole('button', { name: '继续', exact: true }).click();
    expect((await continued).status()).toBe(303);
    await expect(page.getByRole('heading', { name: 'Mock provider account selection' })).toBeVisible();
    expect(new URL(page.url()).searchParams.get('prompt')).toBe('select_account');
    expect(new URL(page.url()).pathname).toBe('/google');
    const cancellation = await prepare('google'); await page.goto(base + cancellation.path);
    let callback = '';
    await page.route('**/relay/auth/mobile/consent', async route => {
      const response = await route.fetch({ maxRedirects: 0 });
      expect(response.status()).toBe(303); callback = response.headers().location;
      // The real handler returns the app callback. Only its final OS dispatch is
      // substituted; this is not a real custom-scheme application launch test.
      await route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', body: '<p>已取消</p>' });
    });
    await page.getByRole('button', { name: '取消', exact: true }).click();
    await expect(page.getByText('已取消')).toBeVisible();
    expect(callback.startsWith('remotecodex://auth/callback?code=')).toBe(true);
    const exchange = await context.request.post(`${base}/relay/auth/mobile/exchange`, { headers: { origin: 'https://app.example.com' }, data: { proof, code: new URL(callback).searchParams.get('code') } });
    expect(exchange.ok()).toBeTruthy(); expect((await exchange.json()).path).toContain('oauth_cancelled');
    await page.unroute('**/relay/auth/mobile/consent');
    const github = await prepare('github'); await page.goto(base + github.path);
    await page.getByRole('button', { name: '继续', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Mock provider account selection' })).toBeVisible();
    expect(new URL(page.url()).searchParams.get('prompt')).toBe('select_account'); expect(new URL(page.url()).pathname).toBe('/github');
  } finally {
    if (relayProcess.exitCode === null && relayProcess.signalCode === null) { const stopped = once(relayProcess, 'exit'); relayProcess.kill('SIGTERM'); await stopped; }
    await new Promise<void>(done => provider.close(() => done()));
    await rm(root, { recursive: true, force: true });
  }
});
