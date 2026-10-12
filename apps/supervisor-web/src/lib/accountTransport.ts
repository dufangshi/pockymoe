import { currentAccount, currentBrowser, boundDevices, verifyDeviceCertificate, type BrowserIdentity, type DeviceCertificate, type AccountKeyInfo } from './accountVault';
import { accountRequest, accountRevocations, clearBrowser, AccountTrustError } from './accountVaultState';
import { signRequest } from './accountCrypto';
import { accountStore } from './accountStore';
export { AccountTrustError };
const registered = new Set<string>(), delivered = new Set<string>();
export async function browserRequestHeaders(browser: BrowserIdentity | null | undefined, aad: string, sealed: Uint8Array) {
  if (!browser) return {};
  return { 'x-rcd-session': browser.browserId, 'x-rcd-signature': await signRequest(browser.privateKey, aad, sealed) };
}
export const registrationKey = (b: BrowserIdentity, deviceId: string) => `${b.accountKeyId}:${b.browserId}:${deviceId}`;
export const isBrowserRegistered = (b: BrowserIdentity, deviceId: string) => registered.has(registrationKey(b, deviceId));
export const markBrowserRegistered = (b: BrowserIdentity, deviceId: string) => { registered.add(registrationKey(b, deviceId)); };
export const forgetBrowserRegistration = (b: BrowserIdentity, deviceId: string) => { registered.delete(registrationKey(b, deviceId)); };
export function resetBrowserTransport() { registered.clear(); delivered.clear(); }
export async function handleBrowserRevoked(browserId: string) {
  await clearBrowser(browserId); resetBrowserTransport();
}
export async function syncDeviceRevocations(deviceId: string, send: (revocations: Awaited<ReturnType<typeof accountRevocations>>) => Promise<void>) {
  const a = await currentAccount(); if (!a || !(await boundDevices()).includes(deviceId)) return;
  const key = `${a.accountKeyId}:${deviceId}`; if (delivered.has(key)) return;
  const list = await accountRevocations(true); await send(list); delivered.add(key);
  const b = await currentBrowser();
  if (b && list.some(r => r.kind === 'browser' && r.subjectId === b.browserId)) {
    await handleBrowserRevoked(b.browserId); throw new AccountTrustError('browser_revoked');
  }
}
export async function verifyTransportDevice(deviceId: string, identityKey: string, certificate: DeviceCertificate | null | undefined,
  hint?: { bound?: boolean; accountKeyId?: string }, enrollment = false) {
  const account = await currentAccount(), expected = (await boundDevices()).includes(deviceId);
  if (enrollment && !certificate) {
    if (account && hint?.accountKeyId && account.accountKeyId !== hint.accountKeyId) throw new AccountTrustError('account_mismatch');
    return { verified: false, bound: !!hint?.bound || expected, grantee: false };
  }
  let authority: AccountKeyInfo | null = account, grantee = false;
  if (certificate && account && certificate.accountKeyId !== account.accountKeyId && !expected) {
    // This metadata only selects a recipient path; its key remains TOFU unless
    // an invite pin was explicitly confirmed. grant-web strengthens that path.
    authority = await accountRequest<AccountKeyInfo>(`/relay/devices/${encodeURIComponent(deviceId)}/owner-account`);
    const pin = await accountStore<string>(`owner-pin:${deviceId}`);
    if (pin && authority.accountKeyId !== pin) throw new AccountTrustError('accountKeyChanged');
    if (!pin) await accountStore(`owner-pin:${deviceId}`, authority.accountKeyId);
    grantee = true;
  }
  if (certificate) {
    if (!authority) throw new AccountTrustError('accountUnlockRequired');
    if (!await verifyDeviceCertificate(certificate, authority, deviceId, identityKey)) throw new AccountTrustError('accountCertificateInvalid');
    if ((await accountRevocations()).some(r => r.kind === 'device' && r.subjectId === deviceId)) throw new AccountTrustError('accountDeviceRevoked');
    await accountStore(`device-certificate:${authority.accountKeyId}:${deviceId}`, certificate);
    return { verified: true, bound: true, grantee };
  }
  if (expected || hint?.bound || (account && await accountStore(`device-certificate:${account.accountKeyId}:${deviceId}`))) {
    if (!account) throw new AccountTrustError('accountUnlockRequired');
    throw new AccountTrustError('accountCertificateMissing');
  }
  return { verified: false, bound: false, grantee: false };
}
export async function hasAccountDeviceCertificate(deviceId: string) {
  const a = await currentAccount(); return !!a && !!await accountStore(`device-certificate:${a.accountKeyId}:${deviceId}`);
}

// Descriptor verification in exchange establishes the recipient role. The
// portal probes a thread-scoped endpoint for accounts other than its owner.
export async function verifySharedDeviceRole(deviceId: string, _scope?: string) {
  if ((await boundDevices()).includes(deviceId)) return false;
  const account = await currentAccount();
  if (!account) return false;
  try {
    const owner = await accountRequest<{ accountKeyId: string }>(`/relay/devices/${encodeURIComponent(deviceId)}/owner-account`);
    return owner.accountKeyId !== account.accountKeyId;
  } catch { return false; }
}
