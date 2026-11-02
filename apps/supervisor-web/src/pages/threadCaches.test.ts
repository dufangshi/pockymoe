import { expect, it } from 'vitest';
import { observeSessionActor } from '../lib/sessionActor';
import { getThreadCaches, rememberThreadDetail } from './threadCaches';
it('clears details, pending reads, access and thread lists on sign-out before B can navigate Back', async () => {
  observeSessionActor('relay:A'); const a = getThreadCaches();
  const detail = { thread: { id: 'private-A' } } as never;
  rememberThreadDetail(a, 'device:thread', detail);
  a.lastThreadLists.set('device', [detail]); a.recentRelayAccess.set('device:thread', {} as never);
  let finish!: (value: never) => void;
  const pending = new Promise<never>(resolve => { finish = resolve; });
  a.pendingThreadDetails.set('device:thread', pending);
  const stale = pending.then(value => rememberThreadDetail(a, 'device:thread', value));
  observeSessionActor(null);
  for (const map of [a.recentThreadDetails, a.lastThreadLists, a.recentRelayAccess, a.pendingThreadDetails]) expect(map.size).toBe(0);
  observeSessionActor('relay:B'); finish(detail); await stale;
  expect(getThreadCaches().recentThreadDetails.get('device:thread')).toBeUndefined();
  expect(a.recentThreadDetails.size).toBe(0);
});
it('clears on direct A→B changes, without clearing on repeated session reads for A', () => {
  observeSessionActor('relay:A'); const a = getThreadCaches();
  rememberThreadDetail(a, 'device:thread', {} as never);
  observeSessionActor('relay:A'); expect(getThreadCaches()).toBe(a);
  observeSessionActor('relay:B'); expect(getThreadCaches()).not.toBe(a);
  expect(getThreadCaches().recentThreadDetails.size).toBe(0);
});

it('invalidates caches when another client window changes account', () => {
  observeSessionActor('relay:A'); const a = getThreadCaches();
  rememberThreadDetail(a, 'device:thread', {} as never);
  window.dispatchEvent(new StorageEvent('storage', { key: 'pockymoe-session-actor', newValue: JSON.stringify('relay:B') }));
  expect(a.recentThreadDetails.size).toBe(0);
  expect(getThreadCaches()).not.toBe(a);
});

it('also clears cached data on the existing browser sign-out/removal signal', () => {
  observeSessionActor('relay:A'); const a = getThreadCaches();
  rememberThreadDetail(a, 'device:thread', {} as never);
  window.dispatchEvent(new Event('pockymoe-account-logout'));
  expect(a.recentThreadDetails.size).toBe(0);
});
