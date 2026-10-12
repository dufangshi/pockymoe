import { translate } from '@pockymoe/thread-ui/i18n';
import {
  currentAccount,
  requireUnlock,
  signingInput,
  verifyDeviceCertificate,
  type AccountKeyInfo,
  type DeviceCertificate,
  type UnlockedAccount,
} from './accountVault';
import { deviceTrustRequest } from './deviceTrust';
import * as transport from './relayTransportCrypto';
import { relayUrl } from './relayOrigin';

export const ENROLLMENT_LIFETIME_MS = 7 * 24 * 60 * 60 * 1000;
export const ENROLLMENT_CHANGED_EVENT = 'pockymoe-enrollment-changed';
export interface PendingEnrollment {
  deviceId: string;
  userId: string;
  enrollSecret: string;
  accountKeyId: string;
  createdAt: number;
  /** Persisted before installation, so a consumed secret does not strand a retry. */
  certificate?: DeviceCertificate;
}
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
const changed = () => {
  if (typeof window !== 'undefined')
    window.dispatchEvent(new Event(ENROLLMENT_CHANGED_EVENT));
};
async function store<T>(
  action: (store: IDBObjectStore, done: (value: T) => void) => void,
): Promise<T> {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const open = indexedDB.open('pockymoe-device-enrollments-v2', 1);
    open.onupgradeneeded = () =>
      open.result.createObjectStore('pending', { keyPath: 'deviceId' });
    open.onerror = () => reject(open.error);
    open.onsuccess = () => resolve(open.result);
  });
  try {
    return await new Promise<T>((resolve, reject) => {
      const tx = db.transaction('pending', 'readwrite');
      let result: T;
      action(tx.objectStore('pending'), (value) => {
        result = value;
      });
      tx.oncomplete = () => resolve(result);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}
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
async function savePending(pending: PendingEnrollment) {
  await store<void>((s, done) => {
    s.put(pending);
    done(undefined);
  });
  changed();
}
export async function deletePendingEnrollment(deviceId: string) {
  await store<void>((s, done) => {
    s.delete(deviceId);
    done(undefined);
  });
  changed();
}
export async function listPendingEnrollments(
  account?: AccountKeyInfo | null,
  now = Date.now(),
) {
  return store<PendingEnrollment[]>((s, done) => {
    const request = s.getAll();
    request.onsuccess = () => {
      const live: PendingEnrollment[] = [];
      for (const pending of request.result as PendingEnrollment[]) {
        if (now - pending.createdAt >= ENROLLMENT_LIFETIME_MS)
          s.delete(pending.deviceId);
        else if (
          !account ||
          (pending.userId === account.userId &&
            pending.accountKeyId === account.accountKeyId)
        )
          live.push(pending);
      }
      done(live);
    };
  });
}
export async function prepareDeviceEnrollment(deviceId: string) {
  const account = await requireUnlock('bind-device');
  const existing = (await listPendingEnrollments(account)).find(
    (p) => p.deviceId === deviceId,
  );
  return {
    accountPublicKey: account.accountPublicKey,
    ...(existing ?? (await createPendingEnrollment(deviceId, account))),
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
export function completeDeviceEnrollment(
  pending: PendingEnrollment,
  online: boolean,
): Promise<boolean> {
  if (!online) return Promise.resolve(false);
  const running = active.get(pending.deviceId);
  if (running) return running;
  const task = complete(pending);
  active.set(pending.deviceId, task);
  void task.then(
    () => active.delete(pending.deviceId),
    () => active.delete(pending.deviceId),
  );
  return task;
}
async function complete(pending: PendingEnrollment) {
  if (Date.now() - pending.createdAt >= ENROLLMENT_LIFETIME_MS) {
    await deletePendingEnrollment(pending.deviceId);
    return false;
  }
  const account = await currentAccount();
  if (
    !account ||
    account.accountKeyId !== pending.accountKeyId ||
    account.userId !== pending.userId
  )
    return false;
  const descriptor = await inspectEnrollmentDevice(pending.deviceId);
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
    unlocked = await requireUnlock('certify-device');
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
    await savePending({ ...pending, certificate });
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
  }
  unlocked ??= await requireUnlock('certify-device');
  if (
    unlocked.accountKeyId !== account.accountKeyId ||
    unlocked.userId !== account.userId
  )
    throw new Error(translate('devices.enrollmentMismatch'));
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
  await unlocked.updateBoundDevices((ids) => [
    ...new Set([...ids, pending.deviceId]),
  ]);
  transport.clearTransportKeyCache(pending.deviceId);
  const status = {
    deviceId: pending.deviceId,
    state: 'encrypted' as const,
    paired: true,
    bound: true,
    strict: true,
    certificateVerified: true,
    identityTrust: 'certificate' as const,
    identityKey: descriptor.identityKey,
    accountKeyId: account.accountKeyId,
  };
  transport.reportTransport(status);
  await deletePendingEnrollment(pending.deviceId);
  return true;
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
  await account.updateBoundDevices((ids) =>
    ids.filter((id) => id !== deviceId),
  );
  await deletePendingEnrollment(deviceId);
}
