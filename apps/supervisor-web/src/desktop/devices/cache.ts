import type { DeviceMetrics } from './model';

type MetricsCache = Record<string, DeviceMetrics>;
const memory = new Map<string, MetricsCache>();
const key = (accountId: string, timeZone: string) => `${location.origin}:${accountId}:${timeZone}`;
function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open('rc-desktop-stats', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('accounts');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
export function warmMetrics(accountId: string, timeZone: string) { return memory.get(key(accountId, timeZone)) ?? {}; }
export async function readMetrics(accountId: string, timeZone: string): Promise<MetricsCache> {
  try {
    const db = await open();
    const result = await new Promise<MetricsCache>((resolve, reject) => {
      const tx = db.transaction('accounts');
      const request = tx.objectStore('accounts').get(key(accountId, timeZone));
      request.onsuccess = () => resolve(request.result ?? {});
      request.onerror = () => reject(request.error);
      tx.oncomplete = () => db.close();
    });
    memory.set(key(accountId, timeZone), result); return result;
  } catch { return warmMetrics(accountId, timeZone); }
}
export async function writeMetrics(accountId: string, timeZone: string, value: MetricsCache) {
  memory.set(key(accountId, timeZone), value);
  try {
    const db = await open();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('accounts', 'readwrite');
      // Latency only describes this visit; do not show an old probe as current.
      tx.objectStore('accounts').put(Object.fromEntries(Object.entries(value).map(([id, row]) => [id, { ...row, latencyMs: undefined }])), key(accountId, timeZone));
      tx.oncomplete = () => { db.close(); resolve(); };
      tx.onerror = () => { db.close(); reject(tx.error); };
    });
  } catch { /* Private browsing and quota limits leave the in-memory cache usable. */ }
}
export async function clearMetrics(accountId: string) {
  const prefix = `${location.origin}:${accountId}:`;
  for (const name of memory.keys()) if (name.startsWith(prefix)) memory.delete(name);
  try {
    const db = await open();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('accounts', 'readwrite');
      const request = tx.objectStore('accounts').openCursor(IDBKeyRange.bound(prefix, `${prefix}\uffff`));
      request.onsuccess = () => { const cursor = request.result; if (cursor) { cursor.delete(); cursor.continue(); } };
      tx.oncomplete = () => { db.close(); resolve(); };
      tx.onerror = () => { db.close(); reject(tx.error); };
    });
  } catch { /* Storage may be unavailable. */ }
}
