import { webcrypto } from 'node:crypto';
import { beforeAll, expect, it } from 'vitest';
import fixture from '../../../../tests/fixtures/public-link-v2.json';
import type { PublicTranscriptSnapshot } from '@pockymoe/thread-ui';
import { createPublicLinkKey, decryptPublicSnapshot, encryptPublicSnapshot, forgetPublicLinkKey, publicLinkUrl, rememberPublicLinkKey } from './publicLinkCrypto';

beforeAll(() => { Object.defineProperty(globalThis, 'crypto', { configurable: true, value: webcrypto }); });
const snapshot = fixture.snapshot as PublicTranscriptSnapshot;

it('decrypts the shared Rust/WebCrypto vector including title, theme and images', async () => {
  expect(await decryptPublicSnapshot({ ...fixture.envelope, version: 2 }, `#k=${fixture.key}`)).toEqual({ ...snapshot, stale: false });
});
it('round trips full snapshots with fresh keys and IVs and preserves relay staleness', async () => {
  const key = createPublicLinkKey();
  expect(key).toMatch(/^[\w-]{43}$/);
  const one = await encryptPublicSnapshot(snapshot, key);
  const two = await encryptPublicSnapshot(snapshot, key);
  expect(one.iv).not.toBe(two.iv);
  expect(Object.keys(one)).toEqual(['version', 'iv', 'ciphertext']);
  expect(JSON.stringify(one)).not.toContain(snapshot.title);
  expect(await decryptPublicSnapshot({ ...one, stale: true }, `#k=${key}`)).toEqual({ ...snapshot, stale: true });
});
it('requires a fragment key and rejects wrong keys, tampering, invalid encoding and versions', async () => {
  const envelope = await encryptPublicSnapshot(snapshot, fixture.key);
  await expect(decryptPublicSnapshot(envelope, '')).rejects.toMatchObject({ reason: 'missing' });
  for (const key of [createPublicLinkKey(), 'invalid+/=', 'AA'])
    await expect(decryptPublicSnapshot(envelope, `#k=${key}`)).rejects.toMatchObject({ reason: 'wrong' });
  for (const invalid of [
    { ...envelope, iv: 'AA' },
    { ...envelope, version: 3 },
    { ...envelope, ciphertext: 'invalid+/=' },
    { ...envelope, ciphertext: (envelope.ciphertext[0] === 'A' ? 'B' : 'A') + envelope.ciphertext.slice(1) },
  ]) await expect(decryptPublicSnapshot(invalid as never, `#k=${fixture.key}`)).rejects.toMatchObject({ reason: 'wrong' });
});
it('keeps version 1 and unversioned plaintext links readable without a fragment', async () => {
  expect(await decryptPublicSnapshot(snapshot, '')).toBe(snapshot);
  const legacy = { ...snapshot, version: 1 };
  expect(await decryptPublicSnapshot(legacy, '')).toBe(legacy);
});
it('stores fragment keys per link, keeps legacy URLs and withholds URLs without a key', () => {
  const id = 'test-key-storage';
  forgetPublicLinkKey(id);
  expect(publicLinkUrl(id, 2)).toBeNull();
  expect(publicLinkUrl(id, 1)).toBe(`${location.origin}/s/${id}`);
  rememberPublicLinkKey(id, fixture.key);
  expect(localStorage.getItem(`pockymoe-public-link-key:${id}`)).toBe(fixture.key);
  expect(publicLinkUrl(id, 2)).toBe(`${location.origin}/s/${id}#k=${fixture.key}`);
  forgetPublicLinkKey(id);
  expect(publicLinkUrl(id, 2)).toBeNull();
});
