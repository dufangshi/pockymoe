import { test, expect } from '@playwright/test';
import { fixture, browserRequest, ok, sendPrompt, sockets, screenshot, unlock } from './helpers/account-key-fixture';

// Passwords, recovery codes, binding commands and relay cookies stay in memory.
test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

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
      'BOUND_DEVICE_IDENTITY: vault cache must preserve {deviceId,identityKey}, rather than deviceId alone. Security-review integration fix pending.').toBe(true);
    const strict = await f.api(d1.api + `/threads/${d1.thread}`, 'GET', undefined, owner.cookie);
    expect(strict.status).toBe(403); expect(strict.data.code).toBe('device_strict_mode');
    await sendPrompt(owner.page, 'ACCOUNT_KEY_HISTORY');
    const second = await f.login(owner);
    const channels = sockets(second.page);
    await second.page.addInitScript(() => {
      (window as any).__accountTrustEvents = [];
      window.addEventListener('remote-codex-transport', event => {
        const status = (event as CustomEvent).detail;
        if (status?.unlockReason) (window as any).__accountTrustEvents.push(status.unlockReason);
      });
    });
    await second.page.goto(d1.url);
    await expect(second.page.getByText('ACCOUNT_KEY_HISTORY', { exact: true })).toBeVisible();
    await sendPrompt(second.page, 'SECOND_BROWSER_ENCRYPTED_REPLY');
    await expect.configure({ soft: true }).poll(() => channels.some(c => c.frames.some(frame => frame.includes('"encrypted"'))), { timeout: 3_000, message: 'BROWSER_ENCRYPTED_WS: real UI must open an encrypted event channel, rather than relying on HTTP polling for the reply' }).toBe(true);
    expect(channels.some(c => c.frames.some(frame => /ACCOUNT_KEY_HISTORY|SECOND_BROWSER_ENCRYPTED_REPLY/.test(frame)))).toBe(false);
    await second.page.reload();
    await expect(second.page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
    expect(await second.page.getByRole('dialog', { name: 'Unlock this browser', exact: true }).count()).toBe(0);
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
    const row = owner.page.locator('.owner-trust-row').filter({ has: owner.page.locator(`[title="${secondId}"]`) });
    const active = channels.filter(c => !c.closed).at(-1);
    expect.soft(!!active, 'BROWSER_REVOCATION_WS_SETUP: no open secure channel exists before revocation').toBe(true);
    const revokedAt = Date.now();
    owner.page.once('dialog', dialog => dialog.accept());
    await row.getByRole('button', { name: 'Remove browser', exact: true }).click();
    await unlock(owner.page, owner.password);
    await expect(row).toHaveCount(0);
    const removed = await browserRequest(second.page, d1.api + `/threads/${d1.thread}`);
    // The UI can send the first revoked request before this explicit fetch;
    // after authenticating browser_revoked it deliberately deletes its key.
    await expect.configure({ soft: true }).poll(() => second.page.evaluate(() => (window as any).__accountTrustEvents.includes('browser_revoked')),
      { timeout: 5_000, message: 'BROWSER_REVOCATION: first authenticated device refusal must be browser_revoked' }).toBe(true);
    expect.soft(['browser_revoked', 'accountUnlockRequired'].includes(removed.data.code), 'BROWSER_REVOCATION_FOLLOWUP: a removed browser cannot issue a further signed read').toBe(true);
    await expect.soft(second.page.getByText('This browser was removed. Enter your password to reconnect.', { exact: true }).first(), 'BROWSER_REMOVED_UI: revoked certificate has a visible reconnect state').toBeVisible({ timeout: 5_000 });
    if (active) await expect.configure({ soft: true }).poll(() => active.closed, { timeout: 5_000, message: 'BROWSER_REVOCATION_WS: open channel stops within 5 s' }).toBe(true);
    if (active) expect.soft(active.closedAt !== undefined && active.closedAt - revokedAt <= 5_000, 'BROWSER_REVOCATION_WS_DEADLINE: channel closes within 5 s of UI revocation').toBe(true);
    f.events.push(`browser revoked authenticatedReason=browser_revoked next=${removed.data.code} channelClosed=${active?.closed ?? 'no-active-channel'} elapsedMs=${Date.now() - revokedAt}`);

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
    await recovery.page.getByRole('button', { name: 'Forgot password? Use your account recovery code', exact: true }).click();
    await recovery.page.getByLabel('Email or username', { exact: true }).fill(owner.username!);
    await recovery.page.getByLabel('Account recovery code', { exact: true }).fill(owner.recovery);
    const recoveredPassword = newPassword + '-recovered'; f.secrets.push(recoveredPassword);
    await recovery.page.getByLabel('New password', { exact: true }).fill(recoveredPassword);
    await recovery.page.getByRole('button', { name: 'Recover account', exact: true }).click();
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
    const upgrade = legacy.page.getByRole('button', { name: 'First sign-in since the security upgrade', exact: true });
    expect.soft(await upgrade.count() === 1, 'LEGACY_EXPLICIT_UPGRADE: security review requires an explicit legacy choice; /auth/kdf must never trigger plaintext password fallback. Pending integration fix.').toBe(true);
    if (await upgrade.count()) await upgrade.click();
    const kdf = await f.api('/relay/auth/kdf?login=legacy-user');
    expect.soft(kdf.data.mode, 'KDF_NO_LEGACY_DOWNGRADE: relay must never request automatic plaintext fallback. Pending integration fix.').not.toBe('legacy');
    let explicitProof = false;
    legacy.page.on('request', request => {
      if (new URL(request.url()).pathname !== '/relay/auth/login' || !request.postData()) return;
      const body = request.postDataJSON(); explicitProof = body.upgrade === true && body.login === 'legacy-user' && body.password === legacyPassword;
    });
    await legacy.page.getByLabel('Email or username', { exact: true }).fill('legacy-user');
    await legacy.page.getByLabel('Password', { exact: true }).fill(legacyPassword);
    await legacy.page.locator('form button[type="submit"]').click();
    expect.soft(explicitProof, 'LEGACY_UPGRADE_PROOF: deliberate legacy submit must send {login,password,upgrade:true}. Pending integration fix.').toBe(true);
    const secure = legacy.page.getByRole('dialog', { name: 'Secure your account', exact: true });
    await expect(secure).toBeVisible();
    const passwordInput = secure.getByLabel('Password', { exact: true });
    expect(await passwordInput.getAttribute('minlength'), 'Short legacy password must be replaced').toBe('12');
    await passwordInput.fill(f.password + '-legacy');
    await secure.getByRole('button', { name: 'Secure your account', exact: true }).click();
    const confirm = secure.getByLabel('Re-type the last group to confirm you saved it', { exact: true });
    await expect(confirm).toBeVisible();
    const code = (await secure.locator('code').textContent())!.trim(); f.secrets.push(code);
    await confirm.fill(code.slice(-4));
    await secure.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect(secure).toHaveCount(0);
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
    await reset.page.getByRole('button', { name: 'Use an admin reset password', exact: true }).click();
    await reset.page.getByLabel('Email or username', { exact: true }).fill(owner.username!);
    await reset.page.getByLabel('Temporary reset password', { exact: true }).fill(temporaryPassword);
    await reset.page.getByRole('button', { name: 'Sign in', exact: true }).click();
    const resetUnlock = reset.page.getByRole('dialog', { name: 'Unlock this browser', exact: true });
    await expect(resetUnlock.getByLabel('Account recovery code', { exact: true })).toBeVisible();
    await resetUnlock.getByLabel('Account recovery code', { exact: true }).fill(owner.recovery);
    const resetPassword = f.password + '-after-reset'; f.secrets.push(resetPassword);
    await resetUnlock.getByLabel('New password', { exact: true }).fill(resetPassword);
    await resetUnlock.getByRole('button', { name: 'Unlock', exact: true }).click();
    await expect(resetUnlock).toHaveCount(0);
    await reset.page.goto(d1.url);
    await sendPrompt(reset.page, 'ADMIN_RESET_RECOVERY_REPLY');
    f.events.push('Admin temporary reset recovered the same vault with recovery code; bound device still works');

  } finally { await f.close(); }
});
