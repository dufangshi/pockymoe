// Public sharing confirmations, owner-account pins and verified certificates only.
// Account and browser keys are owned exclusively by accountVault.
async function database() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open('pockymoe-sharing-trust-v2', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('trust');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
export async function sharingTrustStore<T>(key: string): Promise<T | undefined>;
export async function sharingTrustStore<T>(
  key: string,
  value: T | null,
): Promise<T | undefined>;
export async function sharingTrustStore<T>(
  key: string,
  ...write: [T | null] | []
): Promise<T | undefined> {
  const db = await database();
  try {
    return await new Promise<T | undefined>((resolve, reject) => {
      const tx = db.transaction(
        'trust',
        write.length ? 'readwrite' : 'readonly',
      );
      const store = tx.objectStore('trust');
      let value: T | undefined;
      if (write.length) {
        if (write[0] === null) store.delete(key);
        else store.put(write[0], key);
      } else {
        const request = store.get(key);
        request.onsuccess = () => {
          value = request.result as T | undefined;
        };
      }
      tx.oncomplete = () => resolve(value);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

// Enumerate local trust, never the relay's device inventory.
export async function sharingTrustEntries<T>(prefix: string): Promise<[string, T][]> {
  const db = await database();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction('trust', 'readonly');
      const entries: [string, T][] = [];
      const request = tx.objectStore('trust').openCursor();
      request.onsuccess = () => {
        const cursor = request.result;
        if (!cursor) return;
        if (typeof cursor.key === 'string' && cursor.key.startsWith(prefix))
          entries.push([cursor.key, cursor.value as T]);
        cursor.continue();
      };
      tx.oncomplete = () => resolve(entries);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally { db.close(); }
}
