import { useBrowserStateKey } from './browserStateScope';
import { useEffect } from 'react';
import type { ThreadDto } from '@pockymoe/shared';

export function tabState(status: string, completed: number, seen: number) {
  if (status === 'running') return 'working';
  if (status === 'recovering') return 'recovering';
  if (completed > seen) return 'unread';
  return status === 'failed' ? 'failed' : 'idle';
}

const iconShapes = {
  working:
    '<path d="M25 16a9 9 0 1 1-9-9" fill="none" stroke="#e9ab3c" stroke-width="4" stroke-linecap="round"/>',
  unread: '<circle cx="16" cy="16" r="9" fill="#f85149"/>',
  idle: '<path d="m8 16 5 5 11-11" fill="none" stroke="#a6b2bd" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>',
  failed:
    '<path d="m10 10 12 12m0-12L10 22" stroke="#f85149" stroke-width="4" stroke-linecap="round"/>',
  recovering:
    '<path d="M11 11a5 5 0 0 1 10 0c0 4-5 4-5 8" fill="none" stroke="#e9ab3c" stroke-width="3" stroke-linecap="round"/><circle cx="16" cy="24" r="2" fill="#e9ab3c"/>',
};

export function useThreadTabStatus(thread: ThreadDto | null) {
  const seenKey = useBrowserStateKey(`remote-codex:thread-seen:${window.location.pathname}`);
  useEffect(() => {
    if (!thread || !window.location.pathname.endsWith('/' + thread.id)) return;
    const key = seenKey;
    const completed = Date.parse(thread.lastTurnCompletedAt ?? '') || 0;
    let memorySeen = 0;
    const icons = Array.from(
      document.querySelectorAll<HTMLLinkElement>('link[rel~="icon"]'),
    );
    const previous = icons.map((icon) => [icon, icon.href, icon.type] as const);
    const icon = document.createElement('link');
    icon.rel = 'icon';
    icon.type = 'image/svg+xml';
    icons.forEach((old) => old.remove());
    document.head.append(icon);
    function update() {
      let seen = memorySeen;
      try {
        seen = Math.max(seen, Number(localStorage.getItem(key)) || 0);
      } catch {
        /* Private storage may be unavailable. */
      }
      if (
        document.visibilityState === 'visible' &&
        document.hasFocus() &&
        thread!.status !== 'running' &&
        thread!.status !== 'recovering'
      ) {
        seen = Math.max(seen, completed);
        memorySeen = seen;
        try {
          if (localStorage.getItem(key) !== String(seen))
            localStorage.setItem(key, String(seen));
        } catch {
          /* Keep the in-page read marker. */
        }
      }
      const state = tabState(thread!.status, completed, seen);
      document.title = thread!.title;
      icon.href =
        'data:image/svg+xml,' +
        encodeURIComponent(
          `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="7" fill="#171713"/>${iconShapes[state]}</svg>`,
        );
    }
    update();
    document.addEventListener('visibilitychange', update);
    window.addEventListener('focus', update);
    window.addEventListener('storage', update);
    return () => {
      document.removeEventListener('visibilitychange', update);
      window.removeEventListener('focus', update);
      window.removeEventListener('storage', update);
      icon.remove();
      previous.forEach(([old, href, type]) => {
        old.href = href;
        old.type = type;
        document.head.append(old);
      });
    };
  }, [seenKey, thread?.id, thread?.title, thread?.status, thread?.lastTurnCompletedAt]);
}
