// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { clearStoredOwnerSession, trustStore } from './ownerTrustStore';

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
describe('persisted owner session removal', () => {
  it('removes a revoked key and every device mark in one read/write transaction while keeping credential trust', async () => {
    data.set('session', { sessionId: 'revoked' });
    data.set('registration:revoked:a', true);
    data.set('registration:revoked:b', true);
    data.set('credential:owner', { verified: true });
    await clearStoredOwnerSession('revoked');
    expect([...data.keys()]).toEqual(['credential:owner']);
    expect(modes).toEqual(['readwrite']);
    expect(close).toHaveBeenCalledTimes(1);
    expect(await trustStore('session')).toBeUndefined();
  });
  it('cleans only the revoked session marks if another tab has already unlocked a new key', async () => {
    data.set('session', { sessionId: 'new' });
    data.set('registration:old:a', true);
    data.set('registration:new:a', true);
    data.set('registration:older:a', true);
    await clearStoredOwnerSession('old');
    expect(await trustStore('session')).toEqual({ sessionId: 'new' });
    expect(data.has('registration:old:a')).toBe(false);
    expect(data.has('registration:new:a')).toBe(true);
    expect(data.has('registration:older:a')).toBe(true);
  });
  it('sign-out clears the key and registration marks left by any session', async () => {
    data.set('session', { sessionId: 'new' });
    data.set('registration:old:a', true);
    data.set('registration:new:b', true);
    data.set('credential:owner', { verified: true });
    await clearStoredOwnerSession();
    expect([...data.keys()]).toEqual(['credential:owner']);
  });
});
