// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { sharingTrustStore, sharingTrustEntries } from './sharingTrustStore';

// A minimal asynchronous IDB runtime; exercise the real transaction/cursor
// implementation without a browser profile or a dependency on installed IDB.
const data = new Map<string, unknown>();
const close = vi.fn();
const modes: IDBTransactionMode[] = [];
beforeEach(() => {
  data.clear();
  close.mockClear();
  modes.length = 0;
  vi.stubGlobal('indexedDB', {
    open: () => {
      const request: Record<string, any> = {};
      request.result = {
        close,
        transaction: (_name: string, mode: IDBTransactionMode) => {
          modes.push(mode);
          const tx: Record<string, any> = {};
          tx.objectStore = () => ({
            put: (value: unknown, key: string) => data.set(key, structuredClone(value)),
            delete: (key: string) => data.delete(key),
            get: (key: string) => {
              const read: Record<string, any> = {};
              queueMicrotask(() => {
                read.result = structuredClone(data.get(key));
                read.onsuccess?.();
              });
              return read;
            },
            openCursor: () => {
              const read: Record<string, any> = {};
              const keys = [...data.keys()].sort();
              let index = 0;
              const next = () => queueMicrotask(() => {
                const key = keys[index++];
                read.result = key === undefined ? null : {
                  key,
                  value: structuredClone(data.get(key)),
                  delete: () => data.delete(key),
                  continue: next,
                };
                read.onsuccess?.();
              });
              next();
              return read;
            },
          });
          setTimeout(() => tx.oncomplete?.(), 0);
          return tx;
        },
      };
      queueMicrotask(() => request.onsuccess?.());
      return request;
    },
  });
});

it('enumerates only the selected account certificate prefix without consulting relay inventory', async () => {
  data.set('device-certificate:account-a:hidden', { deviceId: 'hidden' });
  data.set('device-certificate:account-b:other', { deviceId: 'other' });
  data.set('recipient-confirmed:recipient', true);
  expect(await sharingTrustEntries('device-certificate:account-a:')).toEqual([['device-certificate:account-a:hidden', { deviceId: 'hidden' }]]);
  expect(modes).toEqual(['readonly']);
  expect(close).toHaveBeenCalledOnce();
});
it('persists public sharing confirmation independently of an invite pin and can delete the pin', async () => {
  await sharingTrustStore('recipient-confirmed:key', true);
  await sharingTrustStore('invite-owner:device', 'owner-key');
  expect(await sharingTrustStore('invite-owner:device')).toBe('owner-key');
  await sharingTrustStore('invite-owner:device', null);
  expect(await sharingTrustStore('invite-owner:device')).toBeUndefined();
  expect(await sharingTrustStore('recipient-confirmed:key')).toBe(true);
});
