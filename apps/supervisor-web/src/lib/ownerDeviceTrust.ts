import { relayUrl } from './relayOrigin';
import { OwnerTrustError, verifiedCredential } from './ownerSession';
import {
  certificateEntries,
  verifyCertificate,
  type CertificateDocument,
  type DeviceCertificate,
} from './ownerTrustCrypto';
import { trustStore } from './ownerTrustStore';
const nativeFetch = globalThis.fetch.bind(globalThis);
export async function saveDeviceCertificate(certificate: DeviceCertificate) {
  const record = await verifiedCredential(certificate.assertion.credentialId);
  if (!record || !(await verifyCertificate(certificate, record)))
    throw new OwnerTrustError('ownerCertificateInvalid');
  await trustStore(`certificate:${certificate.deviceId}`, certificate);
}
async function remoteDocument(
  deviceId: string,
): Promise<CertificateDocument | null> {
  const response = await nativeFetch(
    relayUrl(`/relay/devices/${encodeURIComponent(deviceId)}/certificate`),
    { credentials: 'include', cache: 'no-store' },
  );
  if (response.status === 404) return null;
  if (!response.ok) throw new OwnerTrustError('ownerCertificateUnavailable');
  const document = (await response.json()) as CertificateDocument;
  if (
    !document ||
    document.deviceId !== deviceId ||
    document.version !== 1 ||
    !Array.isArray(certificateEntries(document)) ||
    !certificateEntries(document).length
  )
    throw new OwnerTrustError('ownerCertificateInvalid');
  return document;
}
export async function deviceCertificate(deviceId: string) {
  const saved = await trustStore<DeviceCertificate>(`certificate:${deviceId}`);
  const document = await remoteDocument(deviceId);
  // Once paired, relay omission cannot downgrade the device to TOFU.
  if (!document) return saved ?? null;
  const candidates = certificateEntries(document);
  let trusted: DeviceCertificate | undefined;
  const unverifiedSigners: string[] = [];
  for (const certificate of candidates) {
    if (
      certificate?.deviceId !== deviceId ||
      certificate.version !== 1 ||
      !certificate.assertion
    )
      continue;
    const record = await verifiedCredential(certificate.assertion.credentialId);
    if (!record) {
      unverifiedSigners.push(certificate.assertion.credentialId);
      continue;
    }
    if (!(await verifyCertificate(certificate, record))) continue;
    if (
      (saved && saved.identityKey !== certificate.identityKey) ||
      (trusted && trusted.identityKey !== certificate.identityKey)
    )
      throw new OwnerTrustError('ownerIdentityMismatch');
    trusted = certificate;
  }
  if (!trusted) {
    if (unverifiedSigners.length)
      throw new OwnerTrustError(
        'owner_session_required',
        unverifiedSigners.length === 1
          ? unverifiedSigners[0]!
          : unverifiedSigners,
      );
    throw new OwnerTrustError('ownerCertificateInvalid');
  }
  await trustStore(`certificate:${deviceId}`, trusted);
  return trusted;
}
export async function appendDeviceCertificate(certificate: DeviceCertificate) {
  const record = await verifiedCredential(certificate.assertion.credentialId);
  if (!record || !(await verifyCertificate(certificate, record)))
    throw new OwnerTrustError('ownerCertificateInvalid');
  const known = await trustStore<DeviceCertificate>(
    `certificate:${certificate.deviceId}`,
  );
  if (known && known.identityKey !== certificate.identityKey)
    throw new OwnerTrustError('ownerIdentityMismatch');
  const previous = await remoteDocument(certificate.deviceId);
  const certificates = (
    previous ? certificateEntries(previous) : known ? [known] : []
  ).filter(
    (entry) =>
      entry.assertion.credentialId !== certificate.assertion.credentialId,
  );
  certificates.push(certificate);
  const response = await nativeFetch(
    relayUrl(`/relay/devices/${encodeURIComponent(certificate.deviceId)}/certificate`),
    {
      method: 'PUT',
      credentials: 'include',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        version: 1,
        deviceId: certificate.deviceId,
        certificates,
      }),
    },
  );
  if (!response.ok) throw new OwnerTrustError('ownerPairUpload');
  await saveDeviceCertificate(certificate);
}

export async function hasDeviceCertificate(deviceId: string) {
  return !!(await trustStore<DeviceCertificate>(`certificate:${deviceId}`)) || !!(await remoteDocument(deviceId));
}

// Relay access only selects the recipient UI path. It never proves an identity.
// A browser that already verified an owner certificate cannot be downgraded.
export async function isRecipientDevice(deviceId: string, threadId?: string, workspaceId?: string) {
  if (await trustStore<DeviceCertificate>(`certificate:${deviceId}`)) return false;
  const query = new URLSearchParams({ deviceId });
  if (threadId) query.set('threadId', threadId);
  if (workspaceId) query.set('workspaceId', workspaceId);
  const response = await nativeFetch(relayUrl(`/relay/access?${query}`), { credentials: 'include', cache: 'no-store' });
  if (!response.ok) return false;
  return (await response.json() as { kind?: string }).kind === 'shared';
}
export async function pinInviteIdentity(deviceId: string, fingerprint: string) {
  const saved = await trustStore<string>(`invite-pin:${deviceId}`);
  if (saved && saved !== fingerprint) throw new OwnerTrustError('ownerIdentityMismatch');
  await trustStore(`invite-pin:${deviceId}`, fingerprint);
}
export async function checkInviteIdentity(deviceId: string, fingerprint: string) {
  const expected = await trustStore<string>(`invite-pin:${deviceId}`);
  if (expected && expected !== fingerprint) throw new OwnerTrustError('ownerIdentityMismatch');
  return !!expected;
}

export async function inviteIdentityPin(deviceId: string) {
  return trustStore<string>(`invite-pin:${deviceId}`);
}
