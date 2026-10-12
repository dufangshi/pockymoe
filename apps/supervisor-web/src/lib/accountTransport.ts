import { verifySharedDevice, rememberCertifiedDevice, locallyCertifiedDevices, ownerAccountPin } from './sharedDeviceTrust';
import { currentAccount, currentBrowser, boundDevices, boundDeviceIdentities, verifyDeviceCertificate, type BrowserIdentity, type DeviceCertificate } from './accountVault';
import { accountRequest, accountRevocations, clearBrowser, AccountTrustError } from './accountVaultState';
import { signRequest } from './accountCrypto';
import { accountStore } from './accountStore';
export { AccountTrustError };
const registered = new Set<string>(), delivered = new Set<string>(), revokedBrowsers = new Set<string>();
export async function browserRequestHeaders(browser: BrowserIdentity | null | undefined, aad: string, sealed: Uint8Array) {
  if (!browser) return {};
  if (revokedBrowsers.has(browser.browserId)) throw new AccountTrustError('browser_revoked');
  return { 'x-rcd-session': browser.browserId, 'x-rcd-signature': await signRequest(browser.privateKey, aad, sealed) };
}
export const registrationKey = (b: BrowserIdentity, deviceId: string) => `${b.accountKeyId}:${b.browserId}:${deviceId}`;
export const isBrowserRegistered = (b: BrowserIdentity, deviceId: string) => registered.has(registrationKey(b, deviceId));
export const markBrowserRegistered = (b: BrowserIdentity, deviceId: string) => { registered.add(registrationKey(b, deviceId)); };
export const forgetBrowserRegistration = (b: BrowserIdentity, deviceId: string) => { registered.delete(registrationKey(b, deviceId)); };
export function resetBrowserTransport() { registered.clear(); delivered.clear(); }
export async function handleBrowserRevoked(browserId: string) {
  revokedBrowsers.add(browserId); await clearBrowser(browserId); resetBrowserTransport();
}
// Account-scoped inventory comes from the owned Devices view, never owner-account.
export async function markOwnedDevices(deviceIds: string[]) {
  const a = await currentAccount(); if (!a) return;
  const existing = await accountStore<string[]>(`owned-devices:${a.accountKeyId}`) ?? [];
  await accountStore(`owned-devices:${a.accountKeyId}`, [...new Set([...existing, ...deviceIds])]);
}
async function isOwnedDevice(deviceId: string) {
  const a = await currentAccount(); if (!a) return false;
  if ((await accountStore<string[]>(`owned-devices:${a.accountKeyId}`) ?? []).includes(deviceId)) return true;
  // Pending enrollment is stored in its own database and is authoritative local intent.
  const { listPendingEnrollments } = await import('./deviceEnrollment');
  return (await listPendingEnrollments(a)).some(e => e.deviceId === deviceId);
}
export async function syncDeviceRevocations(deviceId: string, send: (revocations: Awaited<ReturnType<typeof accountRevocations>>) => Promise<void>) {
  const a = await currentAccount(); if (!a) return;
  const owned = (await boundDevices()).includes(deviceId) || !!await accountStore(`device-certificate:${a.accountKeyId}:${deviceId}`);
  if (!owned && !await accountStore(`protected-device:${a.accountKeyId}:${deviceId}`)) return;
  const key = `${a.accountKeyId}:${deviceId}`; if (delivered.has(key)) return;
  const all = await accountRevocations(true), list = owned ? all : all.filter(r => r.kind === 'browser');
  await send(list); delivered.add(key);
  if (list.some(r => r.kind === 'device' && r.subjectId === deviceId)) throw new AccountTrustError('accountDeviceRevoked');
  const b = await currentBrowser();
  if (b && list.some(r => r.kind === 'browser' && r.subjectId === b.browserId)) {
    await handleBrowserRevoked(b.browserId); throw new AccountTrustError('browser_revoked');
  }
}
export async function verifyTransportDevice(deviceId: string, identityKey: string, certificate: DeviceCertificate | null | undefined,
  hint?: { bound?: boolean; accountKeyId?: string }, enrollment = false) {
  const account = await currentAccount();
  const pin = (await boundDeviceIdentities()).find(d => d.deviceId === deviceId);
  const expected = !!pin || (!!account && (!!await accountStore(`device-certificate:${account.accountKeyId}:${deviceId}`) || (await locallyCertifiedDevices()).includes(deviceId)));

  if (enrollment && !certificate && (!expected || pin?.identityKey === identityKey)) {
    if (account && hint?.accountKeyId && account.accountKeyId !== hint.accountKeyId) throw new AccountTrustError('account_mismatch');
    return { verified: false, bound: !!hint?.bound || expected, grantee: false, identityTrust: 'tofu' as const };
  }
  const owned = expected || await isOwnedDevice(deviceId);
  if (certificate && account && certificate.accountKeyId !== account.accountKeyId && !owned) {
    const shared = await verifySharedDevice(deviceId, identityKey, certificate);
    await accountStore(`protected-device:${account.accountKeyId}:${deviceId}`, true);
    return { verified: true, bound: true, grantee: true, identityTrust: shared.identityTrust };
  }
  const authority = account;
  if (certificate) {
    if (!authority) throw new AccountTrustError('accountUnlockRequired');
    if (!await verifyDeviceCertificate(certificate, authority, deviceId, identityKey)) throw new AccountTrustError('accountCertificateInvalid');
    if ((await accountRevocations()).some(r => r.kind === 'device' && r.subjectId === deviceId)) throw new AccountTrustError('accountDeviceRevoked');
    const previous = await accountStore<DeviceCertificate>(`device-certificate:${authority.accountKeyId}:${deviceId}`);
    const pinnedIdentity = pin?.identityKey || previous?.identityKey;
    if ((pin && !pinnedIdentity) || (pinnedIdentity && pinnedIdentity !== identityKey)) throw new AccountTrustError('accountDeviceIdentityChanged');
    await rememberCertifiedDevice(certificate, authority);
    await accountStore(`device-certificate:${authority.accountKeyId}:${deviceId}`, certificate);
    return { verified: true, bound: true, grantee: false, identityTrust: 'certificate' as const };
  }
  if (expected || hint?.bound || await hasAccountDeviceCertificate(deviceId)) {
    if (!account) throw new AccountTrustError('accountUnlockRequired');
    throw new AccountTrustError('accountCertificateMissing');
  }
  return { verified: false, bound: false, grantee: false, identityTrust: 'tofu' as const };
}
export async function hasAccountDeviceCertificate(deviceId: string) {
  if (await ownerAccountPin(deviceId)) return true;
  const a = await currentAccount(); return !!a && (!!await accountStore(`device-certificate:${a.accountKeyId}:${deviceId}`) || !!await accountStore(`protected-device:${a.accountKeyId}:${deviceId}`) || (await locallyCertifiedDevices()).includes(deviceId));
}

// Descriptor verification in exchange establishes the recipient role. The
// portal probes a thread-scoped endpoint for accounts other than its owner.
export async function verifySharedDeviceRole(deviceId: string, _scope?: string) {
  if ((await boundDevices()).includes(deviceId) || await isOwnedDevice(deviceId)) return false;
  const account = await currentAccount();
  if (!account) return false;
  try {
    const owner = await accountRequest<{ accountKeyId: string }>(`/relay/devices/${encodeURIComponent(deviceId)}/owner-account`);
    return owner.accountKeyId !== account.accountKeyId;
  } catch { return false; }
}
