import { useEffect } from 'react';
import type { DesktopIndexStore } from './data/store';
import { focusedKeyFromPath } from './integration';

/** Activity read marks follow the visible conversation in both shell surfaces. */
export function useThreadReadState(store: DesktopIndexStore, pathname: string) {
  useEffect(() => {
    const key = focusedKeyFromPath(pathname);
    if (!key) return;
    store.rememberThread(key);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const focus = () => { clearTimeout(timer); if (document.hasFocus()) timer = setTimeout(() => store.markRead(key), 1500); };
    const scroll = (event: Event) => {
      const target = event.target;
      if (target instanceof HTMLElement && target.matches('[data-testid="thread-scroll-container"], .thread-graph-scroll-container') && document.hasFocus() && target.scrollHeight - target.scrollTop - target.clientHeight < 8) store.markRead(key);
    };
    focus(); window.addEventListener('focus', focus); window.addEventListener('blur', focus); document.addEventListener('scroll', scroll, true);
    return () => { clearTimeout(timer); window.removeEventListener('focus', focus); window.removeEventListener('blur', focus); document.removeEventListener('scroll', scroll, true); };
  }, [pathname, store]);
}
