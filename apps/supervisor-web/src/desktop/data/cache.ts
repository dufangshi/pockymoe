import type { DesktopIndex } from './types';

const databaseName = 'rc-desktop-index';
function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(databaseName, 1);
    request.onupgradeneeded = () => request.result.createObjectStore('accounts');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
const cacheKey = (accountId: string) => `${location.origin}:${accountId}`;
export async function readIndex(accountId: string): Promise<DesktopIndex | null> {
  try {
    const db = await openDatabase();
    return await new Promise((resolve, reject) => {
      const tx = db.transaction('accounts');
      const request = tx.objectStore('accounts').get(cacheKey(accountId));
      request.onsuccess = () => resolve((request.result as DesktopIndex | undefined) ?? null);
      request.onerror = () => reject(request.error);
      tx.oncomplete = () => db.close();
    });
  } catch { return null; }
}
export async function writeIndex(index: DesktopIndex) {
  try {
    const db = await openDatabase();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('accounts', 'readwrite');
      tx.objectStore('accounts').put({ ...index, refreshing: false, error: null }, cacheKey(index.accountId));
      tx.oncomplete = () => { db.close(); resolve(); };
      tx.onerror = () => { db.close(); reject(tx.error); };
    });
  } catch { /* The in-memory index still works if browser storage is unavailable. */ }
}
export async function clearIndex(accountId: string) {
  try {
    const db = await openDatabase();
    await new Promise<void>((resolve) => {
      const tx = db.transaction('accounts', 'readwrite');
      tx.objectStore('accounts').delete(cacheKey(accountId));
      tx.oncomplete = () => { db.close(); resolve(); };
      tx.onerror = () => { db.close(); resolve(); };
    });
  } catch { /* No persisted cache to clear. */ }
}
