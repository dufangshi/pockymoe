import { useCallback, useEffect, useRef, useState } from 'react';
import { relayModeActive, request } from './api';

/** Whether conversations show the agents' thinking summaries.
 *
 * Signed in, the choice belongs to the account (`/relay/account/preferences`,
 * like the message shortcuts) and every browser and client follows it. The
 * browser keeps a copy so the first paint is right and so local mode, which
 * has no account, still remembers it.
 */
const LOCAL = 'remote-codex-show-reasoning-summaries';
/** A change made here that the account has not confirmed yet. */
const UNSAVED = 'pockymoe-reasoning-summaries-unsaved';

interface AccountPreferences { showReasoningSummaries?: boolean | null }

function read(key: string) {
  try { return localStorage.getItem(key); } catch { return null; }
}
function write(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key); else localStorage.setItem(key, value);
  } catch { /* Private browsing may disable storage. */ }
}

function saveToAccount(showReasoningSummaries: boolean) {
  return request<AccountPreferences>('relay/account/preferences', {
    method: 'PATCH',
    body: JSON.stringify({ showReasoningSummaries }),
  });
}

export function useReasoningSummaries() {
  const account = relayModeActive();
  const [shown, setShown] = useState(() => read(LOCAL) === 'true');
  const revision = useRef(0);

  const adopt = useCallback((value: boolean) => {
    setShown(value);
    write(LOCAL, String(value));
  }, []);

  const refresh = useCallback(async () => {
    if (!account) return;
    const current = ++revision.current;
    try {
      // A change the account never received goes first, or reading would undo it.
      const unsaved = read(UNSAVED);
      if (unsaved !== null) {
        await saveToAccount(unsaved === 'true');
        if (read(UNSAVED) === unsaved) write(UNSAVED, null);
      }
      const preferences = await request<AccountPreferences>('relay/account/preferences', { cache: 'no-store' });
      if (current !== revision.current || read(UNSAVED) !== null) return;
      const chosen = preferences.showReasoningSummaries;
      if (typeof chosen === 'boolean') adopt(chosen);
      // The account has not chosen yet: a browser that had it on makes the choice.
      else if (chosen === null && read(LOCAL) === 'true') await saveToAccount(true);
    } catch {
      // Offline, or a relay from before this preference: the browser's copy stands.
    }
  }, [account, adopt]);

  useEffect(() => {
    void refresh();
    const onFocus = () => { void refresh(); };
    const onVisibility = () => { if (document.visibilityState === 'visible') void refresh(); };
    window.addEventListener('focus', onFocus);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      revision.current += 1;
      window.removeEventListener('focus', onFocus);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [refresh]);

  const set = useCallback((value: boolean) => {
    revision.current += 1;
    adopt(value);
    if (!account) return;
    write(UNSAVED, String(value));
    saveToAccount(value).then(() => {
      if (read(UNSAVED) === String(value)) write(UNSAVED, null);
    }).catch(() => { /* Kept as unsaved; the next refresh sends it again. */ });
  }, [account, adopt]);

  return [shown, set] as const;
}
