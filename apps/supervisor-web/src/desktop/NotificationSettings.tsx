import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Bell, Sparkles } from 'lucide-react';
import { ErrorMark } from '@pockymoe/thread-ui';
export default function DesktopNotificationSettings() {
  useI18n();
  const host = window.pockymoeHost;
  const supported = typeof host?.window?.openPreferences === 'function';
  return <section className="space-y-2 py-4" aria-label={translate('desktopShell.notificationPreferencesLabel')}><h3 className="text-sm font-medium">{translate('desktopShell.notificationPreferences')}</h3><p className="text-xs text-[var(--theme-fg-muted)]">{translate('desktopShell.notificationsManaged')}</p><div className="flex items-center gap-2"><button className="relay-button-secondary inline-flex items-center gap-2" disabled={!supported} onClick={() => host?.window.openPreferences('general')}><Bell size={14} data-icon="notifications" />{translate('desktopShell.notificationPreferences')}<Sparkles size={14} /></button>{!supported ? <ErrorMark kind="unavailable" message={translate('desktopShell.updateShellForPreferences')} /> : null}</div></section>;
}
