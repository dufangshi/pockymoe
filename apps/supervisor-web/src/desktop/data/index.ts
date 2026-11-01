import { createContext, useContext, useRef, useSyncExternalStore } from 'react';
import type { DesktopIndex } from './types';
import { DesktopIndexStore } from './store';
export * from './types';
export * from './store';
export * from './merge';
export const DesktopStoreContext = createContext<DesktopIndexStore | null>(null);
export function useDesktopStore() {
  const store = useContext(DesktopStoreContext); if (!store) throw new Error('Desktop store is required.'); return store;
}
export function useDesktopSelector<T>(selector: (index: DesktopIndex) => T, equal: (a: T, b: T) => boolean = Object.is): T {
  const store = useDesktopStore(); const current = useRef<{ value: T } | null>(null);
  const snapshot = () => {
    const value = selector(store.getSnapshot());
    if (!current.current || !equal(current.current.value, value)) current.current = { value };
    return current.current.value;
  };
  return useSyncExternalStore(store.subscribe, snapshot, snapshot);
}
