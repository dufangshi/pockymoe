import { useBrowserStateKey } from './browserStateScope';
import { useEffect } from 'react';
import type { ThreadDto } from '@pockymoe/shared';

export type TabState = 'working' | 'waiting' | 'unread' | 'idle' | 'failed' | 'interrupted';

export function tabState(status: string, completed: number, seen: number, waiting = false): TabState {
  if (waiting) return 'waiting';
  if (status === 'running' || status === 'recovering') return 'working';
  if (status === 'failed' || status === 'system_error') return 'failed';
  if (status === 'interrupted') return 'interrupted';
  if (completed > seen) return 'unread';
  return 'idle';
}

// The thread status marks of ui-principles §6, drawn on the dark tile in their
// dark-mode colors: open ring, speech bubble, solid dot, hollow ring, diamond, square.
const iconShapes: Record<TabState, string> = {
  working:
    '<path d="M16 7a9 9 0 1 1-9 9" fill="none" stroke="#8ab8f2" stroke-width="3.5" stroke-linecap="round"/>',
  waiting:
    '<path d="M7 9a3 3 0 0 1 3-3h12a3 3 0 0 1 3 3v8a3 3 0 0 1-3 3h-1v5l-5-5h-6a3 3 0 0 1-3-3z" fill="#edb75d"/>',
  unread: '<circle cx="16" cy="16" r="7" fill="#2fbf71"/>',
  idle: '<circle cx="16" cy="16" r="5.5" fill="none" stroke="#a6b2bd" stroke-width="2.5"/>',
  failed: '<path d="M16 7l9 9-9 9-9-9z" fill="#f85149"/>',
  interrupted: '<rect x="10" y="10" width="12" height="12" rx="2" fill="#e9ab3c"/>',
};

export function useThreadTabStatus(thread: ThreadDto | null, waiting = false) {
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
    // A thread first opened here (say, in a background tab) starts as read:
    // only turns finished after that make it unread, as in the sidebar.
    try {
      if (localStorage.getItem(key) === null) localStorage.setItem(key, String(completed));
    } catch {
      memorySeen = completed;
    }
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
      const state = tabState(thread!.status, completed, seen, waiting);
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
  }, [seenKey, thread?.id, thread?.title, thread?.status, thread?.lastTurnCompletedAt, waiting]);
}
