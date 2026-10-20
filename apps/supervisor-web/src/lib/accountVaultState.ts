import { translate } from '@pockymoe/thread-ui/i18n';
import { unlockCause, type UnlockCause } from './unlockCause';
import type { AccountKeyInfo, BoundDevice, BrowserIdentity, DeviceCertificate, Revocation, UnlockedAccount, UnlockReason } from './accountVault';
import { accountStore } from './accountStore';
import { relayUrl } from './relayOrigin';
import { browserLabel } from './browserLabel';
import * as c from './accountCrypto';
export interface Vault extends AccountKeyInfo {
  version: 1; kdf: c.Kdf; slots: { password: c.Sealed; recovery: c.Sealed }; body: c.Sealed; revision: number;
}
interface Body { version: 1; accountPrivateKey: string; boundDevices: BoundDevice[] | string[]; updatedAt: number }
interface Cache extends AccountKeyInfo { revision: number; boundDevices: BoundDevice[] | string[] }
interface WindowState { vault: Vault; body: Body; vdk: Uint8Array; key: CryptoKey | null; expires: number; authKey?: string; recoveryAuthKey?: string }
let unlocked: WindowState | undefined;
let expiry: ReturnType<typeof setTimeout> | undefined;
let serial: Promise<unknown> = Promise.resolve();
export const UNLOCK_WINDOW_MS = 10 * 60 * 1000;
export const ACCOUNT_UNLOCK_REQUEST = 'pockymoe-account-unlock-request';
export const ACCOUNT_CHANGED = 'pockymoe-account-changed';
/** This browser found itself in the account-signed revocation list. */
export const BROWSER_REMOVED = 'pockymoe-browser-removed';
function accountErrorMessage(code: string) {
  try { return translate(`auth.${code}` as Parameters<typeof translate>[0]); }
  catch { return translate('auth.accountRequestFailed'); }
}
export class AccountTrustError extends Error {
  constructor(public code: string) { super(accountErrorMessage(code)); }
}
export async function accountRequest<T>(path: string, method = 'GET', body?: unknown): Promise<T> {
  const response = await fetch(relayUrl(path), { method, credentials: 'include', cache: 'no-store',
    headers: { 'content-type': 'application/json' }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
  if (!response.ok) {
    const data = await response.json().catch(() => null) as { code?: string } | null;
    throw new AccountTrustError(data?.code ?? 'accountRequestFailed');
  }
  return response.status === 204 ? undefined as T : await response.json() as T;
}
export async function setActiveAccount(userId: string | null) {
  const previous = await accountStore<string>('active');
  if (previous !== userId) closeUnlockWindow();
  await accountStore('active', userId);
}
export async function currentAccount(): Promise<AccountKeyInfo | null> {
  const id = await accountStore<string>('active'); if (!id) return null;
  const cache = await accountStore<Cache>(`cache:${id}`);
  return cache ? { userId: cache.userId, accountKeyId: cache.accountKeyId, accountPublicKey: cache.accountPublicKey } : null;
}
export async function currentBrowser(): Promise<BrowserIdentity | null> {
  const a = await currentAccount(); if (!a) return null;
  const browser = await accountStore<BrowserIdentity>(`browser:${a.userId}`);
  if (!browser || browser.accountKeyId !== a.accountKeyId) return null;
  if (!await c.verifyBrowserCertificate(browser.certificate, a)) throw new AccountTrustError('accountCertificateInvalid');
  return browser;
}
export async function boundDevices() {
  const a = await currentAccount(); if (!a) return [];
  return (await boundDeviceIdentities()).map(d => d.deviceId);
}
async function normalizeBoundDevices(entries: (string | BoundDevice)[], account: AccountKeyInfo): Promise<BoundDevice[]> {
  return Promise.all(entries.map(async entry => {
    if (typeof entry !== 'string') return { ...entry };
    // v2 was never shipped, but earlier development vaults used IDs alone.
    // Recover their authenticated identity from the locally verified certificate.
    const cert = await accountStore<DeviceCertificate>(`device-certificate:${account.accountKeyId}:${entry}`);
    return { deviceId: entry, identityKey: cert && await verifyDeviceCertificate(cert, account, entry, cert.identityKey) ? cert.identityKey : '' };
  }));
}
export async function boundDeviceIdentities(): Promise<BoundDevice[]> {
  const a = await currentAccount(); if (!a) return [];
  const cache = await accountStore<Cache>(`cache:${a.userId}`); if (!cache) return [];
  const entries = await normalizeBoundDevices(cache.boundDevices, a);
  if (cache.boundDevices.some(d => typeof d === 'string')) await accountStore(`cache:${a.userId}`, { ...cache, boundDevices: entries });
  return entries;
}
export async function confirmBoundDeviceIdentity(deviceId: string, identityKey: string, expectedPreviousIdentityKey?: string | null) {
  await requireUnlock('certify-device');
  if (!deviceId || c.decode64(identityKey).length !== 65) throw new AccountTrustError('accountCertificateInvalid');
  const task = serial.then(async () => {
    const w = windowState();
    const entries = await normalizeBoundDevices(w.body.boundDevices, w.vault);
    const previous = entries.find(d => d.deviceId === deviceId);
    if (expectedPreviousIdentityKey !== undefined ? (previous?.identityKey ?? null) !== expectedPreviousIdentityKey : previous && previous.identityKey !== identityKey) throw new AccountTrustError('accountDeviceIdentityChanged');
    const next = [...entries.filter(d => d.deviceId !== deviceId), { deviceId, identityKey }];
    await saveWindow(w, { ...w.body, boundDevices: next, updatedAt: Date.now() });
  }); serial = task.catch(() => {}); await task;
}
export function closeUnlockWindow() {
  if (expiry) clearTimeout(expiry); expiry = undefined;
  if (unlocked) { unlocked.vdk.fill(0); unlocked.body.accountPrivateKey = ''; unlocked.key = null; delete unlocked.authKey; delete unlocked.recoveryAuthKey; }
  unlocked = undefined;
}
if (typeof window !== 'undefined') {
  window.addEventListener('pagehide', closeUnlockWindow);
  window.addEventListener('pockymoe-account-logout', closeUnlockWindow);
}
export function isAccountUnlocked() { if (unlocked && unlocked.expires <= Date.now()) closeUnlockWindow(); return !!unlocked; }
async function cacheVault(v: Vault, body: Body) {
  await accountStore<Cache>(`cache:${v.userId}`, { userId: v.userId, accountKeyId: v.accountKeyId,
    accountPublicKey: v.accountPublicKey, revision: v.revision, boundDevices: body.boundDevices });
  await accountStore('has-opened-vault', true);
}
export async function checkVault(v: Vault, expectedUserId: string) {
  if (!v || v.version !== 1 || v.userId !== expectedUserId || !Number.isSafeInteger(v.revision) || v.revision < 1 ||
    new TextEncoder().encode(JSON.stringify(v)).length > 65536 || !await c.validAccount(v)) throw new AccountTrustError('accountVaultInvalid');
  c.checkKdf(v.kdf);
  const saved = await accountStore<Cache>(`cache:${v.userId}`);
  if (saved && (v.accountKeyId !== saved.accountKeyId || v.accountPublicKey !== saved.accountPublicKey))
    throw new AccountTrustError('accountKeyChanged');
  if (saved && v.revision < saved.revision) throw new AccountTrustError('accountVaultRollback');
}
export const slotAad = (v: AccountKeyInfo, name: string) => `pockymoe-vault-slot-v1\n${v.userId}\n${v.accountKeyId}\n${name}`;
export const bodyAad = (v: AccountKeyInfo & { revision: number }) => `pockymoe-vault-body-v1\n${v.userId}\n${v.accountKeyId}\n${v.revision}`;
export async function decryptVault(v: Vault, slotKey: Uint8Array, slot: 'password' | 'recovery', expectedUserId: string) {
  await checkVault(v, expectedUserId);
  let vdk: Uint8Array | undefined;
  try {
    vdk = await c.open(slotKey, v.slots[slot], slotAad(v, slot));
    if (vdk.length !== 32) throw new Error();
    const raw = await c.open(vdk, v.body, bodyAad(v));
    let body: Body; try { body = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(raw)); } finally { raw.fill(0); }
    if (body.version !== 1 || !Number.isSafeInteger(body.updatedAt) || body.updatedAt < 0 || !Array.isArray(body.boundDevices) || body.boundDevices.some(d => typeof d !== 'string' && (!d || typeof d.deviceId !== 'string' || typeof d.identityKey !== 'string' || (d.identityKey !== '' && c.decode64(d.identityKey).length !== 65)))) throw new Error();
    const key = await c.importPrivate(body.accountPrivateKey);
    if (!await c.verify(v.accountPublicKey, await c.sign(key, 'pockymoe-account-key-check-v1'), 'pockymoe-account-key-check-v1')) throw new Error();
    body.boundDevices = await normalizeBoundDevices(body.boundDevices, v);
    return { body, vdk, key };
  } catch { vdk?.fill(0); throw new AccountTrustError('accountUnlockFailed'); }
}
export async function openVault(v: Vault, secret: string, recovery = false, expectedUserId = v.userId) {
  await checkVault(v, expectedUserId);
  const keys = recovery ? await c.recoveryKeys(secret) : await c.passwordKeys(secret, v.kdf);
  const slotKey = 'wrapKey' in keys ? keys.wrapKey : keys.recoveryKey;
  try {
    const data = await decryptVault(v, slotKey, recovery ? 'recovery' : 'password', expectedUserId);
    await installUnlockedVault(v, data, 'authKey' in keys && !await accountStore(`oauth:${v.userId}`) ? keys.authKey : undefined,
      'recoveryAuthKey' in keys ? keys.recoveryAuthKey : undefined);
    return windowHandle();
  } finally { slotKey.fill(0); }
}
export async function installUnlockedVault(v: Vault, data: Awaited<ReturnType<typeof decryptVault>>, authKey?: string, recoveryAuthKey?: string) {
  await setActiveAccount(v.userId); closeUnlockWindow();
  unlocked = { ...data, vault: v, expires: Date.now() + UNLOCK_WINDOW_MS, ...(authKey ? { authKey } : {}), ...(recoveryAuthKey ? { recoveryAuthKey } : {}) };
  expiry = setTimeout(closeUnlockWindow, UNLOCK_WINDOW_MS);
  await cacheVault(v, data.body);
}
// A previously returned handle cannot extend or bypass the ten minute window.
function windowState() { if (!isAccountUnlocked()) throw new AccountTrustError('accountUnlockExpired'); return unlocked!; }
function windowHandle(): UnlockedAccount {
  const w = windowState(), id = w.vault.accountKeyId;
  const current = () => { const active = windowState(); if (active !== w || active.vault.accountKeyId !== id) throw new AccountTrustError('accountUnlockExpired'); return active; };
  return { userId: w.vault.userId, accountKeyId: id, accountPublicKey: w.vault.accountPublicKey,
    sign: async input => { const result = await c.sign(current().key!, input); current(); return result; },
    updateBoundDevices: update => {
      const task = serial.then(async () => {
        const active = current(); const entries = await normalizeBoundDevices(active.body.boundDevices, active.vault); const ids = update(entries.map(d => d.deviceId));
        if (!Array.isArray(ids) || ids.some(id => typeof id !== 'string')) throw new AccountTrustError('accountVaultInvalid');
        await saveWindow(active, { ...active.body, boundDevices: await normalizeBoundDevices([...new Set(ids)].map(id => entries.find(d => d.deviceId === id) ?? id), active.vault), updatedAt: Date.now() });
      }); serial = task.catch(() => {}); return task;
    },
  };
}
/** `cause` says why, for the prompt and support reports; by default the action itself. */
export async function requireUnlock(reason: UnlockReason, cause: UnlockCause = unlockCause('action', { action: reason })): Promise<UnlockedAccount> {
  if (isAccountUnlocked()) return windowHandle();
  if (typeof window === 'undefined') throw new AccountTrustError('accountUnlockRequired');
  return new Promise((resolve, reject) => window.dispatchEvent(new CustomEvent(ACCOUNT_UNLOCK_REQUEST, {
    detail: { reason, cause, resolve, reject },
  })));
}
export async function verifyDeviceCertificate(cert: DeviceCertificate, account: AccountKeyInfo, deviceId: string, identityKey: string) {
  try { return cert.version === 1 && cert.accountKeyId === account.accountKeyId && cert.deviceId === deviceId &&
    cert.identityKey === identityKey && c.decode64(identityKey).length === 65 && Number.isSafeInteger(cert.createdAt) && cert.createdAt >= 0 &&
    await c.validAccount(account) && await c.verify(account.accountPublicKey, cert.signature, c.inputs.device(cert)); } catch { return false; }
}
export async function safetyCode(accountKeyId: string) {
  return c.base32(await c.hash(c.utf8(`pockymoe-acct-safety-v1\n${accountKeyId}`))).slice(0, 12).match(/.{4}/g)!.join(' ');
}
export async function createVault(userId: string, password: string) {
  if ([...password.normalize('NFC')].length < 12) throw new AccountTrustError('accountPasswordShort');
  const pair = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const accountPublicKey = c.encode64(await crypto.subtle.exportKey('spki', pair.publicKey));
  const info = { userId, accountPublicKey, accountKeyId: await c.hash64(c.decode64(accountPublicKey)) };
  const kdf = c.newKdf(), keys = await c.passwordKeys(password, kdf);
  const recoveryCode = c.recoveryDisplay(c.random(20)), recovery = await c.recoveryKeys(recoveryCode), vdk = c.random(32);
  const body: Body = { version: 1, accountPrivateKey: c.encode64(await crypto.subtle.exportKey('pkcs8', pair.privateKey)), boundDevices: [], updatedAt: Date.now() };
  try {
    const vault: Vault = { ...info, version: 1, kdf, revision: 1,
      slots: { password: await c.seal(keys.wrapKey, vdk, slotAad(info, 'password')),
        recovery: await c.seal(recovery.recoveryKey, vdk, slotAad(info, 'recovery')) },
      body: await c.seal(vdk, c.utf8(JSON.stringify(body)), bodyAad({ ...info, revision: 1 })) };
    return { vault, authKey: keys.authKey, recoveryAuthKey: recovery.recoveryAuthKey, recoveryCode };
  } finally { vdk.fill(0); keys.wrapKey.fill(0); recovery.recoveryKey.fill(0); body.accountPrivateKey = ''; }
}
async function saveWindow(w: WindowState, body: Body, extra: Record<string, unknown> = {}, fields: Partial<Vault> = {}) {
  if (windowState() !== w) throw new AccountTrustError('accountUnlockExpired');
  const vault: Vault = { ...w.vault, ...fields, revision: w.vault.revision + 1 };
  vault.body = await c.seal(w.vdk, c.utf8(JSON.stringify(body)), bodyAad(vault));
  if (windowState() !== w) throw new AccountTrustError('accountUnlockExpired');
  await accountRequest('relay/account/vault', 'PUT', { vault, expectedRevision: w.vault.revision, ...(w.authKey ? { currentAuthKey: w.authKey } : w.recoveryAuthKey ? { currentRecoveryAuthKey: w.recoveryAuthKey } : {}), ...extra });
  if (windowState() !== w) throw new AccountTrustError('accountUnlockExpired');
  w.vault = vault; w.body = body; await cacheVault(vault, body);
}
export async function unlockAccount(secret: string, recovery = false) {
  const id = await accountStore<string>('active'); if (!id) throw new AccountTrustError('accountUnlockRequired');
  const vault = await accountRequest<Vault>('relay/account/vault');
  return openVault(vault, secret, recovery, id);
}
export async function registerBrowser(label = browserLabel(), source = 'registerBrowser') {
  let browser = await currentBrowser();
  const revoked = await accountRevocations(true);
  if (browser && revoked.some(r => r.kind === 'browser' && r.subjectId === browser!.browserId)) {
    await accountStore(`browser:${browser.userId}`, null); browser = null;
    // The revocation is signed by the account key, so it is authoritative; say why the prompt appears.
    if (typeof window !== 'undefined') window.dispatchEvent(new Event(BROWSER_REMOVED));
  }
  let w: WindowState | undefined;
  if (!browser) {
    const handle = await requireUnlock('connect', unlockCause('missing-browser-key', { source })); w = windowState();
    const pair = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
    const publicKey = c.encode64(await crypto.subtle.exportKey('spki', pair.publicKey));
    const fields = { accountKeyId: handle.accountKeyId, browserId: await c.hash64(c.decode64(publicKey)), publicKey, createdAt: Date.now() };
    browser = { userId: handle.userId, accountKeyId: handle.accountKeyId, accountPublicKey: handle.accountPublicKey,
      browserId: fields.browserId, privateKey: pair.privateKey, certificate: { version: 1, ...fields, signature: await handle.sign(c.inputs.browser(fields)) } };
  }
  await accountRequest('relay/account/browsers', 'POST', { certificate: browser.certificate, label: label.slice(0, 80) });
  if (w) {
    if (windowState() !== w) throw new AccountTrustError('accountUnlockExpired');
    await accountStore(`browser:${browser.userId}`, browser);
  } else if ((await currentBrowser())?.browserId !== browser.browserId) throw new AccountTrustError('accountUnlockRequired');
  if (typeof window !== 'undefined') window.dispatchEvent(new Event(ACCOUNT_CHANGED));
  return browser;
}
export async function clearBrowser(browserId?: string) {
  const userId = await accountStore<string>('active'); if (!userId) return;
  const current = await accountStore<BrowserIdentity>(`browser:${userId}`);
  if (!browserId || current?.browserId === browserId) {
    await accountStore(`browser:${userId}`, null); closeUnlockWindow();
    if (typeof window !== 'undefined') window.dispatchEvent(new Event('pockymoe-account-logout'));
  }
}
export async function logoutAccount() { await clearBrowser(); await setActiveAccount(null); closeUnlockWindow(); }
export async function accountRevocations(refresh = false) {
  const a = await currentAccount(); if (!a) return [];
  const saved = await accountStore<Revocation[]>(`revocations:${a.userId}`) ?? [];
  if (!refresh) return saved;
  const data = await accountRequest<{ revocations: Revocation[] }>('relay/account/browsers');
  if (!Array.isArray(data.revocations)) throw new AccountTrustError('accountCertificateInvalid');
  const valid: Revocation[] = [];
  for (const r of [...saved, ...data.revocations]) if (r?.accountKeyId === a.accountKeyId && await c.verifyRevocation(r, a)) valid.push(r);
  const merged = new Map(valid.map(r => [`${r.kind}:${r.subjectId}`, r]));
  await accountStore(`revocations:${a.userId}`, [...merged.values()]); return [...merged.values()];
}
export async function revokeAccountSubject(kind: 'browser' | 'device', subjectId: string) {
  const a = await requireUnlock('revoke');
  const fields = { accountKeyId: a.accountKeyId, kind, subjectId, revokedAt: Date.now() };
  const revocation: Revocation = { version: 1, ...fields, signature: await a.sign(c.inputs.revocation(fields)) };
  await accountRequest('relay/account/revocations', 'POST', { revocation });
  const saved = await accountRevocations(); await accountStore(`revocations:${a.userId}`, [...saved, revocation]);
  if (typeof window !== 'undefined') window.dispatchEvent(new Event('pockymoe-account-revocations'));
  if ((await currentBrowser())?.browserId === subjectId && kind === 'browser') await clearBrowser(subjectId);
  return revocation;
}
export async function changeAccountPassword(password: string, oauthOnly = false) {
  await requireUnlock('change-password'); const w = windowState();
  if ([...password.normalize('NFC')].length < 12) throw new AccountTrustError('accountPasswordShort');
  const kdf = c.newKdf(), keys = await c.passwordKeys(password, kdf);
  try {
    const slots = { ...w.vault.slots, password: await c.seal(keys.wrapKey, w.vdk, slotAad(w.vault, 'password')) };
    await saveWindow(w, { ...w.body, updatedAt: Date.now() }, oauthOnly ? {} : {
      authKey: keys.authKey }, { kdf, slots });
    if (oauthOnly) delete w.authKey;
    else w.authKey = keys.authKey;
    delete w.recoveryAuthKey;
  } finally { keys.wrapKey.fill(0); }
}
export async function regenerateRecoveryCode() {
  await requireUnlock('recovery-code'); const w = windowState();
  const code = c.recoveryDisplay(c.random(20)), keys = await c.recoveryKeys(code);
  try { await saveWindow(w, { ...w.body, updatedAt: Date.now() }, { recoveryAuthKey: keys.recoveryAuthKey },
    { slots: { ...w.vault.slots, recovery: await c.seal(keys.recoveryKey, w.vdk, slotAad(w.vault, 'recovery')) } });
    if (w.recoveryAuthKey) w.recoveryAuthKey = keys.recoveryAuthKey;
    return code; }
  finally { keys.recoveryKey.fill(0); }
}
export async function startAccountOver(password?: string) {
  const id = await accountStore<string>('active'); if (!id) throw new AccountTrustError('accountUnlockRequired');
  const state = await accountStore<{ recovery?: boolean }>(`session:${id}`);
  let proof = {};
  if (!state?.recovery) {
    if (!password) throw new AccountTrustError('accountPasswordConfirmationRequired');
    // Always verify a fresh confirmation locally, including OAuth-only accounts
    // whose unlock password never leaves this browser.
    await unlockAccount(password);
    const w = windowState();
    if (w.authKey) proof = { currentAuthKey: w.authKey };
  }
  if (await accountStore<string>('active') !== id) throw new AccountTrustError('accountUnlockRequired');
  await accountRequest('relay/account/vault', 'DELETE', proof);
  await logoutAccount(); await accountStore(`cache:${id}`, null); await accountStore(`revocations:${id}`, null);
}
