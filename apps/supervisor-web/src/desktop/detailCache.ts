import type { ThreadDetailDto } from '@remote-codex/shared';
import type { DesktopIndexStore } from './data/store';

let current: DesktopIndexStore | null = null;
export function installDetailCache(store: DesktopIndexStore) {
  current = store;
  return () => { if (current === store) current = null; };
}
/** Consume a hover-prefetched first page once; subsequent refreshes hit the API. */
export function takePrefetchedDetail(deviceId: string, threadId: string): Promise<ThreadDetailDto> | null {
  return current?.takePrefetchedDetail(deviceId, threadId) ?? null;
}
