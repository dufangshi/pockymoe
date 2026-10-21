import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import { nativeAppBridge } from '../lib/nativeApp';
import {
  currentPushSubscription,
  disablePush,
  enablePush,
  loadPushSettings,
  pushSupported,
  subscriptionId,
  type PushSettings,
} from '../lib/relayPush';

export function RelayNotifications() {
  useI18n();
  const [settings, setSettings] = useState<PushSettings | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState('');
  // A failed read of the settings can be retried from its mark.
  const [loadFailed, setLoadFailed] = useState(false);
  const supported = pushSupported();
  const native = nativeAppBridge();
  async function refresh() {
    setLoadFailed(false);
    try {
      const next = await loadPushSettings();
      setSettings(next);
      const sub = supported && !native ? await currentPushSubscription() : null;
      setEnabled(
        !!sub &&
          Notification.permission === 'granted' &&
          next.subscriptionIds.includes(await subscriptionId(sub.endpoint)),
      );
    } catch (e) {
      setLoadFailed(true);
      setError(
        e instanceof Error
          ? e.message
          : translate("workbench.unableToLoadNotificationSettings"),
      );
    } finally {
      setBusy(false);
    }
  }
  useEffect(() => {
    void refresh();
  }, []);
  function toggle() {
    if (!settings) return;
    setBusy(true);
    setError('');
    // Request permission immediately in the click handler, before other awaits.
    const operation = enabled
      ? disablePush()
      : enablePush(settings.publicKey, Notification.requestPermission());
    void operation.then(refresh).catch((e) => {
      setError(
        e instanceof Error ? e.message : translate("workbench.unableToChangeNotifications"),
      );
      setBusy(false);
    });
  }
  const retry = () => {
    setBusy(true);
    setError('');
    void refresh();
  };
  const mark = error ? <ErrorMark message={error} onRetry={loadFailed ? retry : undefined} /> : null;
  if (native) return (
    <section className="grid gap-4 py-6">
      <h2 className="text-base font-semibold">{translate("settings.notificationsBrowser")}</h2>
      <p className="text-sm text-[var(--theme-fg-muted)]">{translate("workbench.tapASystemNotificationToOpenIts")}</p>
      <p className="text-sm">{native.platform === 'ios'
        ? settings?.nativePushAvailable ? translate("workbench.backgroundPushIsAvailableOnThisRelay") : translate("workbench.thisRelayHasNotConfiguredApplePush")
        : translate("workbench.allowNotificationsAndKeepTheMonitoringService")} {mark}</p>
      <div className="flex flex-wrap gap-3">
        <button className="relay-button-primary min-h-11" onClick={() => native.notificationSettings()}>{translate("workbench.openNotificationSettings")}</button>
        <button className="relay-button-secondary min-h-11" onClick={() => native.changeRelay()}>{translate("workbench.changeRelay")}</button>
      </div>
    </section>
  );
  return (
    <section className="grid gap-5 py-6 sm:grid-cols-[11rem_minmax(0,1fr)] sm:gap-8">
      <header>
        <h2 className="text-base font-semibold">{translate("settings.notificationsBrowser")}</h2>
        <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
          {translate("workbench.yourRelayAccount")}</p>
      </header>
      <div className="space-y-3 text-sm">
        <p>
          {translate("workbench.notifyThisBrowserWhenAThreadTurn")}</p>
        <p className="text-[var(--theme-fg-muted)]">
          {translate("workbench.pagesCanBeClosedClickingANotification")}</p>
        {!supported ? (
          <p>
            {translate("workbench.thisBrowserCannotEnableWebPushHere")}</p>
        ) : (
          <>
            <p>
              {busy
                ? translate("workbench.checkingNotifications")
                : enabled
                  ? translate("workbench.enabledInThisBrowser")
                  : translate("workbench.disabledInThisBrowser")}
            </p>
            <div className="flex items-center gap-2">
              <button
                type="button"
                className="relay-button-primary min-h-11"
                disabled={busy || !settings}
                onClick={toggle}
              >
                {enabled ? translate("workbench.disableNotifications") : translate("workbench.enableNotifications")}
              </button>
              {mark}
            </div>
            {Notification.permission === 'denied' && (
              <p>
                {translate("workbench.notificationsAreBlockedAllowThemInThis")}</p>
            )}
          </>
        )}
        <p className="text-[var(--theme-fg-muted)]">
          {translate("workbench.enableSeparatelyInEachBrowserSigningOut")}</p>
      </div>
    </section>
  );
}
