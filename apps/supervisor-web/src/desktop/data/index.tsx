/* eslint-disable react-refresh/only-export-components -- This module exports store hooks and a context, not UI components. */
import { createContext, useContext, useRef, useSyncExternalStore } from 'react';
import { DesktopIndexStore } from './store';
import type { DesktopIndex } from './types';
export * from './types';
export * from './store';

export const DesktopStoreContext = createContext<DesktopIndexStore | null>(null);
export function useDesktopStore() {
  const store = useContext(DesktopStoreContext);
  if (!store) throw new Error('DesktopStoreContext is required.');
  return store;
}
/** Selector snapshots preserve equality; a timer or another row cannot rerender this subscriber. */
export function useDesktopSelector<T>(selector: (index: DesktopIndex) => T, equal: (a: T, b: T) => boolean = Object.is): T {
  const store = useDesktopStore();
  const selected = useRef<{ value: T } | null>(null);
  const snapshot = () => {
    const value = selector(store.getSnapshot());
    if (!selected.current || !equal(selected.current.value, value)) selected.current = { value };
    return selected.current.value;
  };
  return useSyncExternalStore(store.subscribe, snapshot, snapshot);
}
