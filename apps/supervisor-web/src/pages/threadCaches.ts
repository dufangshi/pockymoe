import type { ThreadDetailDto, ThreadDto, RelayEffectiveAccessDto } from '@pockymoe/shared';
import { sessionRevision, subscribeSessionActor } from '../lib/sessionActor';
function cacheMap<T>(revision: number) {
  const map = new Map<string, T>();
  const set = map.set.bind(map);
  map.set = (key, value) => { if (revision === sessionRevision()) set(key, value); return map; };
  return map;
}
function createCaches() {
  const revision = sessionRevision();
  return {
    isCurrent: () => revision === sessionRevision(),
    recentThreadDetails: cacheMap<ThreadDetailDto>(revision),
    pendingThreadDetails: cacheMap<Promise<ThreadDetailDto>>(revision),
    recentRelayAccess: cacheMap<RelayEffectiveAccessDto>(revision),
    lastThreadLists: cacheMap<ThreadDto[]>(revision),
  };
}
let caches = createCaches();
function invalidateIfChanged() {
  if (caches.isCurrent()) return;
  caches.recentThreadDetails.clear(); caches.pendingThreadDetails.clear();
  caches.recentRelayAccess.clear(); caches.lastThreadLists.clear();
  caches = createCaches();
}
subscribeSessionActor(invalidateIfChanged);
// Do not rely on subscriber ordering if a route starts rendering synchronously.
export const getThreadCaches = () => { invalidateIfChanged(); return caches; };
export function rememberThreadDetail(cache: ReturnType<typeof getThreadCaches>, key: string, detail: ThreadDetailDto) {
  if (!cache.isCurrent()) return;
  cache.recentThreadDetails.delete(key); cache.recentThreadDetails.set(key, detail);
  while (cache.recentThreadDetails.size > 12) cache.recentThreadDetails.delete(cache.recentThreadDetails.keys().next().value!);
}
