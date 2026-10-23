import { useEffect, useState } from 'react';
import { request } from './api';

/** The owner's sharing preferences, saved to the account at the relay
    (`/relay/account/preferences`, like the message shortcuts). */
export interface SharingPreferences {
  /** New shares wait until the owner confirms the recipient's safety code. */
  requireSafetyCode: boolean;
}

const CHANGED = 'pockymoe-sharing-preferences';
let cached: Promise<SharingPreferences> | null = null;

function checked(value: Partial<SharingPreferences>): SharingPreferences {
  // Missing (an older relay) reads as on: the check is the safe default.
  return { requireSafetyCode: value.requireSafetyCode !== false };
}

export function fetchSharingPreferences(fresh = false) {
  if (!cached || fresh) {
    const loading = request<Partial<SharingPreferences>>('relay/account/preferences', { cache: 'no-store' }).then(checked);
    cached = loading;
    loading.catch(() => { if (cached === loading) cached = null; });
  }
  return cached;
}

export async function saveRequireSafetyCode(requireSafetyCode: boolean) {
  const saved = checked(await request<Partial<SharingPreferences>>('relay/account/preferences', {
    method: 'PATCH',
    body: JSON.stringify({ requireSafetyCode }),
  }));
  cached = Promise.resolve(saved);
  window.dispatchEvent(new Event(CHANGED));
  return saved;
}

/** Whether the check is on; `true` until loaded or when it cannot be read. */
export function useRequireSafetyCode() {
  const [value, setValue] = useState(true);
  useEffect(() => {
    let active = true;
    const read = () => void fetchSharingPreferences()
      .then(preferences => { if (active) setValue(preferences.requireSafetyCode); })
      .catch(() => {});
    read();
    window.addEventListener(CHANGED, read);
    return () => { active = false; window.removeEventListener(CHANGED, read); };
  }, []);
  return value;
}
