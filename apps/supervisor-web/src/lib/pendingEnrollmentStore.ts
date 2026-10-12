// Pending device enrollments, kept free of transport imports so the relay
// service worker can read them (it cannot use dynamic imports).
import type { AccountKeyInfo, DeviceCertificate } from './accountVault';

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
const changed = () => {
  if (typeof window !== 'undefined')
    window.dispatchEvent(new Event(ENROLLMENT_CHANGED_EVENT));
};
export async function store<T>(
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
const sameEnrollment = (
  a: PendingEnrollment | undefined,
  b: PendingEnrollment,
) =>
  a?.deviceId === b.deviceId &&
  a.userId === b.userId &&
  a.accountKeyId === b.accountKeyId &&
  a.createdAt === b.createdAt &&
  a.enrollSecret === b.enrollSecret;
export async function pendingIsCurrent(pending: PendingEnrollment) {
  return (await listPendingEnrollments()).some((p) =>
    sameEnrollment(p, pending),
  );
}
export async function savePending(
  pending: PendingEnrollment,
  expected?: PendingEnrollment,
) {
  const saved = await store<boolean>((s, done) => {
    if (!expected) {
      s.put(pending);
      done(true);
      return;
    }
    const request = s.get(pending.deviceId);
    request.onsuccess = () => {
      const current = sameEnrollment(request.result, expected);
      if (current) s.put(pending);
      done(current);
    };
  });
  if (saved) changed();
  return saved;
}
export async function deletePendingEnrollment(
  deviceId: string,
  expected?: PendingEnrollment,
) {
  const deleted = await store<boolean>((s, done) => {
    if (!expected) {
      s.delete(deviceId);
      done(true);
      return;
    }
    const request = s.get(deviceId);
    request.onsuccess = () => {
      const current = sameEnrollment(request.result, expected);
      if (current) s.delete(deviceId);
      done(current);
    };
  });
  if (deleted) changed();
  return deleted;
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
