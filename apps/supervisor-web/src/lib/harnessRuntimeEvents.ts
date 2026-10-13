import type { ThreadEventEnvelope } from '@pockymoe/shared';

const listeners = new Set<() => void>();

/** Forwards harness load and release events from a page's Supervisor socket. */
export function announceHarnessRuntimeEvent(event: ThreadEventEnvelope) {
  const type: string = event.type;
  if (type === 'thread.runtime.loaded' || type === 'thread.runtime.released')
    listeners.forEach(listener => listener());
}

export function onHarnessRuntimeChange(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
