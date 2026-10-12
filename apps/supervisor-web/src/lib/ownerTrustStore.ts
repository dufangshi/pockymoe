// CryptoKeys are structured-cloned into IDB, never exported or put in localStorage.
async function database() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open('pockymoe-owner-trust-v1', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('trust');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
export async function trustStore<T>(key: string): Promise<T | undefined>;
export async function trustStore<T>(
  key: string,
  value: T | null,
): Promise<T | undefined>;
export async function trustStore<T>(
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
