import { useSyncExternalStore } from 'react';

const subscribe = (changed: () => void) => {
  document.addEventListener('visibilitychange', changed);
  window.addEventListener('pockymoe-rendering-change', changed);
  return () => {
    document.removeEventListener('visibilitychange', changed);
    window.removeEventListener('pockymoe-rendering-change', changed);
  };
};
const snapshot = () => !document.hidden && document.documentElement.dataset.pockymoeRendering !== 'paused';

/** Visibility is event driven; hidden readouts must not keep a polling clock. */
export function useDocumentVisible() {
  return useSyncExternalStore(subscribe, snapshot, () => true);
}
