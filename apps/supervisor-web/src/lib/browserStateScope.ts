import type { HostScopeDto } from '@pockymoe/shared';
import { useHostScope } from './hostCapabilities';
import { webBase } from './webBase';

/** Shared Treer proxy origins must never share navigation between mounts/hubs. */
export function browserStateKey(key: string, scope: HostScopeDto | null, mount = webBase) {
  return scope ? `${key}:${JSON.stringify([scope.hubId, mount])}` : key;
}
export function useBrowserStateKey(key: string) {
  return browserStateKey(key, useHostScope());
}
