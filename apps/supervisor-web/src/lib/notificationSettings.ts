import { useCallback, useEffect, useRef, useState } from 'react';
import { translate as t } from '@pockymoe/thread-ui/i18n';
import { relayModeActive, request } from './api';

export type NotificationScope = 'subscribed' | 'all' | 'selected';
export interface NotificationDevice { deviceId: string; name: string }
export interface NotificationWorkspace { deviceId: string; workspaceId: string; name: string; deviceName: string }
export interface NotificationSettings { scope: NotificationScope; devices: NotificationDevice[]; workspaces: NotificationWorkspace[] }
export const NOTIFICATION_CHANGE = 'pockymoe:notification-settings';
export const notificationRoot = () => relayModeActive() ? 'relay/account/notifications' : 'api/account/notifications';
export function notifyNotificationChange() {
  window.dispatchEvent(new Event(NOTIFICATION_CHANGE));
  // Invalidation only; account preferences and subscriptions remain on the server.
  try { localStorage.setItem('pockymoe.notification-revision', `${Date.now()}:${Math.random()}`); } catch { /* Other tabs refresh on focus. */ }
}
export async function fetchNotificationSettings(signal?: AbortSignal) {
  const value = await request<NotificationSettings>(`${notificationRoot()}/preferences`, { cache: 'no-store', ...(signal ? { signal } : {}) });
  if (!['subscribed', 'all', 'selected'].includes(value.scope) || !Array.isArray(value.devices) || !Array.isArray(value.workspaces)) throw new Error(t('settings.notificationsLoadFailed'));
  return value;
}
export function turnSubscriptionPath(device: string | null, thread: string, turn: string) {
  const ids = [thread, turn].map(encodeURIComponent).join('/');
  return `${notificationRoot()}/turns/${relayModeActive() ? `${encodeURIComponent(device!)}/` : ''}${ids}`;
}

export function useNotificationSettings() {
  const [value, setValue] = useState<NotificationSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const revision = useRef(0);
  const mutation = useRef(false);
  const failedSave = useRef<NotificationSettings | null>(null);
  const refresh = useCallback(async () => {
    if (mutation.current) return;
    const current = ++revision.current;
    setLoading(true);
    try {
      const next = await fetchNotificationSettings(AbortSignal.timeout(15000));
      if (current !== revision.current) return;
      setValue(next); setError(null); failedSave.current = null;
    } catch {
      if (current === revision.current) setError(t('settings.notificationsLoadFailed'));
    } finally {
      if (current === revision.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    void refresh();
    const visible = () => { if (document.visibilityState !== 'hidden') void refresh(); };
    const storage = (event: StorageEvent) => { if (event.key === 'pockymoe.notification-revision') visible(); };
    window.addEventListener('focus', visible);
    window.addEventListener(NOTIFICATION_CHANGE, visible);
    window.addEventListener('storage', storage);
    document.addEventListener('visibilitychange', visible);
    return () => {
      ++revision.current;
      window.removeEventListener('focus', visible);
      window.removeEventListener(NOTIFICATION_CHANGE, visible);
      window.removeEventListener('storage', storage);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [refresh]);
  const save = useCallback(async (next: NotificationSettings) => {
    if (mutation.current || !value) return false;
    const previous = value;
    const current = ++revision.current;
    mutation.current = true;
    setValue(next); setSaving(true); setLoading(false); setError(null); setSaved(false);
    try {
      const result = await request<NotificationSettings>(`${notificationRoot()}/preferences`, { method: 'PUT', body: JSON.stringify(next), signal: AbortSignal.timeout(15000) });
      if (current !== revision.current) return false;
      setValue(result); setSaved(true); failedSave.current = null;
      notifyNotificationChange();
      return true;
    } catch {
      if (current === revision.current) { setValue(previous); setError(t('settings.notificationsSaveFailed')); failedSave.current = next; }
      return false;
    } finally {
      mutation.current = false;
      if (current === revision.current) setSaving(false);
    }
  }, [value]);
  const retry = () => { if (failedSave.current) void save(failedSave.current); else void refresh(); };
  return { value, loading, saving, saved, error, save, retry };
}
