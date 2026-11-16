import { useEffect, useMemo } from 'react';
import { useRenderedState } from '../../lib/useRenderedState';
import { documentRenders, subscribeWhenRendering } from '../../lib/renderingLifecycle';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import { fetchRelayPortal, relayModeActive } from '../../lib/api';
import { SHARING_CHANGED_EVENT } from '../../lib/sharePending';
import { sharedItems } from './model';
import { useShareAttention } from './useShareAttention';

/** Rail and Devices use the same outgoing-share checks, including pending codes. */
export function useAccountShareAttention() {
  const [portal, setPortal] = useRenderedState<RelayPortalSummaryDto | null>(null);
  useEffect(() => {
    if (!relayModeActive()) return;
    let active = true;
    const refresh = () => { if (documentRenders()) void fetchRelayPortal().then(value => { if (active) setPortal(value); }).catch(() => {}); };
    const stopPolling = subscribeWhenRendering(() => { refresh(); const timer = setInterval(refresh, 30_000); return () => clearInterval(timer); }, () => {});
    window.addEventListener(SHARING_CHANGED_EVENT, refresh);
    window.addEventListener('focus', refresh);
    return () => { active = false; stopPolling(); window.removeEventListener(SHARING_CHANGED_EVENT, refresh); window.removeEventListener('focus', refresh); };
  }, [setPortal]);
  const items = useMemo(() => sharedItems(portal, 'out'), [portal]);
  return { ...useShareAttention(items), portal };
}
