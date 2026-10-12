// @vitest-environment node
import { webcrypto } from 'node:crypto';
import { beforeEach, expect, it, vi } from 'vitest';
import vectors from './fixtures/accountKeyVectors.json';
import type { Vault } from './accountVaultState';
const state = vi.hoisted(() => ({ store: new Map<string, unknown>(), fetch: vi.fn(), derive: vi.fn() }));
vi.mock('./accountStore', () => ({ accountStore: async (key: string, value?: unknown) => {
  if (value === null) state.store.delete(key); else if (value !== undefined) state.store.set(key, structuredClone(value));
  return structuredClone(state.store.get(key) ?? null);
}, hasStoredAccountVault: async () => [...state.store.keys()].some(key => key.startsWith('cache:')) }));
vi.mock('./accountCrypto', async original => ({ ...await original<typeof import('./accountCrypto')>(), passwordKeys: state.derive }));
vi.stubGlobal('crypto', webcrypto); vi.stubGlobal('fetch', state.fetch); vi.stubGlobal('navigator', { userAgent: 'Test browser', language: 'en-US' });
const auth = await import('./accountAuth');
const vault = await import('./accountVaultState');
const c = await import('./accountCrypto');
const v = vectors.vault.vault as Vault;
beforeEach(() => {
  auth.cancelAccountLogin(); state.store.clear(); state.fetch.mockReset(); state.derive.mockReset().mockImplementation(async () => ({ authKey: vectors.derivation.authKey, wrapKey: c.decode64(vectors.derivation.wrapKey) }));
  state.fetch.mockImplementation(async (url: string) => {
    if (url.includes('/kdf?')) return Response.json({ mode: 'account', kdf: v.kdf });
    if (url.endsWith('/vault')) return Response.json(v);
    if (url.endsWith('/browsers')) return Response.json({ browsers: [], revocations: [] });
    if (url.endsWith('/login') || url.endsWith('/recover')) return Response.json({ session: { user: { id: v.userId } } });
    return Response.json({});
  });
});
it('logs in with authKey, opens the vault locally and registers a non-extractable browser key', async () => {
  await auth.startAccountLogin('user', 'password stays local');
  const body = JSON.parse(state.fetch.mock.calls.find(([url]) => url.endsWith('/login'))![1].body);
  expect(body).toEqual({ identifier: 'user', authKey: vectors.derivation.authKey });
  expect(await auth.finishAccountLogin(v.userId)).toEqual({ setup: false, recovery: false });
  const browser = (await vault.currentBrowser())!;
  expect(browser.privateKey.extractable).toBe(false);
  expect(await c.verifyBrowserCertificate(browser.certificate, vectors.account)).toBe(true);
  vault.closeUnlockWindow(); expect(await vault.currentBrowser()).toEqual(browser);
  await vault.logoutAccount(); expect(await vault.currentBrowser()).toBeNull();
});
it('registers a short readable browser label instead of sending the raw user agent', async () => {
  vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36' });
  try {
    await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
    const registration = state.fetch.mock.calls.find(([url, init]) => url.endsWith('/browsers') && init.method === 'POST')!;
    expect(JSON.parse(registration[1].body).label).toBe('Chrome on macOS');
  } finally { vi.stubGlobal('navigator', { userAgent: 'Test browser', language: 'en-US' }); }
});
it('changes the password after a normal-session recovery unlock using the recovery proof', async () => {
  await auth.setAccountSession(v.userId, false);
  await vault.openVault(v, vectors.derivation.recoveryCodeDisplay, true);
  expect(await auth.accountSessionState()).toMatchObject({ recovery: false });
  await vault.changeAccountPassword('a new sufficiently long password');
  const update = JSON.parse(state.fetch.mock.calls.find(([, init]) => init.method === 'PUT')![1].body);
  expect(update.currentRecoveryAuthKey).toBe(vectors.derivation.recoveryAuthKey);
  expect(update).not.toHaveProperty('recoveryAuthKey');
  expect(update.authKey).toBe(vectors.derivation.authKey); expect(update).not.toHaveProperty('currentAuthKey');
  expect(JSON.stringify([...state.store.values()])).not.toContain(vectors.derivation.recoveryAuthKey);
  await vault.changeAccountPassword('another sufficiently long password');
  const next = JSON.parse(state.fetch.mock.calls.filter(([, init]) => init.method === 'PUT').at(-1)![1].body);
  expect(next.currentAuthKey).toBe(vectors.derivation.authKey); expect(next).not.toHaveProperty('recoveryAuthKey');
});
it('expires the recovery proof with the unlock window and uses only the next password proof', async () => {
  await auth.setAccountSession(v.userId, false);
  vi.useFakeTimers();
  try {
    await vault.openVault(v, vectors.derivation.recoveryCodeDisplay, true);
    vi.advanceTimersByTime(vault.UNLOCK_WINDOW_MS);
    expect(vault.isAccountUnlocked()).toBe(false);
    await expect(vault.changeAccountPassword('a sufficiently long password')).rejects.toMatchObject({ code: 'accountUnlockRequired' });
    expect(state.fetch.mock.calls.some(([, init]) => init.method === 'PUT')).toBe(false);
    await vault.openVault(v, 'local password');
    await vault.changeAccountPassword('a sufficiently long new password');
    const update = JSON.parse(state.fetch.mock.calls.find(([, init]) => init.method === 'PUT')![1].body);
    expect(update.currentAuthKey).toBe(vectors.derivation.authKey); expect(update).not.toHaveProperty('recoveryAuthKey');
  } finally { vault.closeUnlockWindow(); vi.useRealTimers(); }
});
it('requires fresh password confirmation for Start over even while unlocked', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  await expect(vault.startAccountOver()).rejects.toMatchObject({ code: 'accountPasswordConfirmationRequired' });
  expect(state.fetch.mock.calls.some(([, init]) => init.method === 'DELETE')).toBe(false);
  expect(await vault.currentAccount()).not.toBeNull();
});
it('preserves the pending derived key through a second-factor challenge', async () => {
  state.fetch.mockImplementationOnce(async () => Response.json({ mode: 'account', kdf: v.kdf }))
    .mockImplementationOnce(async () => Response.json({ challengeRequired: true, authenticator: true }));
  expect(await auth.startAccountLogin('user', 'local password')).toMatchObject({ challengeRequired: true });
  expect(await auth.finishAccountLogin(v.userId)).toMatchObject({ recovery: false });
  expect(state.derive).toHaveBeenCalledTimes(1);
});
it('requires an explicit legacy upgrade, then requires account setup', async () => {
  await auth.startLegacyUpgrade('legacy', 'old password');
  expect(JSON.parse(state.fetch.mock.calls[0]![1].body)).toEqual({ login: 'legacy', password: 'old password', upgrade: true });
  expect(await auth.finishAccountLogin(v.userId)).toEqual({ setup: true, recovery: false });
  expect(state.derive).not.toHaveBeenCalled();
});
it('supports OAuth-only setup without transmitting an authKey', async () => {
  await auth.setAccountSession(v.userId, true);
  expect(await auth.isOAuthAccount()).toBe(true);
  await auth.setupAccount({ vault: v, authKey: 'local-only', recoveryAuthKey: vectors.derivation.recoveryAuthKey, recoveryCode: vectors.derivation.recoveryCodeDisplay }, true);
  const body = JSON.parse(state.fetch.mock.calls.at(-1)![1].body);
  expect(body!.expectedRevision).toBe(0); expect(body).not.toHaveProperty('authKey');
});
it('recovers locally but delays browser registration until the password reset', async () => {
  await auth.startAccountRecovery('user', vectors.derivation.recoveryCodeDisplay);
  expect(await auth.finishAccountLogin(v.userId)).toEqual({ setup: false, recovery: true });
  expect(vault.isAccountUnlocked()).toBe(true); expect(await vault.currentBrowser()).toBeNull();
  expect(state.fetch.mock.calls.some(([url]) => url.endsWith('/browsers'))).toBe(false);
  await vault.changeAccountPassword('a new sufficiently long password'); await vault.registerBrowser();
  const update = JSON.parse(state.fetch.mock.calls.find(([, init]) => init.method === 'PUT')![1].body);
  expect(update.expectedRevision).toBe(v.revision); expect(update).toHaveProperty('authKey');
  expect(update).not.toHaveProperty('currentAuthKey'); expect(await vault.currentBrowser()).not.toBeNull();
});
it('refuses KDF substitution between login and the vault', async () => {
  await auth.startAccountLogin('user', 'local password');
  state.fetch.mockResolvedValueOnce(Response.json({ ...v, kdf: { ...v.kdf, iterations: 4 } }));
  await expect(auth.finishAccountLogin(v.userId)).rejects.toMatchObject({ code: 'accountVaultInvalid' });
  expect(await vault.currentBrowser()).toBeNull();
});
it('keeps the vault closed and browser absent if authentication fails', async () => {
  state.fetch.mockImplementationOnce(async () => Response.json({ mode: 'account', kdf: v.kdf }))
    .mockImplementationOnce(async () => Response.json({ code: 'invalid_credentials' }, { status: 401 }));
  await expect(auth.startAccountLogin('user', 'wrong')).rejects.toMatchObject({ code: 'invalid_credentials' });
  expect(vault.isAccountUnlocked()).toBe(false); expect(await vault.currentBrowser()).toBeNull();
});
it('registration sends only derived credentials and a vault, preserving relay errors for retry', async () => {
  const prepared = { vault: v, authKey: vectors.derivation.authKey, recoveryAuthKey: vectors.derivation.recoveryAuthKey, recoveryCode: vectors.derivation.recoveryCodeDisplay };
  state.fetch.mockResolvedValueOnce(Response.json({ code: 'registration_password_invalid' }, { status: 403 }));
  await expect(auth.registerAccount({ email: 'test@example.invalid', username: 'tester', registrationPassword: 'wrong invite' }, prepared)).rejects.toMatchObject({ code: 'registration_password_invalid' });
  await auth.registerAccount({ email: 'test@example.invalid', username: 'tester', registrationPassword: 'right invite' }, prepared);
  const body = JSON.parse(state.fetch.mock.calls.at(-1)![1].body);
  expect(body.vault).toEqual(v); expect(body).not.toHaveProperty('password'); expect(body).not.toHaveProperty('recoveryCode');
});
it('uses compare-and-swap for bound device updates and refuses conflicting revisions', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  const account = await vault.requireUnlock('bind-device');
  state.fetch.mockResolvedValueOnce(Response.json({ code: 'vault_conflict' }, { status: 409 }));
  await expect(account.updateBoundDevices(ids => [...ids, 'new-device'])).rejects.toMatchObject({ code: 'vault_conflict' });
  expect(await vault.boundDevices()).not.toContain('new-device');
  await account.updateBoundDevices(ids => [...ids, 'new-device']);
  expect(await vault.boundDevices()).toContain('new-device');
});
it('replaces a relay-revoked browser with a new key after password authentication', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  const old = (await vault.currentBrowser())!;
  const fields = { accountKeyId: v.accountKeyId, kind: 'browser' as const, subjectId: old.browserId, revokedAt: Date.now() };
  const revoke = { version: 1, ...fields, signature: await c.sign(await c.importPrivate(vectors.account.accountPrivateKeyPkcs8), c.inputs.revocation(fields)) };
  state.fetch.mockImplementation(async (url: string) => {
    if (url.includes('/kdf?')) return Response.json({ mode: 'account', kdf: v.kdf });
    if (url.endsWith('/vault')) return Response.json(v);
    if (url.endsWith('/browsers')) return Response.json({ browsers: [], revocations: [revoke] });
    return Response.json({});
  });
  // This suite runs without a DOM; give the module an event target to report to.
  const events = new EventTarget(), removed = vi.fn(); events.addEventListener(vault.BROWSER_REMOVED, removed);
  const g = globalThis as { window?: unknown }; g.window = events;
  try { await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId); }
  finally { delete g.window; }
  expect((await vault.currentBrowser())!.browserId).not.toBe(old.browserId);
  // The page explains the removal instead of a generic unlock prompt.
  expect(removed).toHaveBeenCalledTimes(1);
});
it('keeps revocation tombstones when the relay later omits them', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  state.fetch.mockResolvedValueOnce(Response.json({ browsers: [], revocations: [vectors.revocations.browser] }));
  expect(await vault.accountRevocations(true)).toContainEqual(vectors.revocations.browser);
  expect(await vault.accountRevocations(true)).toContainEqual(vectors.revocations.browser);
});
it('expires pending second-factor derivations rather than keeping them indefinitely', async () => {
  await auth.startAccountLogin('user', 'local'); vi.useFakeTimers(); vi.advanceTimersByTime(10 * 60 * 1000);
  await expect(auth.finishAccountLogin(v.userId)).rejects.toMatchObject({ code: 'accountUnlockExpired' }); vi.useRealTimers();
});
it('reauthenticates v2 security actions with authKey and keeps OAuth unlock passwords local', async () => {
  await auth.setAccountSession(v.userId, false);
  expect(await auth.accountPasswordProof('local password')).toEqual({ authKey: vectors.derivation.authKey });
  await auth.setAccountSession(v.userId, true);
  await expect(auth.accountPasswordProof('local OAuth password')).rejects.toMatchObject({ code: 'accountOAuthFactorRequired' });
});
it('regenerates recovery authentication with currentAuthKey while preserving account identity and bound devices', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  const before = await vault.boundDevices();
  await vault.regenerateRecoveryCode();
  const update = JSON.parse(state.fetch.mock.calls.find(([, init]) => init.method === 'PUT')![1].body);
  expect(update.currentAuthKey).toBe(vectors.derivation.authKey); expect(update).toHaveProperty('recoveryAuthKey');
  expect(update.vault.accountKeyId).toBe(v.accountKeyId); expect(await vault.boundDevices()).toEqual(before);
});
it('keeps OAuth password changes and later recovery-code changes free of relay auth keys', async () => {
  await auth.setAccountSession(v.userId, true);
  await vault.openVault(v, 'local password');
  await vault.changeAccountPassword('a new sufficiently long password', true);
  await vault.regenerateRecoveryCode();
  const updates = state.fetch.mock.calls.filter(([, init]) => init.method === 'PUT').map(([, init]) => JSON.parse(init.body));
  expect(updates).toHaveLength(2);
  for (const update of updates) {
    expect(update).not.toHaveProperty('authKey');
    expect(update).not.toHaveProperty('currentAuthKey');
  }
  expect(updates[1]).toHaveProperty('recoveryAuthKey');
});
it('persists the authoritative account session state and refuses a changed account key hint', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  await auth.setAccountSession(v.userId, { mode: 'oauth', setupRequired: false, recovery: false, hasVault: true, accountKeyId: v.accountKeyId });
  expect(await auth.isOAuthAccount()).toBe(true); expect(await auth.accountSessionState()).toMatchObject({ mode: 'oauth', hasVault: true });
  await expect(auth.setAccountSession(v.userId, { mode: 'account', setupRequired: false, recovery: false, hasVault: true, accountKeyId: vectors.otherAccount.accountKeyId })).rejects.toMatchObject({ code: 'accountKeyChanged' });
});
it('blocks a relay downgrade to legacy for a previously verified login identifier', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  state.fetch.mockResolvedValueOnce(Response.json({ mode: 'legacy' }));
  const before = state.fetch.mock.calls.length;
  await expect(auth.startAccountLogin('USER', 'local')).rejects.toMatchObject({ code: 'accountLoginDowngrade' });
  expect(state.fetch.mock.calls.slice(before)).toHaveLength(1);
});
it('uses an explicit temporary reset login then requires recovery rather than creating a replacement vault', async () => {
  await auth.startTemporaryResetLogin('user', 'temporary password from administrator');
  expect(JSON.parse(state.fetch.mock.calls.at(-1)![1].body)).toEqual({ identifier: 'user', password: 'temporary password from administrator' });
  await auth.setAccountSession(v.userId, { mode: 'account', setupRequired: false, recovery: true, hasVault: true, accountKeyId: v.accountKeyId });
  expect(await auth.finishAccountLogin(v.userId)).toEqual({ setup: false, recovery: true }); expect(vault.isAccountUnlocked()).toBe(false);
  await vault.startAccountOver();
  expect(state.fetch.mock.calls.find(([, init]) => init.method === 'DELETE')![1].body).toBe('{}'); expect(await vault.currentAccount()).toBeNull();
});
it('registers using the exact chosen top-level userId and preserves a collision for a new attempt', async () => {
  const prepared = { vault: v, authKey: vectors.derivation.authKey, recoveryAuthKey: vectors.derivation.recoveryAuthKey, recoveryCode: vectors.derivation.recoveryCodeDisplay };
  state.fetch.mockResolvedValueOnce(Response.json({ code: 'user_id_taken' }, { status: 409 }));
  await expect(auth.registerAccount({ username: 'tester', email: 'test@example.invalid' }, prepared)).rejects.toMatchObject({ code: 'user_id_taken' });
  const body = JSON.parse(state.fetch.mock.calls.at(-1)![1].body); expect(body.userId).toBe(body.vault.userId);
});

it('never sends a fresh browser password because a relay claims legacy mode', async () => {
  state.fetch.mockResolvedValueOnce(Response.json({ mode: 'legacy' }));
  await expect(auth.startAccountLogin('fresh', 'must stay local')).rejects.toMatchObject({ code: 'accountLoginDowngrade' });
  expect(state.fetch).toHaveBeenCalledTimes(1);
});
it('blocks explicit upgrades for known vault identifiers and when the migration build flag is off', async () => {
  await auth.startAccountLogin('known', 'local'); await auth.finishAccountLogin(v.userId);
  state.fetch.mockClear();
  await expect(auth.startLegacyUpgrade('KNOWN', 'must stay local')).rejects.toMatchObject({ code: 'accountLoginDowngrade' });
  expect(state.fetch).not.toHaveBeenCalled();
  vi.stubEnv('VITE_POCKYMOE_LEGACY_UPGRADE', '0');
  try {
    expect(auth.legacyUpgradeEnabled()).toBe(false);
    await expect(auth.startLegacyUpgrade('fresh', 'must stay local')).rejects.toMatchObject({ code: 'accountLoginDowngrade' });
    expect(state.fetch).not.toHaveBeenCalled();
  } finally { vi.unstubAllEnvs(); }
});
it('caches both session username and email after verifying a vault, even when sign-in used an alias', async () => {
  await auth.startAccountLogin('typed alias', 'local');
  await auth.finishAccountLogin(v.userId, { username: 'CanonicalUser', email: 'Account@Example.invalid' });
  for (const login of ['CANONICALUSER', 'account@example.invalid']) {
    state.fetch.mockClear();
    expect(await auth.legacyUpgradeOffer(login)).toBeNull();
    await expect(auth.startLegacyUpgrade(login, 'must stay local', true)).rejects.toMatchObject({ code: 'accountLoginDowngrade' });
    expect(state.fetch).not.toHaveBeenCalled();
  }
});
it('requires an explicit warning confirmation before upgrading another account when any vault was opened', async () => {
  await auth.startAccountLogin('known', 'local'); await auth.finishAccountLogin(v.userId);
  await vault.logoutAccount(); state.fetch.mockClear();
  expect(await auth.legacyUpgradeOffer('unrecognized')).toEqual({ otherVault: true });
  await expect(auth.startLegacyUpgrade('other legacy', 'must stay local')).rejects.toMatchObject({ code: 'accountUpgradeConfirmationRequired' });
  expect(state.fetch).not.toHaveBeenCalled();
  await auth.startLegacyUpgrade('other legacy', 'explicit old password', true);
  expect(JSON.parse(state.fetch.mock.calls[0]![1].body)).toEqual({ login: 'other legacy', password: 'explicit old password', upgrade: true });
});
it('keeps warning about the upgrade after Start over clears the last cached vault', async () => {
  await vault.openVault(v, 'local');
  state.store.delete(`cache:${v.userId}`);
  expect(await auth.legacyUpgradeOffer('new spelling')).toEqual({ otherVault: true });
});
it('posts the existing browser certificate after OAuth login without reopening the key window', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  const browser = await vault.currentBrowser(); vault.closeUnlockWindow();
  await auth.setAccountSession(v.userId, true); state.fetch.mockClear();
  await auth.finishAccountLogin(v.userId);
  const posts = state.fetch.mock.calls.filter(([url, init]) => url.endsWith('/browsers') && init.method === 'POST');
  expect(posts).toHaveLength(1); expect(JSON.parse(posts[0]![1].body).certificate).toEqual(browser!.certificate);
  expect(vault.isAccountUnlocked()).toBe(false); expect(await vault.currentBrowser()).toEqual(browser);
});
it('does not restore a browser key if sign-out happens during an idempotent certificate POST', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  vault.closeUnlockWindow();
  const original = state.fetch.getMockImplementation()!;
  state.fetch.mockImplementation(async (url, init) => {
    if (url.endsWith('/browsers') && init.method === 'POST') { await vault.logoutAccount(); return Response.json({}); }
    return original(url, init);
  });
  await expect(vault.registerBrowser()).rejects.toMatchObject({ code: 'accountUnlockRequired' });
  expect(await vault.currentBrowser()).toBeNull();
});
it('refuses plaintext reauthentication when no account context is locally known', async () => {
  await expect(auth.accountPasswordProof('must stay local')).rejects.toMatchObject({ code: 'accountUnlockRequired' });
  expect(state.fetch).not.toHaveBeenCalled();
});
it('ignores foreign-key and invalid revocations while retaining every valid current-key tombstone', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  state.fetch.mockResolvedValueOnce(Response.json({ browsers: [], revocations: [
    { ...vectors.revocations.browser, accountKeyId: vectors.otherAccount.accountKeyId },
    { ...vectors.revocations.device, signature: 'broken' }, vectors.revocations.browser,
  ] }));
  expect(await vault.accountRevocations(true)).toEqual([vectors.revocations.browser]);
});
it('pins migrated device identities and requires unlocked explicit confirmation for a replacement', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  const id = vectors.deviceCertificate.certificate.deviceId, identityKey = vectors.deviceCertificate.certificate.identityKey;
  await vault.confirmBoundDeviceIdentity(id, identityKey, (await vault.boundDeviceIdentities()).find(d => d.deviceId === id)?.identityKey ?? null);
  expect(await vault.boundDeviceIdentities()).toContainEqual({ deviceId: id, identityKey });
  const update = JSON.parse(state.fetch.mock.calls.filter(([, init]) => init.method === 'PUT').at(-1)![1].body);
  expect(update.currentAuthKey).toBe(vectors.derivation.authKey);
  const data = await vault.decryptVault(update.vault, c.decode64(vectors.derivation.wrapKey), 'password', v.userId);
  expect(data.body.boundDevices.every((entry: unknown) => typeof entry === 'object')).toBe(true);
  vault.closeUnlockWindow();
  await expect(vault.confirmBoundDeviceIdentity(id, vectors.negative.deviceCertWrongIdentity.identityKey)).rejects.toMatchObject({ code: 'accountUnlockRequired' });
});

it('uses local vault state even when reauthentication comes from the admin route', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  expect(await auth.accountPasswordProof('must stay local', true)).toEqual({ authKey: vectors.derivation.authKey });
});
it('migrates string device IDs from an authenticated local certificate on read', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  const cache = state.store.get(`cache:${v.userId}`) as Record<string, unknown>;
  const cert = vectors.deviceCertificate.certificate;
  state.store.set(`cache:${v.userId}`, { ...cache, boundDevices: [cert.deviceId] });
  state.store.set(`device-certificate:${v.accountKeyId}:${cert.deviceId}`, cert);
  expect(await vault.boundDeviceIdentities()).toEqual([{ deviceId: cert.deviceId, identityKey: cert.identityKey }]);
  expect(await vault.boundDevices()).toEqual([cert.deviceId]);
});

it('rejects stale identity confirmations and requires an explicit previous snapshot for replacements', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  const id = vectors.deviceCertificate.certificate.deviceId, original = vectors.deviceCertificate.certificate.identityKey;
  await vault.confirmBoundDeviceIdentity(id, original, (await vault.boundDeviceIdentities()).find(d => d.deviceId === id)?.identityKey ?? null);
  const replacement = vectors.negative.deviceCertWrongIdentity.identityKey;
  await expect(vault.confirmBoundDeviceIdentity(id, replacement)).rejects.toMatchObject({ code: 'accountDeviceIdentityChanged' });
  await expect(vault.confirmBoundDeviceIdentity(id, replacement, null)).rejects.toMatchObject({ code: 'accountDeviceIdentityChanged' });
  await vault.confirmBoundDeviceIdentity(id, replacement, original);
  await expect(vault.confirmBoundDeviceIdentity(id, original, original)).rejects.toMatchObject({ code: 'accountDeviceIdentityChanged' });
  expect(await vault.boundDeviceIdentities()).toContainEqual({ deviceId: id, identityKey: replacement });
});

it('uses the recovery proof for bound-device writes and explicit identity replacement', async () => {
  await auth.setAccountSession(v.userId, false);
  await vault.openVault(v, vectors.derivation.recoveryCodeDisplay, true);
  const account = await vault.requireUnlock('bind-device');
  await account.updateBoundDevices(ids => [...ids, 'recovery-device']);
  await vault.confirmBoundDeviceIdentity('recovery-device', vectors.deviceCertificate.certificate.identityKey, '');
  const replacement = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  await vault.confirmBoundDeviceIdentity('recovery-device', c.encode64(await crypto.subtle.exportKey('raw', replacement.publicKey)), vectors.deviceCertificate.certificate.identityKey);
  const updates = state.fetch.mock.calls.filter(([, init]) => init.method === 'PUT').map(([, init]) => JSON.parse(init.body));
  expect(updates).toHaveLength(3);
  for (const update of updates) {
    expect(update.currentRecoveryAuthKey).toBe(vectors.derivation.recoveryAuthKey);
    expect(update).not.toHaveProperty('currentAuthKey');
    expect(update).not.toHaveProperty('recoveryAuthKey');
  }
  expect(JSON.stringify([...state.store.values()])).not.toContain(vectors.derivation.recoveryAuthKey);
});
it('regenerates a recovery code with its old proof and uses the new proof for subsequent writes', async () => {
  await auth.setAccountSession(v.userId, false);
  await vault.openVault(v, vectors.derivation.recoveryCodeDisplay, true);
  const account = await vault.requireUnlock('bind-device');
  const code = await vault.regenerateRecoveryCode();
  const keys = await c.recoveryKeys(code);
  const update = JSON.parse(state.fetch.mock.calls.find(([, init]) => init.method === 'PUT')![1].body);
  expect(update.currentRecoveryAuthKey).toBe(vectors.derivation.recoveryAuthKey);
  expect(update.recoveryAuthKey).toBe(keys.recoveryAuthKey);
  expect(update.recoveryAuthKey).not.toBe(update.currentRecoveryAuthKey);
  await account.updateBoundDevices(ids => [...ids, 'after-regeneration']);
  const next = JSON.parse(state.fetch.mock.calls.filter(([, init]) => init.method === 'PUT').at(-1)![1].body);
  expect(next.currentRecoveryAuthKey).toBe(keys.recoveryAuthKey);
  expect(next).not.toHaveProperty('recoveryAuthKey');
  keys.recoveryKey.fill(0);
});
it('retains the old recovery proof if regeneration fails', async () => {
  await auth.setAccountSession(v.userId, false);
  await vault.openVault(v, vectors.derivation.recoveryCodeDisplay, true);
  state.fetch.mockResolvedValueOnce(Response.json({ code: 'vault_conflict' }, { status: 409 }));
  await expect(vault.regenerateRecoveryCode()).rejects.toMatchObject({ code: 'vault_conflict' });
  await (await vault.requireUnlock('bind-device')).updateBoundDevices(ids => [...ids, 'after-failure']);
  const next = JSON.parse(state.fetch.mock.calls.filter(([, init]) => init.method === 'PUT').at(-1)![1].body);
  expect(next.currentRecoveryAuthKey).toBe(vectors.derivation.recoveryAuthKey);
});
it('starts over in a normal session only after locally verifying the confirmed password', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  await vault.startAccountOver('confirmed password');
  expect(state.derive).toHaveBeenLastCalledWith('confirmed password', v.kdf);
  const deletion = state.fetch.mock.calls.find(([, init]) => init.method === 'DELETE')!;
  expect(JSON.parse(deletion[1].body)).toEqual({ currentAuthKey: vectors.derivation.authKey });
  expect(await vault.currentAccount()).toBeNull(); expect(await vault.currentBrowser()).toBeNull();
  expect(state.store.has(`cache:${v.userId}`)).toBe(false);
});
it('does not delete a vault or browser when the confirmed password is wrong', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  state.derive.mockResolvedValueOnce({ authKey: 'wrong', wrapKey: new Uint8Array(32) });
  await expect(vault.startAccountOver('wrong password')).rejects.toThrow();
  expect(state.fetch.mock.calls.some(([, init]) => init.method === 'DELETE')).toBe(false);
  expect(await vault.currentBrowser()).not.toBeNull();
});
it('keeps an OAuth-only Start over password local while verifying it before deletion', async () => {
  await auth.setAccountSession(v.userId, true); await vault.openVault(v, 'local');
  await vault.startAccountOver('confirmed local password');
  expect(state.derive).toHaveBeenLastCalledWith('confirmed local password', v.kdf);
  const deletion = state.fetch.mock.calls.find(([, init]) => init.method === 'DELETE')!;
  expect(JSON.parse(deletion[1].body)).toEqual({});
});

it('preserves the browser and vault if password-confirmed Start over is denied by the relay', async () => {
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  state.fetch.mockImplementation(async (url: string, init: RequestInit) => init.method === 'DELETE'
    ? Response.json({ code: 'reauthentication_required' }, { status: 403 }) : Response.json(v));
  await expect(vault.startAccountOver('confirmed password')).rejects.toMatchObject({ code: 'reauthentication_required' });
  expect(await vault.currentBrowser()).not.toBeNull(); expect(await vault.currentAccount()).not.toBeNull();
  expect(state.store.has(`cache:${v.userId}`)).toBe(true);
});
