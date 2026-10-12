import { translate } from '@pockymoe/thread-ui/i18n';
import {
  currentAccount,
  boundDeviceIdentities,
  confirmBoundDeviceIdentity,
  requireUnlock,
  signingInput,
  verifyDeviceCertificate,
  type AccountKeyInfo,
  type DeviceCertificate,
  type UnlockedAccount,
  type Revocation,
} from './accountVault';
import { accountStore } from './accountStore';
import { sharingTrustStore } from './sharingTrustStore';
import { deviceTrustRequest } from './deviceTrust';
import * as transport from './relayTransportCrypto';
import { relayUrl } from './relayOrigin';
import {
  ENROLLMENT_CHANGED_EVENT,
  ENROLLMENT_LIFETIME_MS,
  deletePendingEnrollment,
  listPendingEnrollments,
  pendingIsCurrent,
  savePending,
  type PendingEnrollment,
} from './pendingEnrollmentStore';

export {
  ENROLLMENT_CHANGED_EVENT,
  ENROLLMENT_LIFETIME_MS,
  deletePendingEnrollment,
  listPendingEnrollments,
  type PendingEnrollment,
};

export interface EnrollmentDescriptor {
  identityKey: string;
  certificate?: DeviceCertificate | null;
  trust?: { bound: boolean; strict: boolean; accountKeyId?: string | null };
}
export interface EnrollmentResponse {
  deviceId: string;
  identityKey: string;
  accountKeyId: string;
  mac: string;
}
const encode = (bytes: Uint8Array) =>
  btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
const decode = (text: string) =>
  Uint8Array.from(atob(text.replace(/-/g, '+').replace(/_/g, '/')), (c) =>
    c.charCodeAt(0),
  );
export async function createPendingEnrollment(
  deviceId: string,
  account: AccountKeyInfo,
  now = Date.now(),
) {
  const pending: PendingEnrollment = {
    deviceId,
    userId: account.userId,
    accountKeyId: account.accountKeyId,
    createdAt: now,
    enrollSecret: encode(crypto.getRandomValues(new Uint8Array(32))),
  };
  await savePending(pending);
  return pending;
}
export async function prepareDeviceEnrollment(
  deviceId: string,
  options?: { fresh?: boolean },
) {
  const account = await requireUnlock('bind-device');
  const existing = (await listPendingEnrollments(account)).find(
    (p) => p.deviceId === deviceId,
  );
  return {
    accountPublicKey: account.accountPublicKey,
    ...(!options?.fresh && existing
      ? existing
      : await createPendingEnrollment(deviceId, account)),
  };
}

/** The transport implementation provides this bootstrap handshake without TOFU. */
export async function inspectEnrollmentDevice(
  deviceId: string,
): Promise<EnrollmentDescriptor> {
  const inspect = (
    transport as unknown as {
      inspectEnrollmentDevice: (id: string) => Promise<EnrollmentDescriptor>;
    }
  ).inspectEnrollmentDevice;
  return inspect(deviceId);
}
export async function verifyEnrollmentMac(
  pending: PendingEnrollment,
  response: EnrollmentResponse,
) {
  if (
    response.deviceId !== pending.deviceId ||
    response.accountKeyId !== pending.accountKeyId
  )
    return false;
  try {
    const secret = decode(pending.enrollSecret);
    const mac = decode(response.mac);
    if (secret.length !== 32 || mac.length !== 32) return false;
    const key = await crypto.subtle.importKey(
      'raw',
      secret,
      { name: 'HMAC', hash: 'SHA-256' },
      false,
      ['verify'],
    );
    // WebCrypto verifies HMAC without a JavaScript early-exit byte comparison.
    return await crypto.subtle.verify(
      'HMAC',
      key,
      mac,
      new TextEncoder().encode(
        `pockymoe-acct-enroll-v1\n${response.deviceId}\n${response.identityKey}\n${response.accountKeyId}`,
      ),
    );
  } catch {
    return false;
  }
}
const active = new Map<string, Promise<boolean>>();
export interface EnrollmentIdentityReplacement {
  previousIdentityKey: string;
  identityKey: string;
}
export class EnrollmentIdentityChangeError
  extends Error
  implements EnrollmentIdentityReplacement
{
  constructor(
    public deviceId: string,
    public previousIdentityKey: string,
    public identityKey: string,
  ) {
    super(translate('devices.confirmIdentityReplacement'));
  }
}
export function completeDeviceEnrollment(
  pending: PendingEnrollment,
  online: boolean,
  confirmedReplacement?: EnrollmentIdentityReplacement,
): Promise<boolean> {
  if (!online) return Promise.resolve(false);
  const running = active.get(pending.deviceId);
  if (running) return running;
  const task = complete(pending, confirmedReplacement);
  active.set(pending.deviceId, task);
  void task.then(
    () => active.delete(pending.deviceId),
    () => active.delete(pending.deviceId),
  );
  return task;
}
async function complete(
  pending: PendingEnrollment,
  confirmedReplacement?: EnrollmentIdentityReplacement,
) {
  if (Date.now() - pending.createdAt >= ENROLLMENT_LIFETIME_MS) {
    await deletePendingEnrollment(pending.deviceId, pending);
    return false;
  }
  const account = await currentAccount();
  if (
    !account ||
    account.accountKeyId !== pending.accountKeyId ||
    account.userId !== pending.userId
  )
    return false;
  const revoked =
    (await accountStore<Revocation[]>(`revocations:${account.userId}`)) ?? [];
  if (
    revoked.some(
      (r) =>
        r.accountKeyId === account.accountKeyId &&
        r.kind === 'device' &&
        r.subjectId === pending.deviceId,
    )
  ) {
    await deletePendingEnrollment(pending.deviceId, pending);
    return false;
  }
  if (!(await pendingIsCurrent(pending))) return false;
  const descriptor = await inspectEnrollmentDevice(pending.deviceId);
  const previous = (await boundDeviceIdentities()).find(
    (d) => d.deviceId === pending.deviceId,
  );
  const replacing =
    !!previous && previous.identityKey !== descriptor.identityKey;
  const confirmReplacement = async () => {
    if (!replacing) return;
    if (
      confirmedReplacement?.previousIdentityKey !== previous!.identityKey ||
      confirmedReplacement?.identityKey !== descriptor.identityKey
    )
      throw new EnrollmentIdentityChangeError(
        pending.deviceId,
        previous!.identityKey,
        descriptor.identityKey,
      );
    await confirmBoundDeviceIdentity(
      pending.deviceId,
      descriptor.identityKey,
      previous!.identityKey,
    );
  };
  let certificate = descriptor.certificate;
  let unlocked: UnlockedAccount | undefined;
  if (
    !certificate ||
    !(await verifyDeviceCertificate(
      certificate,
      account,
      pending.deviceId,
      descriptor.identityKey,
    ))
  ) {
    const enrollment = await deviceTrustRequest<EnrollmentResponse>(
      pending.deviceId,
      'enrollment',
    );
    if (
      enrollment.identityKey !== descriptor.identityKey ||
      !(await verifyEnrollmentMac(pending, enrollment))
    )
      throw new Error(translate('devices.enrollmentMismatch'));
    if (!(await pendingIsCurrent(pending))) return false;
    // Authenticate the enrollment before offering any identity replacement.
    await confirmReplacement();
    unlocked = await requireUnlock('certify-device');
    if (!(await pendingIsCurrent(pending))) return false;
    if (
      unlocked.accountKeyId !== pending.accountKeyId ||
      unlocked.userId !== pending.userId
    )
      throw new Error(translate('devices.enrollmentMismatch'));
    const fields = {
      accountKeyId: pending.accountKeyId,
      deviceId: pending.deviceId,
      identityKey: descriptor.identityKey,
      createdAt: Date.now(),
    };
    certificate = {
      version: 1,
      ...fields,
      signature: await unlocked.sign(signingInput.device(fields)),
    };
    if (!(await savePending({ ...pending, certificate }, pending)))
      return false;
    const installed = await deviceTrustRequest<{
      certificate: DeviceCertificate;
    }>(pending.deviceId, 'device-certificate', 'PUT', certificate);
    if (
      !(await verifyDeviceCertificate(
        installed.certificate,
        account,
        pending.deviceId,
        descriptor.identityKey,
      ))
    )
      throw new Error(translate('devices.enrollmentMismatch'));
    certificate = installed.certificate;
  } else await confirmReplacement();
  if (!(await pendingIsCurrent(pending))) return false;
  unlocked ??= await requireUnlock('certify-device');
  if (
    unlocked.accountKeyId !== account.accountKeyId ||
    unlocked.userId !== account.userId
  )
    throw new Error(translate('devices.enrollmentMismatch'));
  if (!(await pendingIsCurrent(pending))) return false;
  const response = await fetch(
    relayUrl(
      `/relay/devices/${encodeURIComponent(pending.deviceId)}/certificate`,
    ),
    {
      method: 'PUT',
      credentials: 'include',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(certificate),
    },
  );
  if (!response.ok) {
    const error = new Error(translate('devices.enrollmentUploadFailed'));
    throw Object.assign(error, { status: response.status });
  }
  if (!(await pendingIsCurrent(pending))) return false;
  if (!replacing)
    await confirmBoundDeviceIdentity(
      pending.deviceId,
      descriptor.identityKey,
      previous?.identityKey ?? null,
    );
  transport.clearTransportKeyCache(pending.deviceId);
  // Only a live owner response may establish Protected in the transport status.
  await deviceTrustRequest(pending.deviceId, 'trust');
  return deletePendingEnrollment(pending.deviceId, pending);
}

export type DeviceProtectionState =
  | 'protected'
  | 'not-protected'
  | 'pending'
  | 'certificate-missing'
  | 'unknown';
export async function deviceProtectionState(
  deviceId: string,
  descriptor: EnrollmentDescriptor | null,
  pending: PendingEnrollment[],
  bound: string[],
  account: AccountKeyInfo | null,
): Promise<DeviceProtectionState> {
  if (
    descriptor?.certificate &&
    account &&
    (await verifyDeviceCertificate(
      descriptor.certificate,
      account,
      deviceId,
      descriptor.identityKey,
    ))
  )
    return 'protected';
  if (bound.includes(deviceId)) return 'certificate-missing';
  if (
    pending.some(
      (p) =>
        p.deviceId === deviceId &&
        p.accountKeyId === account?.accountKeyId &&
        p.userId === account?.userId,
    )
  )
    return 'pending';
  if (descriptor?.trust?.bound || descriptor?.certificate)
    return 'certificate-missing';
  return descriptor ? 'not-protected' : 'unknown';
}
export async function revokeDevice(deviceId: string) {
  const account = await requireUnlock('revoke');
  const fields = {
    accountKeyId: account.accountKeyId,
    kind: 'device' as const,
    subjectId: deviceId,
    revokedAt: Date.now(),
  };
  const revocation = {
    version: 1,
    ...fields,
    signature: await account.sign(signingInput.revocation(fields)),
  };
  const response = await fetch(relayUrl('/relay/account/revocations'), {
    method: 'POST',
    credentials: 'include',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ revocation }),
  });
  if (!response.ok)
    throw new Error(translate('devices.deviceRevocationFailed'));
  // The signed tombstone is permanent even if the later vault CAS fails.
  const saved =
    (await accountStore<Revocation[]>(`revocations:${account.userId}`)) ?? [];
  await accountStore(`revocations:${account.userId}`, [...saved, revocation]);
  const certificateKey = `device-certificate:${account.accountKeyId}:${deviceId}`;
  await Promise.all([
    accountStore(certificateKey, null),
    accountStore(`protected-device:${account.accountKeyId}:${deviceId}`, null),
    sharingTrustStore(certificateKey, null),
  ]);
  transport.clearTransportKeyCache(deviceId);
  if (typeof window !== 'undefined') {
    window.dispatchEvent(
      new CustomEvent('pockymoe-account-revocations-changed', {
        detail: { revocation },
      }),
    );
  }
  // A committed revocation ends enrollment even if the subsequent vault CAS fails.
  await deletePendingEnrollment(deviceId);
  await account.updateBoundDevices((ids) =>
    ids.filter((id) => id !== deviceId),
  );
}
