import type { RelayLoginResultDto, RelayRegisterResultDto } from '@pockymoe/shared';
import { accountRequest, checkVault, closeUnlockWindow, createVault, decryptVault, installUnlockedVault, registerBrowser,
  setActiveAccount, currentAccount, type Vault, AccountTrustError } from './accountVaultState';
import { passwordKeys, recoveryKeys, type Kdf } from './accountCrypto';
import { accountStore } from './accountStore';
interface PendingLogin { wrapKey: Uint8Array; authKey?: string; kdf?: Kdf; recovery: boolean; expires: number }
let pending: PendingLogin | undefined;
let pendingExpiry: ReturnType<typeof setTimeout> | undefined;
function expirePending() {
  if (pendingExpiry) clearTimeout(pendingExpiry);
  const attempt = pending;
  pendingExpiry = setTimeout(() => { if (pending === attempt) { attempt?.wrapKey.fill(0); if (attempt) delete attempt.authKey; } }, 10 * 60 * 1000);
}
let pendingIdentifier: string | undefined;
export interface AccountSessionState {
  mode: 'account' | 'legacy' | 'oauth'; setupRequired: boolean; recovery: boolean; hasVault: boolean; accountKeyId: string | null;
}
export async function accountSessionState() {
  const id = await accountStore<string>('active');
  return id ? await accountStore<AccountSessionState>(`session:${id}`) : undefined;
}
const loginCacheKey = (identifier: string) => `login:${identifier.normalize('NFC').trim().toLowerCase()}`;

export function cancelAccountLogin() { if (pendingExpiry) clearTimeout(pendingExpiry); pendingExpiry = undefined; pending?.wrapKey.fill(0); pending = undefined; pendingIdentifier = undefined; closeUnlockWindow(); }
export async function startAccountLogin(identifier: string, password: string) {
  cancelAccountLogin(); pendingIdentifier = identifier;
  const info = await accountRequest<{ mode: 'account' | 'legacy'; kdf: Kdf }>(`/relay/auth/kdf?login=${encodeURIComponent(identifier)}`);
  if (info.mode === 'legacy') throw new AccountTrustError('accountLoginDowngrade');
  if (info.mode !== 'account') throw new AccountTrustError('accountVaultInvalid');
  const keys = await passwordKeys(password, info.kdf);
  try {
    const result = await accountRequest<RelayLoginResultDto>('/relay/auth/login', 'POST', { identifier, authKey: keys.authKey });
    pending = { wrapKey: keys.wrapKey, authKey: keys.authKey, kdf: info.kdf, recovery: false, expires: Date.now() + 10 * 60 * 1000 };
    expirePending(); return result;
  } catch (e) { keys.wrapKey.fill(0); throw e; }
}
export function legacyUpgradeEnabled() { return import.meta.env.VITE_POCKYMOE_LEGACY_UPGRADE !== '0'; }
/** True when a failed sign-in could be an account that has not been secured yet. */
export async function legacyUpgradeMayApply(login: string) {
  if (!legacyUpgradeEnabled()) return false;
  const known = await accountStore<string>(loginCacheKey(login));
  return !(known && await accountStore(`cache:${known}`));
}
export async function startLegacyUpgrade(login: string, password: string) {
  cancelAccountLogin();
  const known = await accountStore<string>(loginCacheKey(login));
  if (!legacyUpgradeEnabled() || (known && await accountStore(`cache:${known}`))) throw new AccountTrustError('accountLoginDowngrade');
  pendingIdentifier = login;
  return accountRequest<RelayLoginResultDto>('/relay/auth/login', 'POST', { login, password, upgrade: true });
}
export async function startAccountRecovery(login: string, code: string) {
  cancelAccountLogin(); pendingIdentifier = login; const keys = await recoveryKeys(code);
  try {
    const result = await accountRequest<RelayLoginResultDto>('/relay/auth/recover', 'POST', { login, recoveryAuthKey: keys.recoveryAuthKey });
    pending = { wrapKey: keys.recoveryKey, recovery: true, expires: Date.now() + 10 * 60 * 1000 }; expirePending(); return result;
  } catch (e) { keys.recoveryKey.fill(0); throw e; }
}
export async function finishAccountLogin(userId: string) {
  await setActiveAccount(userId);
  const attempt = pending;
  if (!attempt) { const state = await accountSessionState(); return { setup: !state?.hasVault, recovery: !!state?.recovery }; }
  try {
    if (attempt.expires <= Date.now()) throw new AccountTrustError('accountUnlockExpired');
    const vault = await accountRequest<Vault>('/relay/account/vault'); await checkVault(vault, userId);
    if (attempt.kdf && (['name', 'memoryKiB', 'iterations', 'parallelism', 'salt'] as const).some(k => attempt.kdf![k] !== vault.kdf[k])) throw new AccountTrustError('accountVaultInvalid');
    const data = await decryptVault(vault, attempt.wrapKey, attempt.recovery ? 'recovery' : 'password', userId);
    await installUnlockedVault(vault, data, attempt.authKey);
    if (!attempt.recovery) await registerBrowser();
    if (pendingIdentifier) await accountStore(loginCacheKey(pendingIdentifier), userId);
    return { setup: false, recovery: attempt.recovery };
  } finally { attempt.wrapKey.fill(0); if (pending === attempt) { pending = undefined; if (pendingExpiry) clearTimeout(pendingExpiry); pendingExpiry = undefined; } }
}
export type PreparedRegistration = Awaited<ReturnType<typeof createVault>>;
export async function registerAccount(input: { email: string; username: string; registrationPassword?: string }, prepared: PreparedRegistration) {
  const result = await accountRequest<RelayRegisterResultDto>('/relay/auth/register', 'POST', { ...input,
    userId: prepared.vault.userId, vault: prepared.vault, authKey: prepared.authKey, recoveryAuthKey: prepared.recoveryAuthKey });
  for (const identifier of [input.email, input.username]) await accountStore(loginCacheKey(identifier), prepared.vault.userId);
  return result;
}
export async function setupAccount(prepared: PreparedRegistration, oauthOnly: boolean) {
  const id = await accountStore<string>('active'); if (!id || prepared.vault.userId !== id) throw new AccountTrustError('accountVaultInvalid');
  await checkVault(prepared.vault, id);
  await accountRequest('/relay/account/vault', 'PUT', { vault: prepared.vault, expectedRevision: 0,
    ...(!oauthOnly ? { authKey: prepared.authKey } : {}), recoveryAuthKey: prepared.recoveryAuthKey });
  if (pendingIdentifier) await accountStore(loginCacheKey(pendingIdentifier), prepared.vault.userId);
  await setAccountSession(prepared.vault.userId, { mode: oauthOnly ? 'oauth' : 'account', setupRequired: false, recovery: false, hasVault: true, accountKeyId: prepared.vault.accountKeyId });
}
export async function setAccountSession(userId: string | null, state: AccountSessionState | boolean = false) {
  await setActiveAccount(userId);
  if (!userId) return;
  const metadata = typeof state === 'boolean' ? { mode: state ? 'oauth' as const : 'account' as const, setupRequired: false, recovery: false, hasVault: true, accountKeyId: null } : state;
  if (metadata.recovery) closeUnlockWindow();
  await accountStore(`session:${userId}`, metadata); await accountStore(`oauth:${userId}`, metadata.mode === 'oauth');
  const cached = await currentAccount();
  if (cached && metadata.accountKeyId && metadata.accountKeyId !== cached.accountKeyId) throw new AccountTrustError('accountKeyChanged');
}
export async function startTemporaryResetLogin(identifier: string, temporaryPassword: string) {
  cancelAccountLogin();
  return accountRequest<RelayLoginResultDto>('/relay/auth/login', 'POST', { identifier, password: temporaryPassword });
}
export async function isOAuthAccount() { const id = await accountStore<string>('active'); return !!id && !!await accountStore(`oauth:${id}`); }

if (typeof window !== 'undefined') window.addEventListener('pagehide', cancelAccountLogin);

// Relay security reauthentication follows the same password split as login.
export async function accountPasswordProof(password: string, explicitAdmin = false) {
  const id = await accountStore<string>('active');
  // The admin route is an explicit local choice; a relay role never bypasses a cached vault.
  if (explicitAdmin && !await currentAccount()) return { password };
  if (!id) throw new AccountTrustError('accountUnlockRequired');
  if (await isOAuthAccount()) throw new AccountTrustError('accountOAuthFactorRequired');
  const vault = await accountRequest<Vault>('/relay/account/vault'); await checkVault(vault, id);
  const keys = await passwordKeys(password, vault.kdf);
  try { return { authKey: keys.authKey }; } finally { keys.wrapKey.fill(0); }
}
