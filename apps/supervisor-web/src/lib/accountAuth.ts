import type { RelayLoginResultDto, RelayRegisterResultDto } from '@pockymoe/shared';
import { accountRequest, checkVault, closeUnlockWindow, createVault, decryptVault, installUnlockedVault, registerBrowser,
  setActiveAccount, type Vault, AccountTrustError } from './accountVaultState';
import { passwordKeys, recoveryKeys, type Kdf } from './accountCrypto';
import { accountStore } from './accountStore';
interface PendingLogin { wrapKey: Uint8Array; authKey?: string; kdf?: Kdf; recovery: boolean }
let pending: PendingLogin | undefined;
export function cancelAccountLogin() { pending?.wrapKey.fill(0); pending = undefined; closeUnlockWindow(); }
export async function startAccountLogin(identifier: string, password: string) {
  cancelAccountLogin();
  const info = await accountRequest<{ mode: 'account' | 'legacy'; kdf: Kdf }>(`/relay/auth/kdf?login=${encodeURIComponent(identifier)}`);
  if (info.mode === 'legacy') return accountRequest<RelayLoginResultDto>('/relay/auth/login', 'POST', { identifier, password });
  if (info.mode !== 'account') throw new AccountTrustError('accountVaultInvalid');
  const keys = await passwordKeys(password, info.kdf);
  try {
    const result = await accountRequest<RelayLoginResultDto>('/relay/auth/login', 'POST', { identifier, authKey: keys.authKey });
    pending = { wrapKey: keys.wrapKey, authKey: keys.authKey, kdf: info.kdf, recovery: false };
    return result;
  } catch (e) { keys.wrapKey.fill(0); throw e; }
}
export async function startAccountRecovery(login: string, code: string) {
  cancelAccountLogin(); const keys = await recoveryKeys(code);
  try {
    const result = await accountRequest<RelayLoginResultDto>('/relay/auth/recover', 'POST', { login, recoveryAuthKey: keys.recoveryAuthKey });
    pending = { wrapKey: keys.recoveryKey, recovery: true }; return result;
  } catch (e) { keys.recoveryKey.fill(0); throw e; }
}
export async function finishAccountLogin(userId: string) {
  await setActiveAccount(userId);
  const attempt = pending; if (!attempt) return { setup: true, recovery: false };
  try {
    const vault = await accountRequest<Vault>('/relay/account/vault'); await checkVault(vault, userId);
    if (attempt.kdf && (['name', 'memoryKiB', 'iterations', 'parallelism', 'salt'] as const).some(k => attempt.kdf![k] !== vault.kdf[k])) throw new AccountTrustError('accountVaultInvalid');
    const data = await decryptVault(vault, attempt.wrapKey, attempt.recovery ? 'recovery' : 'password', userId);
    await installUnlockedVault(vault, data, attempt.authKey);
    if (!attempt.recovery) await registerBrowser();
    return { setup: false, recovery: attempt.recovery };
  } finally { attempt.wrapKey.fill(0); if (pending === attempt) pending = undefined; }
}
export type PreparedRegistration = Awaited<ReturnType<typeof createVault>>;
export async function registerAccount(input: { email: string; username: string; registrationPassword?: string }, prepared: PreparedRegistration) {
  return accountRequest<RelayRegisterResultDto>('/relay/auth/register', 'POST', { ...input,
    vault: prepared.vault, authKey: prepared.authKey, recoveryAuthKey: prepared.recoveryAuthKey });
}
export async function setupAccount(prepared: PreparedRegistration, oauthOnly: boolean) {
  const id = await accountStore<string>('active'); if (!id || prepared.vault.userId !== id) throw new AccountTrustError('accountVaultInvalid');
  await accountRequest('/relay/account/vault', 'PUT', { vault: prepared.vault, expectedRevision: 0,
    ...(!oauthOnly ? { authKey: prepared.authKey } : {}), recoveryAuthKey: prepared.recoveryAuthKey });
}
export async function setAccountSession(userId: string | null, oauthOnly = false) {
  await setActiveAccount(userId); if (userId) await accountStore(`oauth:${userId}`, oauthOnly);
}
export async function isOAuthAccount() { const id = await accountStore<string>('active'); return !!id && !!await accountStore(`oauth:${id}`); }
