import { useEffect, useRef, useState } from 'react';
import { TurnNotificationButton } from '@pockymoe/thread-ui';
import { translate as t } from '@pockymoe/thread-ui/i18n';
import { request } from '../lib/api';
import { NOTIFICATION_CHANGE, notifyNotificationChange, turnSubscriptionPath, useNotificationSettings } from '../lib/notificationSettings';

export function TurnNotification({ deviceId, threadId, turnId }: { deviceId: string | null; threadId: string; turnId: string }) {
  const settings = useNotificationSettings();
  const [subscribed, setSubscribed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const owner = useRef({ mounted: false, version: 0, mutation: false });
  const retryLoad = useRef<() => void>(() => {});
  const failedMutation = useRef(false);
  const path = turnSubscriptionPath(deviceId, threadId, turnId);
  const enabled = settings.value?.scope === 'subscribed';
  useEffect(() => {
    const current = owner.current;
    current.mounted = true;
    if (!enabled) { setLoading(false); return () => { current.mounted = false; ++current.version; }; }
    const refresh = async () => {
      if (current.mutation || document.visibilityState === 'hidden') return;
      const version = ++current.version;
      try {
        const next = await request<{ subscribed: boolean }>(path, { cache: 'no-store', signal: AbortSignal.timeout(15000) });
        if (current.mounted && version === current.version) { setSubscribed(next.subscribed); setError(null); failedMutation.current = false; }
      } catch {
        // A background read that fails (a weak network, a sleeping device) keeps
        // what is shown; only a change the reader asked for reports a failure.
      } finally {
        if (current.mounted && version === current.version) setLoading(false);
      }
    };
    retryLoad.current = () => { setLoading(true); void refresh(); };
    setLoading(true);
    void refresh();
    const visible = () => { void settings.retry(); void refresh(); };
    const changed = () => void refresh();
    const storage = (event: StorageEvent) => { if (event.key === 'pockymoe.notification-revision') changed(); };
    const interval = window.setInterval(visible, 30000);
    window.addEventListener('focus', changed);
    window.addEventListener('storage', storage);
    window.addEventListener(NOTIFICATION_CHANGE, changed);
    document.addEventListener('visibilitychange', changed);
    return () => {
      current.mounted = false; ++current.version;
      clearInterval(interval);
      window.removeEventListener('focus', changed);
      window.removeEventListener('storage', storage);
      window.removeEventListener(NOTIFICATION_CHANGE, changed);
      document.removeEventListener('visibilitychange', changed);
    };
  }, [path, enabled]);
  if (settings.value && !enabled) return null;
  async function toggle() {
    const current = owner.current;
    if (current.mutation || loading || settings.loading || !enabled) return;
    const previous = subscribed;
    current.mutation = true;
    const version = ++current.version;
    setSubscribed(!previous); setSaving(true); setError(null);
    try {
      const next = await request<{ subscribed: boolean }>(path, { method: 'PUT', body: JSON.stringify({ subscribed: !previous }), signal: AbortSignal.timeout(15000) });
      if (current.mounted && version === current.version) { setSubscribed(next.subscribed); notifyNotificationChange(); }
    } catch {
      if (current.mounted && version === current.version) { setSubscribed(previous); setError(t('settings.notificationsTurnError')); failedMutation.current = true; }
    } finally {
      current.mutation = false;
      if (current.mounted && version === current.version) setSaving(false);
    }
  }
  return <TurnNotificationButton subscribed={subscribed} busy={saving || loading || settings.loading}
    error={error} onToggle={() => void toggle()}
    onRetry={() => { if (failedMutation.current) void toggle(); else retryLoad.current(); }} />;
}
