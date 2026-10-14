import { supervisorPath } from './webBase';
import { translate } from '@pockymoe/thread-ui/i18n';
import {
  boundDevices,
  accountRevocations,
  currentAccount,
  verifyDeviceCertificate,
  type AccountKeyInfo,
  type DeviceCertificate,
} from './accountVault';
import { relayUrl } from './relayOrigin';
import { sharingTrustEntries, sharingTrustStore } from './sharingTrustStore';

export type PublicAccount = Pick<AccountKeyInfo, 'accountKeyId' | 'accountPublicKey'>;
const nativeFetch = globalThis.fetch.bind(globalThis);
export class SharingTrustError extends Error {
  constructor(public readonly code: 'account_mismatch' | 'invalid_certificate') {
    super(translate(code === 'account_mismatch'
      ? 'sharing.accountKeyMismatch'
      : 'sharing.deviceCertificateInvalid'));
  }
}
export function encode64(value: Uint8Array) {
  return btoa(String.fromCharCode(...value)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}
function decode64(value: string) {
  if (!/^[A-Za-z0-9_-]+$/.test(value) || value.length % 4 === 1)
    throw new SharingTrustError('account_mismatch');
  const result = Uint8Array.from(atob(value.replaceAll('-', '+').replaceAll('_', '/')), (c) => c.charCodeAt(0));
  if (encode64(result) !== value) throw new SharingTrustError('account_mismatch');
  return result;
}
export async function validatePublicAccount(account: PublicAccount): Promise<PublicAccount> {
  if (!account || typeof account.accountPublicKey !== 'string' || typeof account.accountKeyId !== 'string')
    throw new SharingTrustError('account_mismatch');
  const publicKey = decode64(account.accountPublicKey);
  const id = encode64(new Uint8Array(await crypto.subtle.digest('SHA-256', publicKey)));
  if (id !== account.accountKeyId) throw new SharingTrustError('account_mismatch');
  try {
    await crypto.subtle.importKey('spki', publicKey, { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
  } catch { throw new SharingTrustError('account_mismatch'); }
  return { accountKeyId: id, accountPublicKey: account.accountPublicKey };
}
const certificateKey = (accountKeyId: string, deviceId: string) =>
  `device-certificate:${accountKeyId}:${deviceId}`;
async function verifies(certificate: DeviceCertificate, account: PublicAccount, deviceId: string, identityKey: string) {
  // owner-account publishes only public key fields. userId is metadata and is
  // unused by certificate verification; it never becomes a wire principal.
  return verifyDeviceCertificate(certificate, { ...account, userId: '' }, deviceId, identityKey).catch(() => false);
}
export async function rememberCertifiedDevice(certificate: DeviceCertificate, account: PublicAccount) {
  const checked = await validatePublicAccount(account);
  if (!certificate || !(await verifies(certificate, checked, certificate.deviceId, certificate.identityKey)))
    throw new SharingTrustError('invalid_certificate');
  await sharingTrustStore(certificateKey(checked.accountKeyId, certificate.deviceId), certificate);
}
async function localOwnerCertificate(deviceId: string) {
  const account = await currentAccount();
  if (!account) return null;
  const certificate = await sharingTrustStore<DeviceCertificate>(certificateKey(account.accountKeyId, deviceId));
  if (!certificate) return null;
  if (!(await verifies(certificate, account, deviceId, certificate.identityKey)))
    throw new SharingTrustError('invalid_certificate');
  return certificate;
}
export async function locallyCertifiedDevices() {
  const account = await currentAccount();
  if (!account) return [];
  const prefix = certificateKey(account.accountKeyId, '');
  const devices: string[] = [];
  for (const [key, certificate] of await sharingTrustEntries<DeviceCertificate>(prefix)) {
    const deviceId = key.slice(prefix.length);
    if (!(await verifies(certificate, account, deviceId, certificate.identityKey)))
      throw new SharingTrustError('invalid_certificate');
    devices.push(deviceId);
  }
  return devices;
}
export async function forgetCertifiedDevice(deviceId: string) {
  const account = await currentAccount();
  if (account) await sharingTrustStore(certificateKey(account.accountKeyId, deviceId), null);
}
// The authenticated vault covers devices this browser has never visited. A
// verified revocation is the only reason to omit a previously trusted device.
export async function sharingGrantDevices() {
  const account = await currentAccount();
  if (!account) throw new SharingTrustError('invalid_certificate');
  const revoked = new Set((await accountRevocations(true))
    .filter((r) => r.accountKeyId === account.accountKeyId && r.kind === 'device')
    .map((r) => r.subjectId));
  // Filter revoked IDs before loading certificates: a removed device may retain
  // an obsolete local record, which must not block revocation elsewhere.
  const bound = await boundDevices();
  const prefix = certificateKey(account.accountKeyId, '');
  const certified = (await sharingTrustEntries<DeviceCertificate>(prefix))
    .filter(([key]) => !revoked.has(key.slice(prefix.length)));
  const ids = new Set(bound.filter((id) => !revoked.has(id)));
  for (const [key, certificate] of certified) {
    const id = key.slice(prefix.length);
    if (!(await verifies(certificate, account, id, certificate.identityKey)))
      throw new SharingTrustError('invalid_certificate');
    ids.add(id);
  }
  return [...ids];
}
async function deviceDescriptor(deviceId: string, threadId?: string) {
  const challenge = encode64(crypto.getRandomValues(new Uint8Array(32)));
  const prefix = threadId ? supervisorPath(`api/threads/${encodeURIComponent(threadId)}`) : supervisorPath('api');
  const response = await nativeFetch(relayUrl(`relay/devices/${encodeURIComponent(deviceId)}${prefix}/transport/key?challenge=${challenge}`), { credentials: 'include', cache: 'no-store' });
  if (response.status === 404) return null;
  if (response.status === 503 || response.status === 504) throw new Error(translate('sharing.protectedOnline'));
  if (!response.ok) throw new SharingTrustError('invalid_certificate');
  const descriptor = await response.json() as { challenge: string; identityKey: string; certificate: DeviceCertificate | null; trust?: { bound?: boolean } };
  if (descriptor.challenge !== challenge || typeof descriptor.identityKey !== 'string')
    throw new SharingTrustError('invalid_certificate');
  return descriptor;
}
export async function ownerDeviceCertificate(deviceId: string) {
  const saved = await localOwnerCertificate(deviceId);
  if (saved) return saved;
  const account = await currentAccount();
  const descriptor = await deviceDescriptor(deviceId);
  const certificate = descriptor?.certificate;
  if (certificate) {
    if (!account || !(await verifies(certificate, account, deviceId, descriptor!.identityKey)))
      throw new SharingTrustError('invalid_certificate');
    await rememberCertifiedDevice(certificate, account);
    return certificate;
  }
  if ((await boundDevices()).includes(deviceId) || descriptor?.trust?.bound) throw new SharingTrustError('invalid_certificate');
  return null;
}
export async function isProtectedSharingDevice(deviceId: string) {
  return !!(await ownerDeviceCertificate(deviceId));
}
export async function inviteOwnerPin(deviceId: string) {
  return sharingTrustStore<string>(`invite-owner:${deviceId}`);
}
// Public pins protect first contact even before the account vault is loaded.
export async function ownerAccountPin(deviceId: string) {
  const invite = await inviteOwnerPin(deviceId);
  const tofu = await sharingTrustStore<string>(`tofu-owner:${deviceId}`);
  if (invite && tofu && invite !== tofu) throw new SharingTrustError('account_mismatch');
  return invite ?? tofu;
}
export async function pinInviteOwner(deviceId: string, accountKeyId: string) {
  if (!/^[A-Za-z0-9_-]{43}$/.test(accountKeyId)) throw new SharingTrustError('account_mismatch');
  // A cached owner account and its bound-device list are stronger than an invite.
  if (await localOwnerCertificate(deviceId) || (await boundDevices()).includes(deviceId)) return;
  const saved = await ownerAccountPin(deviceId);
  if (saved && saved !== accountKeyId) throw new SharingTrustError('account_mismatch');
  await sharingTrustStore(`invite-owner:${deviceId}`, accountKeyId);
}
export async function forgetInviteOwner(deviceId: string) {
  await sharingTrustStore(`invite-owner:${deviceId}`, null);
  await sharingTrustStore(`tofu-owner:${deviceId}`, null);
}
export async function verifySharedDevice(deviceId: string, identityKey: string, certificate: DeviceCertificate | null): Promise<{ accountKeyId: string; identityTrust: 'invite' | 'tofu' }> {
  const response = await nativeFetch(relayUrl(`relay/devices/${encodeURIComponent(deviceId)}/owner-account`), { credentials: 'include', cache: 'no-store' });
  if (!response.ok) throw new SharingTrustError('account_mismatch');
  const account = await validatePublicAccount(await response.json() as PublicAccount);
  const pin = await inviteOwnerPin(deviceId);
  const tofu = await sharingTrustStore<string>(`tofu-owner:${deviceId}`);
  const savedPin = await ownerAccountPin(deviceId);
  if (savedPin && savedPin !== account.accountKeyId)
    throw new SharingTrustError('account_mismatch');
  if (!certificate || !(await verifies(certificate, account, deviceId, identityKey)))
    throw new SharingTrustError('invalid_certificate');
  await rememberCertifiedDevice(certificate, account);
  if (!pin && !tofu) await sharingTrustStore(`tofu-owner:${deviceId}`, account.accountKeyId);
  return { accountKeyId: account.accountKeyId, identityTrust: pin ? 'invite' : 'tofu' };
}
// Invite checks are unsigned: normal browser registration happens on destination
// requests. Certificate trust covers the descriptor identity without a passkey.
export async function checkSharedInviteDevice(deviceId: string, threadId?: string) {
  const saved = await localOwnerCertificate(deviceId);
  const expectedOwner = !!saved || (await boundDevices()).includes(deviceId);
  const descriptor = await deviceDescriptor(deviceId, threadId);
  if (!descriptor?.certificate) throw new SharingTrustError('invalid_certificate');
  const own = await currentAccount();
  if (expectedOwner || (own && descriptor.certificate.accountKeyId === own.accountKeyId)) {
    if (!own || !(await verifies(descriptor.certificate, own, deviceId, descriptor.identityKey)) ||
      (saved && saved.identityKey !== descriptor.identityKey))
      throw new SharingTrustError('invalid_certificate');
    await rememberCertifiedDevice(descriptor.certificate, own);
    return;
  }
  await verifySharedDevice(deviceId, descriptor.identityKey, descriptor.certificate);
}
export async function sharingOwnerAccountKeyId(deviceId: string) {
  const certificate = await ownerDeviceCertificate(deviceId);
  if (!certificate) throw new SharingTrustError('invalid_certificate');
  return certificate.accountKeyId;
}
