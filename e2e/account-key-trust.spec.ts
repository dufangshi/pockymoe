import { test, expect } from '@playwright/test';
import { createServer, type RequestListener } from 'node:http';
import { randomBytes } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fixture, browserRequest, type Device, type Result, retainBrowserRequest, ok, sendPrompt, sockets, screenshot, unlock, UNLOCK_DIALOG, readRecoveryCode, confirmRecoveryCode } from './helpers/account-key-fixture';

// Passwords, recovery codes, binding commands and relay cookies stay in memory.
test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

test('preview leases authorize bound Web, CLI and thread-recipient previews while unbound previews stay compatible', async ({ browser }, info) => {
  test.skip(info.project.name !== 'desktop-chromium', 'Preview protocol coverage runs on desktop only');
  test.setTimeout(300_000);
  const f = await fixture(browser, info, { previews: true });
  const requests: { path: string; lease: boolean; app: string | null }[] = [];
  const handler: RequestListener = (request, response) => {
    const url = new URL(request.url ?? '/', 'http://127.0.0.1:16275');
    // Keep only safe facts: lease ids and preview cookies never enter evidence.
    requests.push({ path: url.pathname, lease: url.searchParams.has('lease'), app: url.searchParams.get('app') });
    response.writeHead(200, { 'content-type': 'text/html', 'cache-control': 'no-store' });
    response.end('<!doctype html><html><body><h1>ACCOUNT_KEY_PREVIEW</h1></body></html>');
  };
  const service = createServer(handler);
  // A thread's own agent maps this second port: when a turn starts, the fake
  // harness runs the driver with that thread's CLI credential.
  const shared = createServer(handler);
  const driver = join(f.root, 'thread-driver.sh');
  await writeFile(driver, `#!/bin/sh\nout="${f.root}/preview-$POCKYMOE_THREAD_ID"\n"${f.binary}" preview create --port 16276 --path /shared-preview > "$out.part" && mv "$out.part" "$out.json"\n`, { mode: 0o755 });
  const agentDriver = { POCKYMOE_E2E_FAKE_CLI_DRIVER: driver };
  // Navigation errors can include the lease and one-time login ticket in their
  // URLs. Retain only a status, and diagnose failures with the labeled assertions.
  const navigate = async (page: import('@playwright/test').Page, url: string) => {
    try { return (await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 20_000 }))?.status() ?? 0; }
    catch { return 0; }
  };
  try {
    await new Promise<void>((done, fail) => { service.once('error', fail); service.listen(16275, '127.0.0.1', done); });
    const owner = await f.account('preview-owner');
    const bound = await f.device(owner, 1, agentDriver);
    await f.bind(owner, bound);
    await owner.page.locator('[aria-controls="matter-thread-tools"]').click();
    await owner.page.getByRole('button', { name: 'Port mappings', exact: true }).click();
    const manager = owner.page.getByRole('dialog', { name: 'Port mappings', exact: true });
    await expect(manager.getByRole('button', { name: 'Enable', exact: true })).toBeEnabled();
    await manager.getByRole('spinbutton', { name: 'HTTP port', exact: true }).fill('16275');
    await manager.getByRole('textbox', { name: 'Port label', exact: true }).fill('Lease test app');
    await manager.getByRole('button', { name: 'Enable', exact: true }).click();
    await expect(manager.getByText('Lease test app · 127.0.0.1:16275', { exact: true })).toBeVisible();
    const mappings = ok(await browserRequest(owner.page, bound.api + '/port-mappings'), 'Real UI port mapping').mappings;
    const mapping = mappings.find((row: any) => row.port === 16275);
    expect(!!mapping, 'Real Web UI created the bound device mapping').toBe(true);
    const openPath = `/relay/devices/${bound.id}/port-mappings/${mapping.id}/open`;
    const expectedHost = `p-${mapping.id}.preview.localhost`;
    let webLease: string | null = null, webLeaseStatus = 0, webOpenStatus = 0;
    owner.page.on('request', request => {
      const url = new URL(request.url());
      if (url.pathname === openPath && request.method() === 'POST') {
        webLease = url.searchParams.get('lease');
        if (webLease) f.secrets.push(webLease);
      }
    });
    owner.page.on('response', response => {
      const path = new URL(response.url()).pathname;
      if (path === bound.api + `/port-mappings/${mapping.id}/lease`) webLeaseStatus = response.status();
      if (path === openPath) webOpenStatus = response.status();
    });
    const popup = owner.page.waitForEvent('popup');
    await manager.getByRole('button', { name: 'Open', exact: true }).click();
    const web = await popup;
    await expect.poll(() => webLeaseStatus, { timeout: 5_000, message: 'Real UI obtains an owner-signed lease from the strict device' }).toBe(200);
    expect(typeof webLease === 'string' && /^[A-Za-z0-9_-]{43}$/.test(webLease), 'Device lease uses the specified base64url format').toBe(true);
    await expect.configure({ soft: true }).poll(() => webOpenStatus, { timeout: 5_000,
      message: 'PREVIEW_WEB_LEASE_FORMAT: relay must accept the device-issued 43-character base64url lease, rather than require a UUID' }).toBe(200);
    await expect.configure({ soft: true }).poll(() => web.getByRole('heading', { name: 'ACCOUNT_KEY_PREVIEW', exact: true }).isVisible().catch(() => false),
      { timeout: 5_000, message: 'PREVIEW_WEB_LOAD: owner-signed Web preview must reach the local HTTP app through the relay preview origin; a closed failure popup is not a loaded app' }).toBe(true);
    expect.soft(!web.isClosed() && new URL(web.url()).hostname === expectedHost,
      'PREVIEW_WEB_ORIGIN: Web preview must load on the mapping-specific relay origin').toBe(true);
    const webRequests = requests.filter(request => request.path === '/');
    expect.soft(webRequests.length > 0 && webRequests.every(request => !request.lease),
      'PREVIEW_WEB_QUERY_STRIPPED: bound Web preview must reach the app without a lease query parameter; blocked launch also fails this check').toBe(true);
    f.events.push(`preview Web leaseStatus=${webLeaseStatus} openStatus=${webOpenStatus} upstreamRequests=${webRequests.length}`);

    const cli = await f.cli(['--cli-config', bound.cli, 'preview', 'create', '--port', '16275', '--path', '/cli-preview?app=cli']);
    expect(cli.code, 'Local agent CLI reserves a preview on the bound device').toBe(0);
    const cliLease = cli.data?.leaseId;
    expect(typeof cliLease === 'string' && /^[A-Za-z0-9_-]{43}$/.test(cliLease), 'Local CLI creates its own live device lease').toBe(true);
    f.secrets.push(cliLease);
    expect(cliLease === webLease, 'Local CLI and owner browser leases are distinct').toBe(false);
    const cliURL = new URL(cli.data.openUrl);
    expect(cliURL.searchParams.get('lease') === cliLease, 'Agent-facing CLI openUrl carries its local lease').toBe(true);
    const cliPage = await owner.context.newPage();
    let cliLeaseRequests = 0, cliOpenedWithOwnLease = false, cliOpenStatus = 0;
    cliPage.on('request', request => {
      const url = new URL(request.url());
      if (url.pathname === bound.api + `/port-mappings/${mapping.id}/lease`) cliLeaseRequests++;
      if (url.pathname === openPath) {
        const lease = url.searchParams.get('lease');
        if (lease) f.secrets.push(lease);
        cliOpenedWithOwnLease = lease === cliLease;
      }
    });
    cliPage.on('response', response => { if (new URL(response.url()).pathname === openPath) cliOpenStatus = response.status(); });
    await navigate(cliPage, cliURL.href);
    await expect.soft(cliPage.getByRole('heading', { name: 'ACCOUNT_KEY_PREVIEW', exact: true }),
      'PREVIEW_CLI_LOAD: agent CLI openUrl must load the app through its own lease').toBeVisible({ timeout: 5_000 });
    // A signed-in owner's link is redeemed by the Relay's document route with the
    // link's lease, without the Web page's open POST; a page fallback must POST it.
    expect.soft((cliOpenStatus === 0 || cliOpenedWithOwnLease) && cliLeaseRequests === 0,
      'PREVIEW_CLI_LEASE_REUSED: preview page must use the supplied local CLI lease without requesting a browser lease').toBe(true);
    expect.soft(new URL(cliPage.url()).hostname === expectedHost,
      'PREVIEW_CLI_ORIGIN: CLI preview must load on the mapping-specific relay origin').toBe(true);
    const cliRequests = requests.filter(request => request.path === '/cli-preview');
    expect.soft(cliRequests.length > 0 && cliRequests.every(request => !request.lease && request.app === 'cli'),
      'PREVIEW_CLI_QUERY_STRIPPED: relay must strip lease while preserving the application query; blocked launch also fails this check').toBe(true);
    f.events.push(`preview CLI openStatus=${cliOpenStatus} usedOwnLease=${cliOpenedWithOwnLease} browserLeaseRequests=${cliLeaseRequests} upstreamRequests=${cliRequests.length}`);

    // Bypass all browser signing and call only the relay's preview-open API.
    const missing = await f.api(openPath, 'POST', { path: '/missing-lease' }, owner.cookie);
    expect.soft(missing.status, 'PREVIEW_MISSING_LEASE: relay-only request cannot open a strict-device mapping without a lease').toBe(403);
    expect.soft(missing.data?.code, 'PREVIEW_MISSING_LEASE_CODE: no-lease request must require device authorization').toBe('preview_lease_required');
    const foreignLease = randomBytes(32).toString('base64url'); f.secrets.push(foreignLease);
    const foreign = await f.api(openPath + '?lease=' + foreignLease, 'POST', { path: '/foreign-lease' }, owner.cookie);
    expect.soft(foreign.status,
      'PREVIEW_FOREIGN_LEASE_ROUTED: syntactically valid unissued lease must reach device enforcement, rather than fail relay UUID validation').toBe(200);
    if (foreign.status === 200) {
      const foreignPage = await owner.context.newPage();
      const status = await navigate(foreignPage, foreign.data.url);
      expect.soft(status >= 400 && status < 600, 'PREVIEW_FOREIGN_LEASE_DENIED: device refuses the relay open frame without a live mapping lease').toBe(true);
      expect(requests.some(request => request.path === '/foreign-lease'), 'Unissued lease never reaches the previewed app').toBe(false);
      f.events.push(`preview unissued lease upstreamStatus=${status}`);
    }
    expect(requests.some(request => request.path === '/missing-lease'), 'Missing lease never reaches the previewed app').toBe(false);

    // A second real Supervisor stays unbound and accepts the legacy no-lease
    // relay path. It shares only the tiny local service, never trust state.
    const unbound = await f.device(owner, 2, agentDriver);
    const legacy = ok(await f.api(unbound.api + '/port-mappings', 'POST', { port: 16275, label: 'Unbound app' }, owner.cookie), 'Unbound mapping');
    const legacyOpen = `/relay/devices/${unbound.id}/port-mappings/${legacy.id}/open`;
    await expect.poll(async () => (await f.api(legacyOpen, 'POST', { path: '/unbound-preview?app=legacy' }, owner.cookie)).status,
      { message: 'Wait for the unbound mapping heartbeat to reach the relay' }).toBe(200);
    const legacyLaunch = ok(await f.api(legacyOpen, 'POST', { path: '/unbound-preview?app=legacy' }, owner.cookie), 'Unbound preview without lease');
    expect(new URL(legacyLaunch.url).searchParams.has('lease'), 'Unbound launch needs no lease').toBe(false);
    const legacyPage = await owner.context.newPage();
    expect(await navigate(legacyPage, legacyLaunch.url), 'Legacy unbound preview loads without any lease').toBe(200);
    await expect(legacyPage.getByRole('heading', { name: 'ACCOUNT_KEY_PREVIEW', exact: true })).toBeVisible();
    expect(new URL(legacyPage.url()).hostname).toBe(`p-${legacy.id}.preview.localhost`);
    expect(requests.some(request => request.path === '/unbound-preview' && request.app === 'legacy' && !request.lease)).toBe(true);
    expect.soft(requests.some(request => request.lease), 'PREVIEW_LEASE_NOT_FORWARDED: no previewed-app request may receive the lease query parameter').toBe(false);
    f.events.push(`preview missingLease=${missing.status}/${missing.data?.code} unissuedLeaseOpen=${foreign.status} unboundWithoutLease=200 appReceivedLease=${requests.some(request => request.lease)}`);

    // The people a thread is shared with open the CLI link of a mapping that
    // thread's agent made, on a bound and an unbound device, until the share
    // is revoked; before the share they cannot.
    await new Promise<void>((done, fail) => { shared.once('error', fail); shared.listen(16276, '127.0.0.1', done); });
    const friend = await f.account('preview-friend');
    const threadPreview = async (device: Device, label: string, send: (path: string, body: unknown) => Promise<Result>) => {
      const result = join(f.root, `preview-${device.thread}.json`);
      expect((await send(`${device.api}/threads/${device.thread}/prompt`, { prompt: 'Map the shared preview' })).status, `${label}: owner starts a turn`).toBe(200);
      await expect.poll(() => readFile(result, 'utf8').then(text => JSON.parse(text).threadId, () => null),
        { message: `${label}: the agent's own credential records its thread` }).toBe(device.thread);
      const preview = JSON.parse(await readFile(result, 'utf8'));
      f.secrets.push(preview.leaseId);
      expect(preview.access, `${label}: thread mapping`).toBe('thread');
      const page = await friend.context.newPage();
      const before = requests.filter(request => request.path === '/shared-preview').length;
      await navigate(page, preview.openUrl);
      await expect(page.getByRole('alert'), `${label}: without a share the recipient is refused`).toBeVisible();
      expect(requests.filter(request => request.path === '/shared-preview').length).toBe(before);
      // Shared through the relay API to test previews, not the safety-code check.
      ok(await f.api('/relay/account/preferences', 'PATCH', { requireSafetyCode: false }, owner.cookie), `${label}: no safety-code check`);
      const share = ok(await f.api('/relay/shares', 'POST', { targetIdentifier: 'preview-friend', deviceId: device.id, threadId: device.thread, threadAccess: 'read', workspaceAccess: 'none' }, owner.cookie), `${label}: share the thread`);
      await navigate(page, preview.openUrl);
      await expect(page.getByRole('heading', { name: 'ACCOUNT_KEY_PREVIEW', exact: true }), `${label}: the recipient opens the CLI link`).toBeVisible();
      expect(new URL(page.url()).hostname).toBe(`p-${preview.id}.preview.localhost`);
      ok(await f.api(`/relay/shares/${share.id}`, 'DELETE', undefined, owner.cookie), `${label}: revoke the share`);
      const after = await page.evaluate(() => fetch(location.href, { cache: 'no-store' }).then(response => response.status));
      expect(after, `${label}: revoking the share ends access at the next request`).toBe(401);
      f.events.push(`preview thread recipient ${label} opened=true afterRevoke=${after}`);
      await page.close();
    };
    await threadPreview(bound, 'bound', (path, body) => browserRequest(owner.page, path, 'POST', body));
    await threadPreview(unbound, 'unbound', (path, body) => f.api(path, 'POST', body, owner.cookie));
  } finally {
    await f.close();
    service.closeAllConnections(); shared.closeAllConnections();
    await new Promise<void>(done => service.close(() => done()));
    await new Promise<void>(done => shared.close(() => done()));
  }
});

test('account key registration, enrollment, browser trust and recovery', async ({ browser }, info) => {
  test.setTimeout(300_000);
  const mobile = info.project.name === 'mobile-chromium';
  const f = await fixture(browser, info);
  try {
    const transmitted: { path: string; password: boolean; authKey: boolean }[] = [];
    // Observe real HTTP traffic, retaining only safe facts about each body.
    const originalContext = f.context.bind(f);
    f.context = async (...args) => {
      const result = await originalContext(...args);
      result.page.on('request', request => {
        if (!new URL(request.url()).pathname.startsWith('/relay/auth/')) return;
        const raw = request.postData(); if (!raw) return;
        const body = JSON.parse(raw);
        transmitted.push({ path: new URL(request.url()).pathname, password: 'password' in body || raw.includes(f.password), authKey: typeof body.authKey === 'string' });
      });
      return result;
    };
    const owner = await f.account('trust-owner', mobile);
    expect(transmitted.find(r => r.path.endsWith('/register'))).toEqual({ path: '/relay/auth/register', password: false, authKey: true });
    const vault = ok(await f.api('/relay/account/vault', 'GET', undefined, owner.cookie), 'Vault persisted');
    expect(!!vault?.body?.ciphertext && !!vault?.slots?.password && !!vault?.slots?.recovery, 'Only encrypted vault slots and body persisted').toBe(true);
    const d1 = await f.device(owner, 1);
    const trust = await f.bind(owner, d1, mobile);
    if (mobile) {
      const second = await f.login(owner);
      await screenshot(second.page, info, 'v2-security-browsers');
      const login = await f.context();
      await login.page.goto(f.base + '/relay-portal');
      await expect(login.page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
      await screenshot(login.page, info, 'v2-login');
      return; // Mobile captures only the requested product states.
    }
    const cachedBoundDevices = await owner.page.evaluate(async userId => {
      const db = await new Promise<IDBDatabase>((resolve, reject) => {
        const request = indexedDB.open('pockymoe-account-v2');
        request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error);
      });
      try {
        return await new Promise<unknown[]>((resolve, reject) => {
          const store = db.transaction('account').objectStore('account');
          const keys = store.getAllKeys(), values = store.getAll();
          values.onsuccess = () => {
            const index = keys.result.findIndex(key => String(key).endsWith('\ncache:' + userId));
            resolve(values.result[index]?.boundDevices ?? []);
          };
          values.onerror = () => reject(values.error);
        });
      } finally { db.close(); }
    }, owner.userId);
    expect.soft(cachedBoundDevices.some((entry: any) => entry?.deviceId === d1.id && entry.identityKey === trust.certificate.identityKey),
      'BOUND_DEVICE_IDENTITY: vault cache must preserve {deviceId,identityKey}, rather than deviceId alone').toBe(true);
    const strict = await f.api(d1.api + `/threads/${d1.thread}`, 'GET', undefined, owner.cookie);
    expect(strict.status).toBe(403); expect(strict.data.code).toBe('device_strict_mode');
    await sendPrompt(owner.page, 'ACCOUNT_KEY_HISTORY');
    const second = await f.login(owner);
    const channels = sockets(second.page);
    await second.page.goto(d1.url);
    await expect(second.page.getByText('ACCOUNT_KEY_HISTORY', { exact: true })).toBeVisible();
    await sendPrompt(second.page, 'SECOND_BROWSER_ENCRYPTED_REPLY');
    await expect.configure({ soft: true }).poll(() => channels.some(c => c.frames.some(frame => frame.includes('"encrypted"'))), { timeout: 3_000, message: 'BROWSER_ENCRYPTED_WS: real UI must open an encrypted event channel, rather than relying on HTTP polling for the reply' }).toBe(true);
    expect(channels.some(c => c.frames.some(frame => /ACCOUNT_KEY_HISTORY|SECOND_BROWSER_ENCRYPTED_REPLY/.test(frame)))).toBe(false);
    await second.page.reload();
    await expect(second.page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
    expect(await second.page.getByRole('dialog', { name: UNLOCK_DIALOG }).count()).toBe(0);
    expect(transmitted.filter(r => r.path.endsWith('/login')).every(r => !r.password && r.authKey), 'Account login sends authKey, never password').toBe(true);
    // A certificate signed by another account cannot register on an owner-only device.
    const outsider = await f.account('trust-outsider');
    const foreign = ok(await f.api('/relay/account/browsers', 'GET', undefined, outsider.cookie), 'Outsider certificate').browsers[0].certificate;
    const forged = await browserRequest(owner.page, d1.api + '/transport/browser', 'POST', foreign);
    expect.soft(forged.status, 'FORGED_BROWSER: foreign account certificate cannot command the bound device').toBe(403);
    expect.soft(forged.data.code, 'FORGED_BROWSER: reject certificate authority mismatch').toBe('account_mismatch');

    // Fresh password login decrypts boundDevices before the relay hides the certificate.
    const hidden = await f.login(owner);
    let intercepted = 0;
    await hidden.context.route('**/transport/key?*', async route => {
      try {
        const response = await route.fetch();
        const descriptor = await response.json();
        intercepted++;
        await route.fulfill({ response, json: { ...descriptor, certificate: null, trust: { bound: false, strict: false } } });
      } catch {
        if (!hidden.page.isClosed()) throw new Error('Test descriptor interception failed');
      }
    });
    await hidden.page.goto(d1.url);
    await expect.soft(hidden.page.locator('[data-encryption="identity-changed"]').first(),
      'HIDDEN_CERTIFICATE: vault boundDevices must forbid relay-trust fallback even when descriptor lies').toBeVisible({ timeout: 10_000 });
    expect(intercepted, 'Descriptor downgrade was actually injected').toBeGreaterThan(0);
    expect(await hidden.page.getByRole('button', { name: 'Send Prompt', exact: true }).isEnabled().catch(() => false)).toBe(false);
    await hidden.context.unrouteAll({ behavior: 'ignoreErrors' });
    await hidden.context.close();

    await owner.page.goto(f.base + '/relay-account');
    const secondId = second.browserId;
    const retainedRequest = await retainBrowserRequest(f, second);
    const retainedBefore = await retainedRequest(d1, `/threads/${d1.thread}`, owner.cookie);
    expect(retainedBefore.status, 'Retained real browser key can read before revocation').toBe(200);
    expect(retainedBefore.encrypted, 'Retained-key preflight verifies an encrypted device response').toBe(true);
    expect(retainedBefore.data.thread.id).toBe(d1.thread);
    const row = owner.page.locator('.owner-trust-row').filter({ has: owner.page.locator(`[title="${secondId}"]`) });
    const active = channels.filter(c => !c.closed).at(-1);
    expect.soft(!!active, 'BROWSER_REVOCATION_WS_SETUP: no open secure channel exists before revocation').toBe(true);
    const revokedAt = Date.now();
    owner.page.once('dialog', dialog => dialog.accept());
    await row.getByRole('button', { name: 'Remove browser', exact: true }).click();
    await unlock(owner.page, owner.password);
    await expect(row).toHaveCount(0);
    // Section 11: a browser revocation ends the relay sessions bound to that
    // browser, so the relay refuses it before any device dispatch.
    const removed = await browserRequest(second.page, d1.api + `/threads/${d1.thread}`);
    expect.soft(removed.status, `BROWSER_REVOCATION_RELAY_SESSION: relay ends the removed browser's bound session (code ${removed.data?.code})`).toBe(401);
    if (active) await expect.configure({ soft: true }).poll(() => active.closed, { timeout: 5_000, message: 'BROWSER_REVOCATION_WS: open channel stops within 5 s' }).toBe(true);
    if (active) expect.soft(active.closedAt !== undefined && active.closedAt - revokedAt <= 5_000, 'BROWSER_REVOCATION_WS_DEADLINE: channel closes within 5 s of UI revocation').toBe(true);
    // Enforcement must not depend on the relay ending that session. Give the
    // removed browser a live relay session that is not bound to it (a relay
    // that keeps serving it). The UI may learn removal from the account-signed
    // revocation list before contacting the device and discard the old key.
    await second.context.addCookies(owner.cookies);
    await second.page.goto(d1.url);
    await expect.soft(second.page.getByText(/^This browser was removed(\. Enter your password to reconnect\.)?$/).first(), 'BROWSER_REMOVED_UI: verified account revocation or authenticated device refusal must show the reconnect state').toBeVisible({ timeout: 10_000 });
    // Independently use the retained key for a fresh encrypted, signed request.
    // A live owner relay session prevents relay session invalidation alone from
    // explaining this refusal. Never depend on the UI's first refusal source.
    const retainedAfter = await retainedRequest(d1, `/threads/${d1.thread}`, owner.cookie);
    expect.soft(retainedAfter.encrypted, 'BROWSER_REVOCATION_DEVICE_AUTHENTICATED: old-key refusal must be an authenticated encrypted device response').toBe(true);
    expect.soft(retainedAfter.status, 'BROWSER_REVOCATION: device must refuse the old browser key directly after removal').toBe(401);
    expect.soft(retainedAfter.data?.code, 'BROWSER_REVOCATION_DEVICE_CODE: fresh request signed with the old key must return browser_revoked').toBe('browser_revoked');
    const followup = await browserRequest(second.page, d1.api + `/threads/${d1.thread}`);
    expect.soft(['browser_revoked', 'accountUnlockRequired'].includes(followup.data?.code), `BROWSER_REVOCATION_FOLLOWUP: a removed browser cannot issue a further signed read (code ${followup.data?.code})`).toBe(true);
    expect.soft(await second.page.getByText('ACCOUNT_KEY_HISTORY', { exact: true }).count(), 'BROWSER_REMOVED_HISTORY: removed browser cannot load thread history').toBe(0);
    f.events.push(`browser revoked relay=${removed.status}/${removed.data?.code} directOldKey=${retainedAfter.status}/${retainedAfter.data?.code} directEncrypted=${retainedAfter.encrypted} next=${followup.data?.code} channelClosed=${active?.closed ?? 'no-active-channel'} elapsedMs=${Date.now() - revokedAt}`);

    await owner.page.getByRole('button', { name: 'Change password', exact: true }).click();
    const changing = owner.page.getByRole('dialog', { name: 'Change password', exact: true });
    const newPassword = f.password + '-changed'; f.secrets.push(newPassword);
    await changing.getByLabel('New password', { exact: true }).fill(newPassword);
    await changing.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect(changing).toHaveCount(0);
    owner.password = newPassword;
    await owner.page.goto(d1.url);
    await sendPrompt(owner.page, 'AFTER_PASSWORD_CHANGE');
    const changed = await f.login(owner);
    await changed.page.goto(d1.url);
    await sendPrompt(changed.page, 'NEW_PASSWORD_LOGIN_REPLY');

    const recovery = await f.context();
    await recovery.page.goto(f.base + '/relay-portal?returnTo=%2Frelay-account');
    await recovery.page.getByRole('button', { name: 'Forgot password?', exact: true }).click();
    await recovery.page.getByLabel('Email or username', { exact: true }).fill(owner.username!);
    await recovery.page.getByLabel('Recovery code', { exact: true }).fill(owner.recovery);
    const recoveredPassword = newPassword + '-recovered'; f.secrets.push(recoveredPassword);
    await recovery.page.getByLabel('New password', { exact: true }).fill(recoveredPassword);
    await recovery.page.getByLabel('Confirm password', { exact: true }).fill(recoveredPassword);
    await recovery.page.getByRole('button', { name: 'Reset password', exact: true }).click();
    await expect(recovery.page.getByRole('heading', { name: 'Security', exact: true })).toBeVisible();
    await recovery.page.goto(d1.url);
    await sendPrompt(recovery.page, 'RECOVERY_REWRAPPED_REPLY');
    owner.password = recoveredPassword;
    const recovered = await f.login(owner);
    expect(recovered.accountKeyId).toBe(owner.accountKeyId);

    // Seed the existing legacy SHA256 format in this disposable relay database.
    const legacyPassword = 'short'; f.secrets.push(legacyPassword);
    await f.sql('c.execute("INSERT INTO relay_users (id,email,username,role,enabled,created_at,password_salt,password_hash) VALUES (?,?,?,?,?,?,?,?)",(str(uuid.uuid4()),"legacy@example.test","legacy-user","user",1,datetime.datetime.now(datetime.timezone.utc).isoformat(),"__remote_codex_legacy_sha256__",hashlib.sha256(p["password"].encode()).hexdigest()))', { password: legacyPassword });
    const legacy = await f.context();
    await legacy.page.goto(f.base + '/relay-portal?returnTo=%2Frelay-account');
    const kdf = await f.api('/relay/auth/kdf?login=legacy-user');
    expect.soft(kdf.data.mode, 'KDF_NO_LEGACY_DOWNGRADE: relay must never request automatic plaintext fallback').not.toBe('legacy');
    await legacy.page.getByLabel('Email or username', { exact: true }).fill('legacy-user');
    await legacy.page.getByLabel('Password', { exact: true }).fill(legacyPassword);
    // A normal sign-in derives a key and fails; only then is the explicit upgrade offered.
    await legacy.page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect(legacy.page.getByRole('alert')).toHaveText('Incorrect login or password.');
    const upgrade = legacy.page.getByRole('button', { name: 'Continue with this password', exact: true });
    await expect.soft(upgrade, 'LEGACY_EXPLICIT_UPGRADE: security review requires an explicit legacy choice; /auth/kdf must never trigger plaintext password fallback').toBeVisible();
    const upgradeLogin = legacy.page.waitForRequest(request => new URL(request.url()).pathname === '/relay/auth/login' && request.method() === 'POST' && request.postDataJSON()?.upgrade === true);
    await upgrade.click();
    const upgradeBody = (await upgradeLogin).postDataJSON();
    const explicitProof = upgradeBody?.upgrade === true && upgradeBody?.login === 'legacy-user' && upgradeBody?.password === legacyPassword;
    expect.soft(explicitProof, 'LEGACY_UPGRADE_PROOF: deliberate legacy submit must send {login,password,upgrade:true}').toBe(true);
    await expect(legacy.page.getByRole('heading', { name: 'Secure your account', exact: true })).toBeVisible();
    const passwordInput = legacy.page.getByLabel('Password', { exact: true });
    expect(await passwordInput.getAttribute('minlength'), 'Short legacy password must be replaced').toBe('12');
    await passwordInput.fill(f.password + '-legacy');
    await legacy.page.getByLabel('Confirm password', { exact: true }).fill(f.password + '-legacy');
    await legacy.page.getByRole('button', { name: 'Continue', exact: true }).click();
    const code = await readRecoveryCode(legacy.page); f.secrets.push(code);
    await confirmRecoveryCode(legacy.page, code);
    await expect(legacy.page.getByRole('heading', { name: 'Security', exact: true })).toBeVisible();
    f.events.push('Legacy account upgraded with forced long password and confirmed recovery');

    // Admin reset changes the relay credential but cannot decrypt the vault.
    const adminLogin = await fetch(f.base + '/relay/auth/login', { method: 'POST',
      headers: { 'content-type': 'application/json', origin: f.base },
      body: JSON.stringify({ identifier: 'sharing-admin', password: f.password }) });
    expect(adminLogin.status).toBe(200);
    const adminCookie = adminLogin.headers.get('set-cookie')!.split(';')[0]; f.secrets.push(adminCookie, adminCookie.split('=')[1]);
    const temporaryPassword = f.password + '-temporary'; f.secrets.push(temporaryPassword);
    ok(await f.api(`/relay/admin/users/${owner.userId}/reset-password`, 'POST', { password: temporaryPassword }, adminCookie), 'Admin temporary reset');
    const reset = await f.context();
    await reset.page.goto(f.base + '/relay-portal?returnTo=%2Frelay-account');
    await reset.page.getByRole('button', { name: 'Forgot password?', exact: true }).click();
    await reset.page.getByRole('tab', { name: 'Admin password', exact: true }).click();
    await reset.page.getByLabel('Email or username', { exact: true }).fill(owner.username!);
    await reset.page.getByLabel('Temporary reset password', { exact: true }).fill(temporaryPassword);
    await reset.page.getByRole('button', { name: 'Sign in', exact: true }).click();
    // The account gate asks for the recovery code and a new password before the app opens.
    await expect(reset.page.getByRole('heading', { name: 'Set a new password', exact: true })).toBeVisible();
    await reset.page.getByLabel('Recovery code', { exact: true }).fill(owner.recovery);
    const resetPassword = f.password + '-after-reset'; f.secrets.push(resetPassword);
    await reset.page.getByLabel('New password', { exact: true }).fill(resetPassword);
    await reset.page.getByLabel('Confirm password', { exact: true }).fill(resetPassword);
    await reset.page.getByRole('button', { name: 'Reset password', exact: true }).click();
    await expect(reset.page.getByRole('heading', { name: 'Security', exact: true })).toBeVisible();
    await reset.page.goto(d1.url);
    await sendPrompt(reset.page, 'ADMIN_RESET_RECOVERY_REPLY');
    f.events.push('Admin temporary reset recovered the same vault with recovery code; bound device still works');

  } finally { await f.close(); }
});
