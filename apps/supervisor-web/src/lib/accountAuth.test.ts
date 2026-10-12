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
it('allows plaintext only for legacy login, then requires account setup', async () => {
  state.fetch.mockImplementationOnce(async () => Response.json({ mode: 'legacy' }));
  await auth.startAccountLogin('legacy', 'old password');
  expect(JSON.parse(state.fetch.mock.calls[1]![1].body)).toEqual({ identifier: 'legacy', password: 'old password' });
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
