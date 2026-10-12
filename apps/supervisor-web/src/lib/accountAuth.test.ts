// @vitest-environment node
import { webcrypto } from 'node:crypto';
import { beforeEach, expect, it, vi } from 'vitest';
import vectors from './fixtures/accountKeyVectors.json';
import type { Vault } from './accountVaultState';
const state = vi.hoisted(() => ({ store: new Map<string, unknown>(), fetch: vi.fn(), derive: vi.fn() }));
vi.mock('./accountStore', () => ({ accountStore: async (key: string, value?: unknown) => {
  if (value === null) state.store.delete(key); else if (value !== undefined) state.store.set(key, structuredClone(value));
  return structuredClone(state.store.get(key) ?? null);
} }));
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
  await auth.startAccountLogin('user', 'local'); await auth.finishAccountLogin(v.userId);
  expect((await vault.currentBrowser())!.browserId).not.toBe(old.browserId);
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
