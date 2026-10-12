// @vitest-environment node
import { webcrypto } from 'node:crypto';
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { runInNewContext } from 'node:vm';
import { beforeEach, expect, it, vi } from 'vitest';
import vectors from './fixtures/accountKeyVectors.json';
import * as c from './accountCrypto';
import * as vault from './accountVaultState';
import type { BrowserCertificate, DeviceCertificate, Revocation } from './accountVault';
const data = vi.hoisted(() => new Map<string, unknown>());
vi.mock('./accountStore', () => ({ accountStore: async (key: string, value?: unknown) => {
  if (value === null) data.delete(key); else if (value !== undefined) data.set(key, structuredClone(value));
  return structuredClone(data.get(key) ?? null);
} }));
vi.stubGlobal('crypto', webcrypto);
const a = vectors.account;
const v = vectors.vault.vault as vault.Vault;
beforeEach(() => { vault.closeUnlockWindow(); data.clear(); vi.useRealTimers(); });
it('matches all derivation, recovery and safety code vectors', async () => {
  const d = vectors.derivation;
  expect(c.encode64(await c.hkdf(c.decode64(d.master), 'pockymoe-relay-auth-v1'))).toBe(d.authKey);
  expect(c.encode64(await c.hkdf(c.decode64(d.master), 'pockymoe-vault-wrap-v1'))).toBe(d.wrapKey);
  expect(c.encode64(c.parseRecovery(d.recoveryCodeDisplay))).toBe(d.recoveryCodeBytes);
  expect(c.recoveryDisplay(c.parseRecovery(d.recoveryCode))).toBe(d.recoveryCodeDisplay);
  const keys = await c.recoveryKeys(d.recoveryCode);
  expect(c.encode64(keys.recoveryKey)).toBe(d.recoveryKey); expect(keys.recoveryAuthKey).toBe(d.recoveryAuthKey);
  expect(await vault.safetyCode(vectors.safetyCode.accountKeyId)).toBe(vectors.safetyCode.display);
  expect(c.base32(await c.hash(c.utf8(`pockymoe-acct-safety-v1\n${a.accountKeyId}`))).slice(0, 12)).toBe(vectors.safetyCode.code);
});
it('verifies browser/device certificates and both revocation vectors', async () => {
  const b = vectors.browserCertificate.certificate as BrowserCertificate, d = vectors.deviceCertificate.certificate as DeviceCertificate;
  expect(c.inputs.browser(b)).toBe(vectors.browserCertificate.signingInput);
  expect(c.inputs.device(d)).toBe(vectors.deviceCertificate.signingInput);
  expect(await c.verifyBrowserCertificate(b, a)).toBe(true);
  expect(await vault.verifyDeviceCertificate(d, a, d.deviceId, d.identityKey)).toBe(true);
  for (const entry of Object.values(vectors.revocations)) {
    expect(await c.verifyRevocation(entry as Revocation, a)).toBe(true);
  }
});
it('opens fixed password and recovery slots and validates the account private key', async () => {
  expect(vault.slotAad(v, 'password')).toBe(vectors.vault.slotAadPassword);
  expect(vault.bodyAad(v)).toBe(vectors.vault.bodyAad);
  for (const [slot, key] of [['password', vectors.derivation.wrapKey], ['recovery', vectors.derivation.recoveryKey]] as const) {
    const result = await vault.decryptVault(v, c.decode64(key), slot, a.userId);
    expect(c.encode64(result.vdk)).toBe(vectors.vault.vdk);
    const expected = JSON.parse(vectors.vault.bodyPlaintext);
    expect(result.body).toEqual({ ...expected, boundDevices: expected.boundDevices.map((deviceId: string) => ({ deviceId, identityKey: '' })) });
    expect(result.key.extractable).toBe(false);
  }
});
it('matches enrollment MAC, request signatures and peer signatures', async () => {
  const d = vectors.deviceCertificate.certificate;
  expect(await c.enrollmentMac(vectors.enrollment.enrollSecret, d.deviceId, d.identityKey, a.accountKeyId)).toBe(vectors.enrollment.mac);
  const r = vectors.requestSignature;
  expect(await c.requestSigningInput(r.aad, c.decode64(r.sealed))).toBe(r.signingInput);
  expect(r.sessionHeader).toBe(vectors.browserCertificate.certificate.browserId);
  expect(await c.verify(vectors.browserCertificate.certificate.publicKey, r.signature, r.signingInput)).toBe(true);
  const signature = await c.signRequest(await c.importPrivate(vectors.browserCertificate.browserPrivateKeyPkcs8), r.aad, c.decode64(r.sealed));
  expect(await c.verify(vectors.browserCertificate.certificate.publicKey, signature, r.signingInput)).toBe(true);
  const p = vectors.peerSignature;
  expect(p.signingInput).toBe(`pockymoe-peer-request-v1\n${p.callerDeviceId}\n${p.aad}\n${await c.hash64(c.decode64(p.sealed))}`);
  const identitySpki = await crypto.subtle.exportKey('spki', await crypto.subtle.importKey('raw', c.bytes(c.decode64(d.identityKey)), { name: 'ECDSA', namedCurve: 'P-256' }, true, ['verify']));
  expect(await c.verify(c.encode64(identitySpki), p.signature, p.signingInput)).toBe(true);
});
it('rejects EVERY negative interoperability vector', async () => {
  const n = vectors.negative, d = vectors.deviceCertificate.certificate;
  for (const cert of [n.browserCertSignedByOtherAccount, n.browserCertWrongBrowserId, n.browserCertTamperedCreatedAt])
    expect(await c.verifyBrowserCertificate(cert as BrowserCertificate, a)).toBe(false);
  for (const cert of [n.deviceCertWrongDevice, n.deviceCertWrongIdentity])
    expect(await vault.verifyDeviceCertificate(cert as DeviceCertificate, a, d.deviceId, d.identityKey)).toBe(false);
  expect(await c.verifyRevocation(n.revocationWrongKind as Revocation, a)).toBe(false);
  expect(c.constantEqual(vectors.enrollment.mac, n.enrollmentMacWrongIdentity)).toBe(false);
  await expect(c.open(c.decode64(vectors.vault.vdk), v.body, n.vaultBodyWrongRevisionAad)).rejects.toThrow();
});
it('rejects weak KDF parameters before allocating or deriving', async () => {
  for (const patch of [{ memoryKiB: 65535 }, { iterations: 2 }, { parallelism: 0 }, { salt: '' }, { iterations: 3.5 }])
    await expect(c.deriveMaster('any password', { ...v.kdf, ...patch })).rejects.toThrow();
  await expect(vault.createVault('new-user', 'short')).rejects.toMatchObject({ code: 'accountPasswordShort' });
});
it('refuses rollback and an account key replacement even if the new hash matches', async () => {
  const decrypted = await vault.decryptVault(v, c.decode64(vectors.derivation.wrapKey), 'password', a.userId);
  await vault.installUnlockedVault(v, decrypted);
  await expect(vault.checkVault({ ...v, revision: v.revision - 1 }, a.userId)).rejects.toMatchObject({ code: 'accountVaultRollback' });
  await expect(vault.checkVault({ ...v, ...vectors.otherAccount }, a.userId)).rejects.toMatchObject({ code: 'accountKeyChanged' });
});
it('expires account handles at ten minutes while keeping browser keys usable', async () => {
  const decrypted = await vault.decryptVault(v, c.decode64(vectors.derivation.wrapKey), 'password', a.userId);
  vi.useFakeTimers(); await vault.installUnlockedVault(v, decrypted);
  const handle = await vault.requireUnlock('connect');
  expect(await handle.sign('check')).toBeTypeOf('string');
  vi.advanceTimersByTime(vault.UNLOCK_WINDOW_MS);
  expect(vault.isAccountUnlocked()).toBe(false); expect(decrypted.vdk.every(x => x === 0)).toBe(true);
  await expect(handle.sign('check')).rejects.toMatchObject({ code: 'accountUnlockExpired' });
});
it('checks hash-wasm WASM core against RFC 9106 section 5.3 Argon2id', async () => {
  // The library public API hardcodes empty AD. Inject RFC AD only in this
  // test's H0 construction; its WASM core, passes and final hash stay intact.
  const require = createRequire(import.meta.url);
  const source = readFileSync(join(dirname(require.resolve('hash-wasm')), 'argon2.umd.min.js'), 'utf8');
  const needle = 'D.update(J(0));const a=';
  expect(source.split(needle)).toHaveLength(2);
  const exports: { argon2id?: (options: unknown) => Promise<string> } = {};
  runInNewContext(source.replace(needle, 'D.update(J(12)),D.update(new Uint8Array(12).fill(4));const a='), { exports, module: { exports }, Buffer, TextEncoder, WebAssembly, setTimeout });
  const tag = await exports.argon2id!({ password: new Uint8Array(32).fill(1), salt: new Uint8Array(16).fill(2), secret: new Uint8Array(8).fill(3), memorySize: 32, iterations: 3, parallelism: 4, hashLength: 32, outputType: 'hex' });
  expect(tag).toBe('0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659');
});
it('uses the production Argon2id API with NFC-normalized passwords', async () => {
  expect(await c.deriveMaster('cafe\u0301-password', v.kdf)).toEqual(await c.deriveMaster('caf\u00e9-password', v.kdf));
});
