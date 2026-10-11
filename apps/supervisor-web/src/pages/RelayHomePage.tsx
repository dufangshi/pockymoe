import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ProductHeader } from '../components/ProductHeader';
import { ArrowRight, BookOpen, MonitorSmartphone, RefreshCw } from 'lucide-react';
import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';

import type { RelaySessionDto } from '@pockymoe/shared';
import { ApiError, enableRelayMode, fetchRelaySession } from '../lib/api';

function errorMessage(caught: unknown) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : translate("auth.theRelayServiceCouldNotBeReached");
}

export function RelayHomePage() {
  const { locale: i18nLocale } = useI18n();
  const [session, setSession] = useState<RelaySessionDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  async function loadSession() {
    setLoading(true);
    setError(null);
    try {
      enableRelayMode();
      setSession(await fetchRelaySession());
    } catch (caught) {
      setSession(null);
      setError(errorMessage(caught));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void loadSession();
  }, []);

  const authenticated =
    session?.authenticated === true && session.user?.role !== 'admin';

  const title = loading
    ? translate("auth.checkingRelayAccess")
    : error
      ? translate("auth.relayServiceUnavailable")
      : authenticated
        ? translate("auth.chooseADeviceToContinue")
        : translate("auth.signInToYourRelayWorkspace");

  return (
    <main className="min-h-screen bg-[var(--app-bg)] px-4 py-5 text-[var(--app-fg)] sm:px-6 sm:py-6" data-page="relay-home">
      <div className="mx-auto w-full max-w-5xl">
        <ProductHeader title="Pockymoe" actions={<Link className="product-icon-button" to="/relay-guide" aria-label={translate("auth.guide")} title={translate("auth.guide")}><BookOpen size={18} /></Link>} />

        <section className="py-10 sm:py-14" aria-busy={loading}>
          <div className="flex items-center gap-2 text-sm text-[var(--theme-fg-muted)]">
            <span
              aria-hidden="true"
              className={`h-2 w-2 rounded-full ${
                loading
                  ? 'bg-[var(--theme-fg-muted)]'
                  : error
                    ? 'bg-[var(--status-danger-fg)]'
                    : authenticated
                      ? 'bg-[var(--status-success-fg)]'
                      : 'bg-[var(--theme-fg-muted)]'
              }`}
            />
            {loading
              ? translate("auth.checkingSession")
              : error
                ? translate("auth.connectionFailed")
                : authenticated
                  ? translate("auth.signedInAs", { value1: session.user?.username })
                  : translate("auth.signedOut")}
          </div>

          <h1 className="mt-4 max-w-2xl text-2xl font-semibold tracking-normal text-[var(--theme-fg)] sm:text-3xl">
            {title}
          </h1>
          <p className="mt-3 max-w-2xl text-sm leading-6 text-[var(--theme-fg-soft)]">
            {error
              ? translate("auth.yourSessionCouldNotBeCheckedVerify")
              : authenticated
                ? translate("auth.openDeviceManagementToConnectToA")
                : translate("auth.useYourRelayAccountToReachThe")}
          </p>

          {error ? (
            <div
              className="mt-6 max-w-2xl rounded-lg bg-[var(--status-danger-bg)] px-4 py-3 text-sm text-[var(--status-danger-fg)]"
              role="alert"
            >
              <p>{error}</p>
              <button
                className="relay-button-secondary mt-3 inline-flex h-11 items-center gap-2"
                disabled={loading}
                onClick={() => void loadSession()}
                type="button"
              >
                <RefreshCw aria-hidden="true" className="h-4 w-4" />
                {translate("auth.retry")}</button>
            </div>
          ) : loading ? (
            <div className="mt-6 h-11 w-36 animate-pulse rounded-lg bg-[var(--theme-muted)]" aria-hidden="true" />
          ) : (
            <Link
              className="relay-button-primary mt-6 inline-flex h-11 items-center gap-2 px-4"
              to={authenticated ? '/relay-devices' : '/relay-portal'}
            >
              <MonitorSmartphone aria-hidden="true" className="h-4 w-4" />
              {authenticated ? translate("auth.openDevices") : translate("auth.signIn")}
              <ArrowRight aria-hidden="true" className="h-4 w-4" />
            </Link>
          )}
        </section>

        <section className="border-t border-[var(--theme-border)] py-6">
          <div className="grid gap-4 sm:grid-cols-[10rem_minmax(0,1fr)]">
            <div>
              <h2 className="text-sm font-semibold text-[var(--theme-fg)]">{translate("auth.connectionPath")}</h2>
              <p className="mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
                {translate("auth.threeStepsOneOutboundTunnel")}</p>
            </div>
            <ol className="divide-y divide-[var(--theme-border)] border-y border-[var(--theme-border)]">
              <ConnectionStep number="01" title={translate("auth.registerADevice")}>
                {translate("auth.createAPermanentTokenForThePrivate")}</ConnectionStep>
              <ConnectionStep number="02" title={translate("auth.startTheSupervisor")}>
                {translate("auth.keepAnOutboundRelayConnectionOpenFrom")}</ConnectionStep>
              <ConnectionStep number="03" title={translate("auth.openYourWorkspace")}>
                {translate("auth.selectTheOnlineDeviceAndContinueTo")}</ConnectionStep>
            </ol>
          </div>
        </section>
      </div>
    </main>
  );
}

function ConnectionStep({
  children,
  number,
  title,
}: {
  children: React.ReactNode;
  number: string;
  title: string;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <li className="grid grid-cols-[2rem_minmax(0,1fr)] gap-3 py-3 first:pt-0 last:pb-0">
      <span className="font-mono text-xs leading-6 text-[var(--theme-fg-muted)]">{number}</span>
      <div className="min-w-0">
        <p className="text-sm font-medium text-[var(--theme-fg)]">{title}</p>
        <p className="mt-0.5 text-sm leading-6 text-[var(--theme-fg-muted)]">{children}</p>
      </div>
    </li>
  );
}
