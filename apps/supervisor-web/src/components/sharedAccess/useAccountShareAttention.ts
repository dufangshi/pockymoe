import { useEffect, useMemo, useState } from 'react';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import { fetchRelayPortal, relayModeActive } from '../../lib/api';
import { SHARING_CHANGED_EVENT } from '../../lib/sharePending';
import { sharedItems } from './model';
import { useShareAttention } from './useShareAttention';

/** Rail and Devices use the same outgoing-share checks, including pending codes. */
export function useAccountShareAttention() {
  const [portal, setPortal] = useState<RelayPortalSummaryDto | null>(null);
  useEffect(() => {
    if (!relayModeActive()) return;
    let active = true;
    const refresh = () => { void fetchRelayPortal().then(value => { if (active) setPortal(value); }).catch(() => {}); };
    refresh();
    window.addEventListener(SHARING_CHANGED_EVENT, refresh);
    window.addEventListener('focus', refresh);
    const timer = setInterval(refresh, 30_000);
    return () => { active = false; clearInterval(timer); window.removeEventListener(SHARING_CHANGED_EVENT, refresh); window.removeEventListener('focus', refresh); };
  }, []);
  const items = useMemo(() => sharedItems(portal, 'out'), [portal]);
  return { ...useShareAttention(items), portal };
}
