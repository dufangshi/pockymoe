import { test, expect, type CDPSession, type Page, type TestInfo } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

// The pairing fragment and exported virtual-authenticator private key must stay
// in memory. A trace would record navigation URLs and CDP/evaluate arguments.
test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

async function freePort() {
  const server = createServer();
  await new Promise<void>(done => server.listen(0, '127.0.0.1', done));
  const port = (server.address() as { port: number }).port;
  await new Promise<void>(done => server.close(() => done()));
  return port;
}

async function authenticator(page: Page) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2', transport: 'internal', hasResidentKey: true,
      hasUserVerification: true, isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  return { cdp, authenticatorId };
}

// Read only public metadata from the real browser's session store, never its key.
async function ownerSessionId(page: Page) {
  return page.evaluate(async () => {
    if (!(await indexedDB.databases()).some(db => db.name === 'pockymoe-owner-trust-v1')) return null;
    const db = await new Promise<IDBDatabase>((done, fail) => {
      const request = indexedDB.open('pockymoe-owner-trust-v1', 1);
      request.onsuccess = () => done(request.result);
      request.onerror = () => fail(new Error('Owner trust database unavailable'));
    });
    try {
      return await new Promise<string | null>((done, fail) => {
        const request = db.transaction('trust').objectStore('trust').get('session');
        request.onsuccess = () => done(request.result?.sessionId ?? null);
        request.onerror = () => fail(new Error('Owner session unavailable'));
      });
    } finally { db.close(); }
  });
}

async function browserRequest(page: Page, path: string, method = 'GET', body?: unknown) {
  // The production Service Worker performs encryption and owner signing.
  return page.evaluate(async ({ path, method, body }) => {
    const response = await fetch(path, {
      method, credentials: 'same-origin', headers: { 'content-type': 'application/json' },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    return { status: response.status, data: await response.json() };
  }, { path, method, body });
}

async function openTrustPanel(page: Page, info: TestInfo) {
  await page.locator('button[data-encryption="encrypted"]').first().click();
  const manage = page.getByRole('button', { name: 'Manage device trust', exact: true });
  await expect(manage).toBeVisible();
  if (info.project.name === 'mobile-chromium') {
    await expect.soft(manage, 'MOBILE_TRUST_POPOVER: Manage device trust must be reachable inside the phone viewport').toBeInViewport({ ratio: 1, timeout: 1_000 });
    const box = await manage.boundingBox();
    if (box && (box.y + box.height > page.viewportSize()!.height || box.x < 0)) {
      await screenshot(page, info, 'device-trust-popover-overflow');
      const geometry = JSON.stringify({ button: box, viewport: page.viewportSize() });
      await writeFile(resolve('.scratch/owner-pairing-mobile-popover-geometry.json'), geometry);
      await info.attach('trust-popover-geometry', {
        body: geometry, contentType: 'application/json',
      });
      // Preserve the touch-layout failure, then activate with the keyboard so
      // the requested trust-panel screenshot can still be captured.
      await manage.focus();
      await manage.press('Enter');
    } else await manage.click();
  } else await manage.click();
  const panel = page.getByRole('dialog', { name: 'Manage device trust', exact: true });
  await expect(panel.getByText('Strict mode enabled', { exact: true })).toBeVisible();
  return panel;
}

async function screenshot(page: Page, info: TestInfo, name: string) {
  await mkdir(resolve('.scratch/screenshots'), { recursive: true });
  await page.screenshot({ path: resolve(`.scratch/screenshots/${info.project.name}-${name}.png`) });
}

async function sendPrompt(page: Page, marker: string) {
  const frames: string[] = [];
  page.on('websocket', socket => {
    if (new URL(socket.url()).pathname.endsWith('/ws')) {
      socket.on('framereceived', frame => frames.push(String(frame.payload)));
    }
  });
  return async (url: string) => {
    await page.goto(url);
    const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true });
    await expect(prompt).toBeVisible();
    // Wait for the real secure channel before sending, rather than accepting a
    // historical reply recovered by a later HTTP fetch.
    await expect.poll(() => frames.some(frame => frame.includes('"encrypted"')),
      { message: 'The strict device must establish an encrypted WebSocket' }).toBe(true);
    const receivedBefore = frames.length;
    await prompt.fill(`Reply with exactly ${marker}`);
    await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
    await expect(page.getByText(marker, { exact: true })).toBeVisible();
    expect(frames.slice(receivedBefore).some(frame => frame.includes('"encrypted"')),
      'The fake runtime reply must arrive over the encrypted WebSocket').toBe(true);
    expect(frames.some(frame => frame.includes(marker)),
      'The relay WebSocket must never see the plaintext prompt/reply').toBe(false);
  };
}

test('owner passkey pairs a strict device, trusts a second browser, revokes and resets', async ({ page, browser, context }, info) => {
  const screenshotsOnly = info.project.name === 'mobile-chromium';
  const root = await mkdtemp(join(tmpdir(), 'pockymoe-owner-pairing-'));
  const binary = resolve('target/debug/pockymoe');
  const base = `http://localhost:${await freePort()}`;
  const password = randomBytes(24).toString('hex');
  const secrets = [password];
  const processes: ChildProcess[] = [];
  const logs: string[] = [];
  const events: string[] = [];
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  const redact = (value: string) => secrets.reduce((text, secret) =>
    text.split(secret).join('[REDACTED]'), value)
    .replace(/(secret=)[^&\s]+/g, '$1[REDACTED]');
  function start(command: string, extra: Record<string, string>) {
    const sessionSecret = randomBytes(32).toString('hex');
    secrets.push(sessionSecret);
    const proc = spawn(binary, [command], {
      env: {
        ...env, HOST: '127.0.0.1', POCKYMOE_E2E_FAKE_RUNTIME: '1',
        POCKYMOE_ADMIN_USERNAME: 'pairing-admin', POCKYMOE_ADMIN_PASSWORD: password,
        POCKYMOE_SESSION_SECRET: sessionSecret, POCKYMOE_RELAY_SESSION_SECRET: sessionSecret,
        ...extra,
      }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    proc.stdout?.on('data', data => logs.push(`[${command}] ${String(data)}`));
    proc.stderr?.on('data', data => logs.push(`[${command}] ${String(data)}`));
    processes.push(proc);
  }
  async function cli(args: string[]) {
    // In particular, never inherit stdout for `pair` or put its output in an assertion.
    return new Promise<{ code: number | null; output: string }>((done, fail) => {
      let output = '';
      const child = spawn(binary, args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
      child.stdout.on('data', data => { output += String(data); });
      child.stderr.on('data', data => { output += String(data); });
      child.once('error', () => fail(new Error('Pair CLI could not start')));
      child.once('exit', code => done({ code, output }));
    });
  }
  async function api(path: string, method = 'GET', body?: unknown, cookie?: string) {
    const response = await fetch(base + path, {
      method, headers: {
        'content-type': 'application/json', origin: base, ...(cookie ? { cookie } : {}),
      }, ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(10_000),
    });
    const text = await response.text();
    const data = text ? JSON.parse(text) : null;
    events.push(`fixture ${method} ${path.split('?')[0]} ${response.status}${data?.code ? ` code=${data.code}` : ''}`);
    return { status: response.status, data };
  }
  function ok(result: { status: number; data: any }, label: string) {
    // Successful auth/device bodies include tokens; never use the body as a label.
    expect(result.status, label).toBeGreaterThanOrEqual(200);
    expect(result.status, label).toBeLessThan(300);
    return result.data;
  }
  async function connectDevice(name: string, count: number) {
    const device = ok(await api('/relay/devices', 'POST', { name }, cookie), 'Create isolated device');
    secrets.push(device.token);
    const port = String(await freePort());
    const db = join(root, `device-${count}.sqlite`);
    start('relay-supervisor', {
      PORT: port, POCKYMOE_RELAY_SUPERVISOR_PORT: port,
      POCKYMOE_RELAY_SERVER_URL: base, POCKYMOE_RELAY_AGENT_TOKEN: device.token,
      POCKYMOE_DATABASE_PATH: db, DATABASE_URL: db,
      POCKYMOE_WORKSPACE_ROOT: join(root, `workspaces-${count}`),
      WORKSPACE_ROOT: join(root, `workspaces-${count}`),
    });
    await expect.poll(async () => (await api('/healthz')).data.connectedSupervisors).toBe(count);
    return { id: device.device.id as string, db, api: `/relay/devices/${device.device.id}/api` };
  }
  const observe = (observedContext: typeof context) => observedContext.on('response', response => {
    const path = new URL(response.url()).pathname;
    if (path.startsWith('/relay/')) events.push(`${response.request().method()} ${path} ${response.status()} encrypted=${response.headers()['x-rcd-encrypted'] === '1'}`);
  });
  observe(context);
  page.on('pageerror', error => events.push(`pageerror: ${error.message}`));
  let cookie: string;
  let secondContext: Awaited<ReturnType<typeof browser.newContext>> | undefined;
  let cdp: CDPSession | undefined;
  try {
    start('relay', {
      PORT: new URL(base).port, POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_DATABASE_PATH: join(root, 'relay.sqlite'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_PUBLIC_BASE_URL: base,
      POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'),
    });
    await expect.poll(() => fetch(base + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
    ok(await api('/relay/auth/register', 'POST', {
      username: 'pairing-owner', email: 'pairing-owner@example.test', password,
    }), 'Register relay user');
    await context.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
    const firstAuth = await authenticator(page);
    cdp = firstAuth.cdp;
    await page.goto(base + '/relay-portal?returnTo=%2Frelay-account');
    await page.getByLabel('Email or username', { exact: true }).fill('pairing-owner');
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Security', exact: true })).toBeVisible();
    const sessionCookie = (await context.cookies(base)).find(c => c.name === 'remote_codex_relay_session');
    expect(!!sessionCookie, 'Real UI sign-in must set the relay session cookie').toBe(true);
    secrets.push(sessionCookie!.value);
    cookie = `remote_codex_relay_session=${sessionCookie!.value}`;
    const user = ok(await api('/relay/auth/session', 'GET', undefined, cookie), 'Relay owner session').user;
    const dismissTour = ({ id }: { id: string }) => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, `relay:${id}`])}`,
      JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
    await context.addInitScript(dismissTour, user);
    await page.getByRole('button', { name: 'Add passkey', exact: true }).click();
    await page.getByRole('textbox', { name: 'Passkey name', exact: true }).fill('Synced owner passkey');
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect(page.getByText('Synced owner passkey', { exact: true })).toBeVisible();
    const records = ok(await api('/relay/account/passkeys/public', 'GET', undefined, cookie), 'Public passkey directory');
    expect(records.passkeys).toHaveLength(1);
    expect(records.passkeys[0]).toMatchObject({ rpId: 'localhost', origin: base, algorithm: -7 });
    const credentials = (await cdp.send('WebAuthn.getCredentials', { authenticatorId: firstAuth.authenticatorId })).credentials;
    expect(credentials.length, 'UI registration must create one real virtual passkey').toBe(1);
    secrets.push(credentials[0]!.privateKey);

    const device = await connectDevice('Owner paired device', 1);
    const work = join(root, 'workspace');
    await mkdir(work);
    const workspace = ok(await api(device.api + '/workspaces', 'POST', {
      label: 'Owner trust workspace', absPath: work,
    }, cookie), 'Seed workspace before pairing');
    const thread = ok(await api(device.api + '/threads/start', 'POST', {
      workspaceId: workspace.id, model: 'fake', title: 'Owner trust thread',
    }, cookie), 'Seed fake-runtime thread before pairing');
    const threadId = thread.thread?.id ?? thread.id;
    const threadUrl = `${base}/devices/${device.id}/threads/${threadId}`;
    let pairing = await cli(['pair', '--database', device.db, '--web-origin', base]);
    if (pairing.code !== 0) logs.push(`pair CLI failed: ${pairing.output.replace(/http[^\s]*\/pair#[^\s]*/g, '[REDACTED PAIR URL]')}`);
    if (!screenshotsOnly) expect.soft(pairing.code, 'PAIR_CLI_IDENTITY: a freshly relay-connected device must create an offer before any browser key handshake').toBe(0);
    if (pairing.code !== 0 && pairing.output.includes('No such file or directory')) {
      // Test-only continuation for the lazy identity sidecar bug. Keep the
      // failed assertion above visible while reaching the remaining checks.
      ok(await api(device.api + '/transport/key?challenge=pairing-e2e-bootstrap', 'GET', undefined, cookie), 'Initialize device transport identity for remaining checks');
      pairing = await cli(['pair', '--database', device.db, '--web-origin', base]);
    }
    expect(pairing.code, 'Local pair CLI must create an offer after transport initialization').toBe(0);
    const pairUrl = pairing.output.match(/http:\/\/localhost:\d+\/pair#[^\s\x1b]+/)?.[0];
    expect(!!pairUrl, 'Pair CLI must print a pairing URL (kept in memory)').toBe(true);
    secrets.push(pairUrl!, new URLSearchParams(new URL(pairUrl!).hash.slice(1)).get('secret')!);
    // CDP navigation avoids Playwright's goto call log retaining the fragment.
    await cdp.send('Page.navigate', { url: pairUrl! }).catch(() => { throw new Error('Pair page navigation failed'); });
    await expect(page.getByRole('button', { name: 'Pair with passkey', exact: true })).toBeVisible();
    expect(new URL(page.url()).hash, 'Pair page must scrub the secret fragment').toBe('');
    await screenshot(page, info, 'pair');
    await page.getByRole('button', { name: 'Pair with passkey', exact: true }).click();
    await expect(page.getByText('Pairing saved. Unlock with your passkey to finish connecting.', { exact: true })).toBeVisible();
    const certificate = ok(await api(`/relay/devices/${device.id}/certificate`, 'GET', undefined, cookie), 'Relay must persist device certificate');
    expect(certificate).toMatchObject({ version: 1, deviceId: device.id,
      assertion: { credentialId: records.passkeys[0].credentialId } });
    const plaintext = await api(device.api + '/workspaces', 'GET', undefined, cookie);
    expect(plaintext.status, 'A paired device must reject a cookie-authenticated plaintext API request').toBe(403);
    expect(plaintext.data.code).toBe('device_strict_mode');
    await page.getByRole('button', { name: 'Unlock with passkey', exact: true }).click();
    await expect(page.getByText('Device paired. Owner passkey protection is enabled.', { exact: true })).toBeVisible();
    await page.getByRole('link', { name: 'Open device', exact: true }).click();
    await expect(page.getByText('Owner trust workspace', { exact: true })).toBeVisible();
    const trust = await browserRequest(page, device.api + '/transport/trust');
    expect(trust.status).toBe(200);
    expect(trust.data).toMatchObject({ paired: true, strict: true, deviceId: device.id });
    if (!screenshotsOnly) await (await sendPrompt(page, 'OWNER_FIRST_BROWSER_REPLY'))(threadUrl);

    secondContext = await browser.newContext(info.project.use);
    observe(secondContext);
    await secondContext.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
    await secondContext.addInitScript(dismissTour, user);
    await secondContext.addCookies([sessionCookie!]);
    const secondPage = await secondContext.newPage();
    const secondAuth = await authenticator(secondPage);
    for (const credential of credentials) await secondAuth.cdp.send('WebAuthn.addCredential', {
      authenticatorId: secondAuth.authenticatorId, credential,
    });
    await secondPage.goto(base + '/relay-account');
    expect(await ownerSessionId(secondPage), 'Second browser must have no inherited owner session').toBeNull();
    // Account has no device fetches: its fresh stores must have no identity pin.
    expect(await secondPage.evaluate(async () => {
      if (!(await indexedDB.databases()).some(db => db.name === 'remote-codex-transport-v1')) return false;
      const db = await new Promise<IDBDatabase>(done => {
        const request = indexedDB.open('remote-codex-transport-v1', 1);
        request.onsuccess = () => done(request.result);
      });
      try { return await new Promise<boolean>(done => {
        const request = db.transaction('identities').objectStore('identities').count();
        request.onsuccess = () => done(request.result > 0);
      }); } finally { db.close(); }
    }), 'Second browser must start without trust-on-first-use identity pins').toBe(false);
    await secondPage.goto(`${base}/devices/${device.id}/workspaces`);
    const unlock = secondPage.getByRole('dialog', { name: 'Unlock your devices', exact: true });
    await expect(unlock).toBeVisible();
    await expect(unlock.getByRole('button', { name: 'Unlock with passkey', exact: true })).toBeEnabled();
    await screenshot(secondPage, info, 'unlock');
    await unlock.getByRole('button', { name: 'Unlock with passkey', exact: true }).click();
    await expect(unlock).toHaveCount(0);
    if (!screenshotsOnly) await expect.soft(secondPage.getByText('Owner trust workspace', { exact: true }),
      'UNLOCK_RELOAD: workspace reads must resume after a successful explicit unlock').toBeVisible({ timeout: 5_000 });
    if (await secondPage.getByRole('button', { name: 'Retry', exact: true }).isVisible()) {
      // Test-only continuation: WorkspacesPage currently keeps its initial
      // owner_session_required error after the global unlock succeeds.
      await secondPage.getByRole('button', { name: 'Retry', exact: true }).click();
    }
    await expect(secondPage.getByText('Owner trust workspace', { exact: true })).toBeVisible();
    expect(await secondPage.evaluate(async deviceId => {
      const db = await new Promise<IDBDatabase>(done => {
        const request = indexedDB.open('pockymoe-owner-trust-v1', 1);
        request.onsuccess = () => done(request.result);
      });
      try { return await new Promise<boolean>(done => {
        const request = db.transaction('trust').objectStore('trust').get(`certificate:${deviceId}`);
        request.onsuccess = () => done(request.result?.deviceId === deviceId);
      }); } finally { db.close(); }
    }, device.id), 'Fresh browser must verify and store the relay certificate with its own passkey').toBe(true);
    if (!screenshotsOnly) await (await sendPrompt(secondPage, 'OWNER_SECOND_BROWSER_REPLY'))(threadUrl);
    const secondId = await ownerSessionId(secondPage);
    expect(!!secondId).toBe(true);
    expect(secondId).not.toBe(await ownerSessionId(page));
    await page.goto(base + '/relay-devices');
    const panel = await openTrustPanel(page, info);
    await expect(panel.getByRole('button', { name: 'Revoke session', exact: true })).toHaveCount(2);
    await screenshot(page, info, 'device-trust');
    if (screenshotsOnly) return; // Mobile is visual QA; protocol coverage runs once on desktop.

    const row = panel.getByRole('listitem').filter({ has: page.locator('code').filter({ hasText: secondId! }) });
    await row.getByRole('button', { name: 'Revoke session', exact: true }).click();
    await expect(row).toHaveCount(0);
    const afterRevoke = await browserRequest(secondPage, device.api + '/workspaces');
    // Keep these assertions red on the pre-tombstone integration base. Soft
    // assertions allow independent unpaired/reset checks to run and report too.
    expect.soft(afterRevoke.status, 'REVOCATION: a revoked owner session must not silently re-register and regain workspace access').toBe(401);
    await expect.soft(secondPage.getByRole('dialog', { name: 'Unlock your devices', exact: true }),
      'REVOCATION: second browser must require a fresh explicit passkey unlock').toBeVisible({ timeout: 5_000 });
    const sessions = await browserRequest(page, device.api + '/transport/trust');
    expect.soft(sessions.data.sessions.some((s: { sessionId: string }) => s.sessionId === secondId),
      'REVOCATION: deleted session must remain revoked after the client retries registration').toBe(false);
    // Once the revocation fix lands, verify a fresh gesture produces a new key.
    if (afterRevoke.status === 401) {
      await secondPage.getByRole('dialog', { name: 'Unlock your devices', exact: true })
        .getByRole('button', { name: 'Unlock with passkey', exact: true }).click();
      await expect.poll(() => ownerSessionId(secondPage)).not.toBe(secondId);
      expect((await browserRequest(secondPage, device.api + '/workspaces')).status).toBe(200);
    }

    const unpaired = await connectDevice('Unpaired legacy trust device', 2);
    expect((await api(unpaired.api + '/workspaces', 'GET', undefined, cookie)).status,
      'Unpaired device must preserve cookie-authenticated plaintext API access').toBe(200);
    await page.keyboard.press('Escape');
    await page.goto(base + '/relay-devices');
    await page.locator('article').filter({ hasText: 'Unpaired legacy trust device' })
      .getByRole('button', { name: 'Device connection encrypted', exact: true }).click();
    await expect(page.getByText('Unpaired · Relay trusted', { exact: true })).toBeVisible();

    const reset = await cli(['pair', '--database', device.db, '--reset', '--yes']);
    expect(reset.code, 'Local confirmed reset must succeed').toBe(0);
    expect((await api(device.api + '/workspaces', 'GET', undefined, cookie)).status,
      'Plaintext API access must work again immediately after local reset').toBe(200);
    const resetKey = ok(await api(device.api + '/transport/key?challenge=pairing-e2e-reset', 'GET', undefined, cookie), 'Reset device key status');
    expect(resetKey.trust).toMatchObject({ paired: false, strict: false });
  } finally {
    await secondContext?.close();
    await Promise.all(processes.map(proc => new Promise<void>(done => {
      if (proc.exitCode !== null || proc.signalCode !== null) return done();
      const timer = setTimeout(() => proc.kill('SIGKILL'), 2_000);
      proc.once('exit', () => { clearTimeout(timer); done(); });
      proc.kill('SIGTERM');
    })));
    const evidence = redact([...events, ...logs].join('\n'));
    await writeFile(resolve(`.scratch/owner-pairing-${info.project.name}.log`), evidence);
    if (info.errors.length) await info.attach('sanitized-process-and-http-log', { body: evidence, contentType: 'text/plain' });
    await rm(root, { recursive: true, force: true });
  }
});
