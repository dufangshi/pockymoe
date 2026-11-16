import { useCallback, useLayoutEffect, useRef, useState, type SetStateAction } from 'react';
import { cancelRenderedUpdate, documentRenders, scheduleRenderedUpdate, subscribeWhenRendering } from './renderingLifecycle';

/** A visual readout keeps receiving its latest value while its view is paused,
 * then catches up before reveal. It creates no timer or background commit. */
export function useRenderedState<T>(initial: T | (() => T), enabled = true, batch = true) {
  const [value, setValue] = useState<T>(initial);
  const latest = useRef(value), active = useRef(enabled);
  const applied = useRef(value);
  active.current = enabled;
  const apply = useCallback(() => {
    const next = latest.current;
    if (!active.current || !documentRenders() || Object.is(applied.current, next)) return;
    applied.current = next; setValue(() => next);
  }, []);
  const update = useCallback((action: SetStateAction<T>) => {
    const next = typeof action === 'function' ? (action as (current: T) => T)(latest.current) : action;
    if (Object.is(latest.current, next)) return;
    latest.current = next;
    if (active.current && documentRenders()) { if (batch) scheduleRenderedUpdate(apply); else apply(); }
  }, [apply, batch]);
  useLayoutEffect(() => {
    const cancel = () => cancelRenderedUpdate(apply);
    if (!enabled) { cancel(); return; }
    const reveal = () => { cancel(); apply(); };
    if (documentRenders()) reveal();
    return subscribeWhenRendering(() => cancel, reveal);
  }, [enabled, apply]);
  return [value, update] as const;
}
