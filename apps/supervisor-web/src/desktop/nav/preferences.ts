import { useCallback, useState } from 'react';
import type { Scope } from './model';
export interface NavPreferences { width: number; collapsed: boolean; expanded: Record<string, boolean>; scope: Scope; provider: string }
export function useNavPreferences(accountId: string) {
  const key = `rc-desktop-nav:${location.origin}:${accountId}`;
  const [value, setValue] = useState<NavPreferences>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem(key) ?? '{}') as Partial<NavPreferences>;
      return { width: Math.max(180, Math.min(420, saved.width ?? 264)), collapsed: Boolean(saved.collapsed), expanded: saved.expanded ?? {}, scope: ['all', 'active', 'pinned'].includes(saved.scope ?? '') ? saved.scope! : 'all', provider: saved.provider ?? '' };
    } catch { return { width: 264, collapsed: false, expanded: {}, scope: 'all', provider: '' }; }
  });
  const update = useCallback((patch: Partial<NavPreferences> | ((current: NavPreferences) => Partial<NavPreferences>)) => {
    setValue(current => {
      const next = { ...current, ...(typeof patch === 'function' ? patch(current) : patch) };
      try { localStorage.setItem(key, JSON.stringify(next)); } catch { /* Preferences are optional. */ }
      return next;
    });
  }, [key]);
  return [value, update] as const;
}
