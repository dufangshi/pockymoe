import { translate } from '@pockymoe/thread-ui/i18n';
import { argon2id } from 'hash-wasm';
import type { AccountKeyInfo, BrowserCertificate, DeviceCertificate, Revocation } from './accountVault';
export const utf8 = (s: string) => new TextEncoder().encode(s);
export const bytes = (v: Uint8Array): ArrayBuffer => v.slice().buffer;
export function encode64(v: ArrayBuffer | Uint8Array) {
  const a = v instanceof Uint8Array ? v : new Uint8Array(v);
  let s = ''; for (let i = 0; i < a.length; i += 8192) s += String.fromCharCode(...a.subarray(i, i + 8192));
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
export function decode64(s: string) {
  if (!/^[A-Za-z0-9_-]*$/.test(s)) throw new Error('Invalid base64url');
  const a = Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
  if (encode64(a) !== s) throw new Error('Invalid base64url'); return a;
}
export const random = (n: number) => crypto.getRandomValues(new Uint8Array(n));
export const hash = async (a: Uint8Array) => new Uint8Array(await crypto.subtle.digest('SHA-256', bytes(a)));
export const hash64 = async (a: Uint8Array) => encode64(await hash(a));
export interface Kdf { name: 'argon2id'; memoryKiB: number; iterations: number; parallelism: number; salt: string }
export const newKdf = (): Kdf => ({ name: 'argon2id', memoryKiB: 65536, iterations: 3, parallelism: 1, salt: encode64(random(16)) });
export function checkKdf(k: Kdf) {
  if (k.name !== 'argon2id' || ![k.memoryKiB, k.iterations, k.parallelism].every(Number.isSafeInteger) ||
    k.memoryKiB < 65536 || k.iterations < 3 || k.parallelism < 1 || decode64(k.salt).length !== 16)
    throw new Error(translate('auth.accountKdfUnsafe'));
}
export async function deriveMaster(password: string, k: Kdf) {
  checkKdf(k);
  return argon2id({ password: password.normalize('NFC'), salt: decode64(k.salt), memorySize: k.memoryKiB,
    iterations: k.iterations, parallelism: k.parallelism, hashLength: 32, outputType: 'binary' });
}
export async function hkdf(ikm: Uint8Array, info: string) {
  const key = await crypto.subtle.importKey('raw', bytes(ikm), 'HKDF', false, ['deriveBits']);
  return new Uint8Array(await crypto.subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(), info: utf8(info) }, key, 256));
}
export async function passwordKeys(password: string, kdf: Kdf) {
  const master = await deriveMaster(password, kdf);
  try { return { authKey: encode64(await hkdf(master, 'pockymoe-relay-auth-v1')), wrapKey: await hkdf(master, 'pockymoe-vault-wrap-v1') }; }
  finally { master.fill(0); }
}
const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
export function base32(a: Uint8Array) {
  let bits = 0, value = 0, out = '';
  for (const b of a) { value = (value << 8) | b; bits += 8; while (bits >= 5) { bits -= 5; out += alphabet[(value >>> bits) & 31]; } }
  if (bits) out += alphabet[(value << (5 - bits)) & 31]; return out;
}
export function parseRecovery(s: string) {
  const clean = s.toUpperCase().replace(/[- ]/g, '').replace(/O/g, '0').replace(/[IL]/g, '1');
  if (clean.length !== 32 || [...clean].some(c => !alphabet.includes(c))) throw new Error(translate('auth.accountRecoveryInvalid'));
  const out = new Uint8Array(20); let bits = 0, value = 0, i = 0;
  for (const c of clean) { value = (value << 5) | alphabet.indexOf(c); bits += 5; if (bits >= 8) { bits -= 8; out[i++] = (value >>> bits) & 255; } } return out;
}
export const recoveryDisplay = (a: Uint8Array) => base32(a).match(/.{4}/g)!.join('-');
export async function recoveryKeys(code: string) {
  const raw = parseRecovery(code);
  try { return { recoveryKey: await hkdf(raw, 'pockymoe-vault-recovery-v1'), recoveryAuthKey: encode64(await hkdf(raw, 'pockymoe-relay-recovery-v1')) }; }
  finally { raw.fill(0); }
}
export interface Sealed { iv: string; ciphertext: string }
export async function seal(key: Uint8Array, plain: Uint8Array, aad: string): Promise<Sealed> {
  const iv = random(12), imported = await crypto.subtle.importKey('raw', bytes(key), 'AES-GCM', false, ['encrypt']);
  return { iv: encode64(iv), ciphertext: encode64(await crypto.subtle.encrypt({ name: 'AES-GCM', iv: bytes(iv), additionalData: utf8(aad) }, imported, bytes(plain))) };
}
export async function open(key: Uint8Array, sealed: Sealed, aad: string) {
  const iv = decode64(sealed.iv); if (iv.length !== 12) throw new Error('accountVaultInvalid');
  const imported = await crypto.subtle.importKey('raw', bytes(key), 'AES-GCM', false, ['decrypt']);
  return new Uint8Array(await crypto.subtle.decrypt({ name: 'AES-GCM', iv: bytes(iv), additionalData: utf8(aad) }, imported, bytes(decode64(sealed.ciphertext))));
}
export const importPrivate = (s: string) => crypto.subtle.importKey('pkcs8', bytes(decode64(s)), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign']);
export const sign = async (key: CryptoKey, input: string) => encode64(await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, key, utf8(input)));
export async function verify(publicKey: string, signature: string, input: string) {
  try {
    if (decode64(signature).length !== 64) return false;
    const key = await crypto.subtle.importKey('spki', bytes(decode64(publicKey)), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
    return await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, key, bytes(decode64(signature)), utf8(input));
  } catch { return false; }
}
export async function validAccount(a: AccountKeyInfo) { try { return a.accountKeyId === await hash64(decode64(a.accountPublicKey)); } catch { return false; } }
export const inputs = {
  browser: (c: Omit<BrowserCertificate, 'version' | 'signature'>) => `pockymoe-acct-browser-v1\n${c.accountKeyId}\n${c.browserId}\n${c.publicKey}\n${c.createdAt}`,
  device: (c: Omit<DeviceCertificate, 'version' | 'signature'>) => `pockymoe-acct-device-v1\n${c.accountKeyId}\n${c.deviceId}\n${c.identityKey}\n${c.createdAt}`,
  revocation: (r: Omit<Revocation, 'version' | 'signature'>) => `pockymoe-acct-revoke-v1\n${r.accountKeyId}\n${r.kind}\n${r.subjectId}\n${r.revokedAt}`,
};
export async function verifyBrowserCertificate(c: BrowserCertificate, a: AccountKeyInfo) {
  try { return c.version === 1 && c.accountKeyId === a.accountKeyId && Number.isSafeInteger(c.createdAt) && c.createdAt >= 0 &&
    await validAccount(a) && c.browserId === await hash64(decode64(c.publicKey)) && await verify(a.accountPublicKey, c.signature, inputs.browser(c)); } catch { return false; }
}
export async function verifyRevocation(r: Revocation, a: AccountKeyInfo) {
  try { return r.version === 1 && ['browser', 'device'].includes(r.kind) && r.accountKeyId === a.accountKeyId &&
    Number.isSafeInteger(r.revokedAt) && r.revokedAt >= 0 && await validAccount(a) && await verify(a.accountPublicKey, r.signature, inputs.revocation(r)); } catch { return false; }
}
export const requestSigningInput = async (aad: string, sealed: Uint8Array) => `pockymoe-request-v1\n${aad}\n${await hash64(sealed)}`;
export const signRequest = async (key: CryptoKey, aad: string, sealed: Uint8Array) => sign(key, await requestSigningInput(aad, sealed));
export async function enrollmentMac(secret: string, deviceId: string, identityKey: string, accountKeyId: string) {
  const key = await crypto.subtle.importKey('raw', bytes(decode64(secret)), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  return encode64(await crypto.subtle.sign('HMAC', key, utf8(`pockymoe-acct-enroll-v1\n${deviceId}\n${identityKey}\n${accountKeyId}`)));
}
export function constantEqual(a: string, b: string) {
  const x = decode64(a), y = decode64(b); let mismatch = x.length ^ y.length;
  for (let i = 0; i < Math.max(x.length, y.length); i++) mismatch |= (x[i] ?? 0) ^ (y[i] ?? 0); return mismatch === 0;
}
