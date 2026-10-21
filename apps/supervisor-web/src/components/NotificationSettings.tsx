import { useState } from 'react';
import { Folder, Laptop, X } from 'lucide-react';
import { translate as t, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark } from '@pockymoe/thread-ui';
import { relayModeActive } from '../lib/api';
import { useNotificationSettings, type NotificationSettings as Settings, type NotificationScope } from '../lib/notificationSettings';
import { WorkbenchThreadPicker, type ScopeSelection } from './WorkbenchThreadPicker';
import { RelayNotifications } from './RelayNotifications';

export function NotificationSettings() {
  useI18n();
  const state = useNotificationSettings();
  const [removed, setRemoved] = useState<Settings | null>(null);
  const value = state.value;
  const disabled = state.saving || !value;
  // A failed refresh (on focus, another tab's change) keeps the choices already
  // shown and says nothing; only a failed save, or a first read that never
  // arrived, gets a mark.
  const quietRefresh = value != null && state.error === t('settings.notificationsLoadFailed');
  const error = quietRefresh ? null : state.error;
  function add(kind: 'device' | 'workspace', selection: ScopeSelection) {
    if (!value) return;
    setRemoved(null);
    const deviceId = selection.deviceId ?? 'local';
    if (kind === 'device') {
      if (value.devices.some(d => d.deviceId === deviceId)) return;
      void state.save({ ...value, devices: [...value.devices, { deviceId, name: selection.deviceName }] });
    } else if (selection.workspace) {
      const workspace = selection.workspace;
      if (value.workspaces.some(w => w.deviceId === deviceId && w.workspaceId === workspace.id)) return;
      void state.save({ ...value, workspaces: [...value.workspaces, { deviceId, workspaceId: workspace.id, name: workspace.label, deviceName: selection.deviceName }] });
    }
  }
  async function remove(next: Settings) {
    const previous = value;
    if (await state.save(next)) setRemoved(previous);
  }
  return <section className="space-y-5 py-2" aria-label={t('settings.notificationsTab')}>
    <p className="notification-scope-hint text-sm text-[var(--theme-fg-muted)]">{t(relayModeActive() ? 'settings.accountScope' : 'settings.notificationsLocalScope')}</p>
    {/* These hints explain notification behavior and storage scope in every theme. */}
    <fieldset disabled={disabled} className="notification-scope-options space-y-2">
      <legend className="sr-only">{t('settings.notificationsTab')}</legend>
      {([
        ['subscribed', 'settings.notificationsSubscribed', 'settings.notificationsSubscribedHint'],
        ['all', 'settings.notificationsAll', 'settings.notificationsAllHint'],
        ['selected', 'settings.notificationsSelected', 'settings.notificationsSelectedHint'],
      ] as const).map(([scope, label, hint]) => <label key={scope} className="notification-scope-option flex min-h-11 items-start gap-3 rounded-md border border-[var(--theme-border)] p-3">
        <input type="radio" name="notification-scope" value={scope} checked={value?.scope === scope} className="mt-0.5 h-4 w-4 shrink-0 accent-[var(--theme-accent-solid)]"
          onChange={() => { if (value) { setRemoved(null); void state.save({ ...value, scope: scope as NotificationScope }); } }} />
        <span><span className="block text-sm font-medium">{t(label)}</span><span className="notification-scope-hint mt-1 block text-xs text-[var(--theme-fg-muted)]">{t(hint)}</span></span>
      </label>)}
    </fieldset>
    {value?.scope === 'selected' && <div className="space-y-3">
      <div className="notification-scope-cards">
        {value.devices.map(device => <div className="notification-scope-card" key={`d:${device.deviceId}`}>
          <Laptop size={17} aria-hidden="true" /><span><strong>{device.name}</strong><small>{t('files.device')}</small></span>
          <button type="button" disabled={disabled} aria-label={t('settings.notificationsRemove', { name: device.name })} onClick={() => void remove({ ...value, devices: value.devices.filter(d => d.deviceId !== device.deviceId) })}><X size={16} /></button>
        </div>)}
        {value.workspaces.map(workspace => <div className="notification-scope-card" key={`w:${workspace.deviceId}:${workspace.workspaceId}`}>
          <Folder size={17} aria-hidden="true" /><span><strong>{workspace.name}</strong><small>{workspace.deviceName}</small></span>
          <button type="button" disabled={disabled} aria-label={t('settings.notificationsRemove', { name: workspace.name })} onClick={() => void remove({ ...value, workspaces: value.workspaces.filter(w => w.deviceId !== workspace.deviceId || w.workspaceId !== workspace.workspaceId) })}><X size={16} /></button>
        </div>)}
      </div>
      <div className="notification-scope-add">
        {(['device', 'workspace'] as const).map(kind => <WorkbenchThreadPicker key={kind} deviceId={null} workspaceId="" threadId="" threads={[]} onSelect={() => {}}
          selectScope={kind} onSelectScope={selection => add(kind, selection)} disabled={disabled || value.devices.length + value.workspaces.length >= 200} />)}
      </div>
      {!value.devices.length && !value.workspaces.length && <p className="notification-scope-hint text-xs text-[var(--theme-fg-muted)]">{t('settings.notificationsEmpty')}</p>}
    </div>}
    <div className="flex min-h-5 items-center gap-1 text-xs">
      <span role="status" aria-live="polite">
        {state.saving ? t('settings.saving') : state.loading ? t('settings.loading') : removed ? <>{t('settings.notificationsRemoved')} · <button type="button" className="underline" disabled={disabled} onClick={() => { const previous = removed; setRemoved(null); void state.save(previous); }}>{t('settings.notificationsUndo')}</button></> : state.saved ? t('files.saved_c0ae8f') : null}
      </span>
      {error && !state.saving && !state.loading && <ErrorMark message={error} onRetry={state.retry} retryLabel={t('settings.retry')} />}
    </div>
    {relayModeActive() ? <RelayNotifications /> : <p className="notification-scope-hint text-xs text-[var(--theme-fg-muted)]">{t('settings.notificationsLocalBrowser')}</p>}
  </section>;
}
