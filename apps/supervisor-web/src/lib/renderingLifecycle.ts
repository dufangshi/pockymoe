export const documentRenders = () => typeof document === 'undefined' ||
  (!document.hidden && document.documentElement.dataset.pockymoeRendering !== 'paused');

const pending = new Set<() => void>();
let frame: number | undefined;
export function scheduleRenderedUpdate(update: () => void) {
  pending.add(update);
  frame ??= window.requestAnimationFrame(() => {
    frame = undefined;
    const batch = [...pending]; pending.clear();
    batch.forEach(apply => apply());
  });
}
export function cancelRenderedUpdate(update: () => void) {
  pending.delete(update);
  if (!pending.size && frame !== undefined) { window.cancelAnimationFrame(frame); frame = undefined; }
}

/** UI subscriptions follow rendering presence. Data owners keep receiving their
 * events; a revealed view catches up once, without committing hidden trees. */
export function subscribeWhenRendering(subscribe: (changed: () => void) => () => void, changed: () => void, batch = false) {
  let stop: (() => void) | undefined;
  let listening = false;
  const deliver = () => { if (listening && documentRenders()) changed(); };
  const notify = () => {
    if (!batch) { deliver(); return; }
    scheduleRenderedUpdate(deliver);
  };
  const sync = (catchUp: boolean) => {
    if (documentRenders()) {
      if (!stop) { listening = true; stop = subscribe(notify); if (catchUp) changed(); }
    } else {
      listening = false;
      stop?.(); stop = undefined;
      cancelRenderedUpdate(deliver);
    }
  };
  const wake = () => sync(true);
  document.addEventListener('visibilitychange', wake);
  window.addEventListener('pockymoe-rendering-change', wake);
  sync(false);
  return () => {
    listening = false; stop?.(); cancelRenderedUpdate(deliver);
    document.removeEventListener('visibilitychange', wake);
    window.removeEventListener('pockymoe-rendering-change', wake);
  };
}
