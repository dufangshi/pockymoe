import { useCallback, useContext, useRef, useSyncExternalStore } from 'react';
import { documentRenders, subscribeWhenRendering } from '../../lib/renderingLifecycle';
import type { DesktopIndex } from './types';
import { DesktopIndexStore } from './store';
export * from './types';
export * from './store';
export * from './merge';
export { DesktopStoreContext } from './context';
import { DesktopStoreContext } from './context';
export function useDesktopStore() {
  const store = useContext(DesktopStoreContext); if (!store) throw new Error('Desktop store is required.'); return store;
}
export function useDesktopSelector<T>(selector: (index: DesktopIndex) => T, equal: (a: T, b: T) => boolean = Object.is, enabled = true): T {
  const store = useDesktopStore(); const current = useRef<{ value: T } | null>(null);
  const snapshot = () => {
    if (current.current && (!enabled || !documentRenders())) return current.current.value;
    const value = selector(store.getSnapshot());
    if (!current.current || !equal(current.current.value, value)) current.current = { value };
    return current.current.value;
  };
  const subscribe = useCallback((changed: () => void) => enabled ? subscribeWhenRendering(store.subscribe, changed, true) : () => {}, [store, enabled]);
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}
