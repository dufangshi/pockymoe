import { clearMetrics } from '../devices/cache';
import { relayOrigin } from '../../lib/relayOrigin';
import type { DesktopIndex } from './types';
const name = 'pockymoe-desktop-index';
const key = (accountId: string) => `${location.origin}:${relayOrigin()}:${accountId}`;
function database(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(name, 1);
    request.onupgradeneeded = () => request.result.createObjectStore('accounts');
    request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error);
  });
}
export async function readIndex(accountId: string): Promise<DesktopIndex | null> {
  try {
    const db = await database();
    return await new Promise((resolve, reject) => {
      const tx = db.transaction('accounts'); const request = tx.objectStore('accounts').get(key(accountId));
      request.onsuccess = () => resolve((request.result as DesktopIndex | undefined) ?? null);
      request.onerror = () => reject(request.error); tx.oncomplete = () => db.close();
    });
  } catch { return null; }
}
export async function writeIndex(index: DesktopIndex) {
  try {
    const db = await database();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('accounts', 'readwrite');
      tx.objectStore('accounts').put({ ...index, refreshing: false, error: null, pulseKeys: [] }, key(index.accountId));
      tx.oncomplete = () => { db.close(); resolve(); }; tx.onerror = () => { db.close(); reject(tx.error); };
    });
  } catch { /* Memory remains usable when IndexedDB is unavailable. */ }
}
export async function clearIndex(accountId: string) {
  await clearMetrics(accountId);
  try {
    const db = await database();
    await new Promise<void>(resolve => {
      const tx = db.transaction('accounts', 'readwrite'); tx.objectStore('accounts').delete(key(accountId));
      tx.oncomplete = tx.onerror = () => { db.close(); resolve(); };
    });
  } catch { /* Nothing to clear when storage is disabled. */ }
}
