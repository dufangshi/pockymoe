/* Imported packs, per browser, in IndexedDB: a small `packs` store for listing and
   a `files` store with the pack's files. */

import type { ThemeManifest } from './manifest';

export interface StoredPackInfo {
  id: string;
  manifest: ThemeManifest;
  hash: string;
  importedAt: number;
}

export interface ThemePackStorage {
  list(): Promise<StoredPackInfo[]>;
  info(id: string): Promise<StoredPackInfo | undefined>;
  files(id: string): Promise<Map<string, Uint8Array> | undefined>;
  put(info: StoredPackInfo, files: Map<string, Uint8Array>): Promise<void>;
  remove(id: string): Promise<void>;
}

const DB_NAME = 'pockymoe-theme-packs';

function open() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => {
      request.result.createObjectStore('packs', { keyPath: 'id' });
      request.result.createObjectStore('files');
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function transaction<T>(stores: string[], mode: IDBTransactionMode, run: (tx: IDBTransaction) => IDBRequest | void): Promise<T> {
  const db = await open();
  try {
    return await new Promise<T>((resolve, reject) => {
      const tx = db.transaction(stores, mode);
      const request = run(tx);
      tx.oncomplete = () => resolve(request ? (request.result as T) : (undefined as T));
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error ?? new Error('IndexedDB transaction aborted'));
    });
  } finally {
    db.close();
  }
}

export const indexedDbThemePackStorage: ThemePackStorage = {
  list: () => transaction<StoredPackInfo[]>(['packs'], 'readonly', (tx) => tx.objectStore('packs').getAll()),
  info: (id) => transaction<StoredPackInfo | undefined>(['packs'], 'readonly', (tx) => tx.objectStore('packs').get(id)),
  async files(id) {
    const entries = await transaction<[string, Uint8Array][] | undefined>(['files'], 'readonly', (tx) => tx.objectStore('files').get(id));
    return entries ? new Map(entries) : undefined;
  },
  put: (info, files) =>
    transaction<void>(['packs', 'files'], 'readwrite', (tx) => {
      tx.objectStore('files').put([...files.entries()], info.id);
      tx.objectStore('packs').put(info);
    }),
  remove: (id) =>
    transaction<void>(['packs', 'files'], 'readwrite', (tx) => {
      tx.objectStore('files').delete(id);
      tx.objectStore('packs').delete(id);
    }),
};

/** An in-memory store, for tests and browsers without IndexedDB. */
export function memoryThemePackStorage(): ThemePackStorage {
  const packs = new Map<string, { info: StoredPackInfo; files: Map<string, Uint8Array> }>();
  return {
    list: async () => [...packs.values()].map((entry) => entry.info),
    info: async (id) => packs.get(id)?.info,
    files: async (id) => packs.get(id)?.files,
    put: async (info, files) => void packs.set(info.id, { info, files: new Map(files) }),
    remove: async (id) => void packs.delete(id),
  };
}
