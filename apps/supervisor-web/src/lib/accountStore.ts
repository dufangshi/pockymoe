import { relayOrigin } from './relayOrigin';
// CryptoKeys are structured-cloned. Account private keys and VDK never enter IDB.
function openAccountStore() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const req = indexedDB.open('pockymoe-account-v2', 1);
    req.onupgradeneeded = () => req.result.createObjectStore('account');
    req.onsuccess = () => resolve(req.result); req.onerror = () => reject(req.error);
  });
}
/** Includes caches written by browsers before the persistent migration guard. */
export async function hasStoredAccountVault() {
  const db = await openAccountStore();
  try { return await new Promise<boolean>((resolve, reject) => {
    const tx = db.transaction('account', 'readonly'), prefix = `${relayOrigin()}\ncache:`;
    const req = tx.objectStore('account').count(IDBKeyRange.bound(prefix, `${prefix}\uffff`));
    tx.oncomplete = () => resolve(req.result > 0); tx.onerror = tx.onabort = () => reject(tx.error);
  }); } finally { db.close(); }
}
export async function accountStore<T>(key: string): Promise<T | undefined>;
export async function accountStore<T>(key: string, value: T | null): Promise<T | undefined>;
export async function accountStore<T>(key: string, ...write: [T | null] | []) {
  const db = await openAccountStore();
  try { return await new Promise<T | undefined>((resolve, reject) => {
    const tx = db.transaction('account', write.length ? 'readwrite' : 'readonly');
    const store = tx.objectStore('account'), scoped = `${relayOrigin()}\n${key}`; let result: T | undefined;
    if (write.length) { if (write[0] === null) store.delete(scoped); else store.put(write[0], scoped); }
    else { const req = store.get(scoped); req.onsuccess = () => { result = req.result as T | undefined; }; }
    tx.oncomplete = () => resolve(result); tx.onerror = tx.onabort = () => reject(tx.error);
  }); } finally { db.close(); }
}
