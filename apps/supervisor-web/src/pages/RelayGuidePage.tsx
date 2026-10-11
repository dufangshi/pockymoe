import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ArrowLeft, CheckCircle2 } from 'lucide-react';
import { useEffect } from 'react';
import { Link } from 'react-router-dom';

import { enableRelayMode } from '../lib/api';

const setupCommand = [
  'curl -fsSL https://pockymoe.example.com/setup.sh | sh -s -- \\',
  '  --relay https://pockymoe.example.com --token DEVICE_TOKEN --port 8787',
].join('\n');

const connectionModes = [
  {
    get title() { return translate("devices.localMode"); },
    get detail() { return translate("devices.forTheSameMachineAnEmulatorLAN"); },
  },
  {
    get title() { return translate("devices.serverMode"); },
    get detail() { return translate("devices.forADirectlyExposedSupervisorProtectedBy"); },
  },
  {
    get title() { return translate("devices.relayMode"); },
    get detail() { return translate("devices.forAMachineThatShouldAcceptNo"); },
  },
];

const relaySteps = [
  {
    get title() { return translate("devices.registerOrSignIn"); },
    get detail() { return translate("devices.openTheRelayPortalThenCreateOr"); },
  },
  {
    get title() { return translate("devices.createADevice"); },
    get detail() { return translate("devices.inDevicesChooseARecognizableNameFor"); },
  },
  {
    get title() { return translate("devices.copyTheSetupCommand"); },
    get detail() { return translate("devices.useCopySetupTheCommandContainsThis"); },
  },
  {
    get title() { return translate("devices.startTheSupervisor"); },
    get detail() { return translate("devices.runTheCommandOnTheWorkspaceHost"); },
  },
  {
    get title() { return translate("devices.connectAndWork"); },
    get detail() { return translate("devices.returnToDevicesAndWaitForOnline"); },
  },
  {
    get title() { return translate("devices.shareWhenNeeded"); },
    get detail() { return translate("devices.fromAThreadOpenSharingEnterA"); },
  },
];

export function RelayGuidePage() {
  const { locale: i18nLocale } = useI18n();
  useEffect(() => {
    enableRelayMode();
  }, []);

  return (
    <main className="min-h-screen overflow-x-hidden bg-[var(--app-bg)] px-4 py-5 text-[var(--app-fg)] sm:px-6 sm:py-6" data-page="relay-guide">
      <article className="mx-auto w-full min-w-0 max-w-3xl">
        <header className="border-b border-[var(--theme-border)] pb-6">
          <Link className="relay-button-secondary mb-6 inline-flex h-11 items-center gap-2" to="/">
            <ArrowLeft aria-hidden="true" className="h-4 w-4" />
            {translate("devices.relayHome")}</Link>
          <p className="text-sm font-medium text-[var(--theme-accent-strong)]">{translate("devices.setupGuide")}</p>
          <h1 className="mt-2 text-2xl font-semibold tracking-normal text-[var(--theme-fg)] sm:text-3xl">
            {translate("devices.connectAPrivateSupervisor")}</h1>
          <p className="mt-3 max-w-2xl text-sm leading-6 text-[var(--theme-fg-soft)]">
            {translate("devices.pickTheModeThatMatchesYourNetwork")}</p>
        </header>

        <section className="py-8" aria-labelledby="connection-modes-heading">
          <h2 id="connection-modes-heading" className="text-lg font-semibold text-[var(--theme-fg)]">
            {translate("devices.connectionModes")}</h2>
          <dl className="mt-4 divide-y divide-[var(--theme-border)] border-y border-[var(--theme-border)]">
            {connectionModes.map((mode) => (
              <div className="grid gap-1 py-4 sm:grid-cols-[9rem_minmax(0,1fr)] sm:gap-5" key={mode.title}>
                <dt className="text-sm font-medium text-[var(--theme-fg)]">{mode.title}</dt>
                <dd className="text-sm leading-6 text-[var(--theme-fg-muted)]">{mode.detail}</dd>
              </div>
            ))}
          </dl>
        </section>

        <section className="border-t border-[var(--theme-border)] py-8" aria-labelledby="relay-steps-heading">
          <h2 id="relay-steps-heading" className="text-lg font-semibold text-[var(--theme-fg)]">
            {translate("devices.relaySetup")}</h2>
          <ol className="mt-4 divide-y divide-[var(--theme-border)] border-y border-[var(--theme-border)]">
            {relaySteps.map((step, index) => (
              <li className="grid grid-cols-[2.75rem_minmax(0,1fr)] gap-3 py-4" key={step.title}>
                <span
                  aria-hidden="true"
                  className="inline-flex h-8 w-8 items-center justify-center rounded-full bg-[var(--theme-muted)] font-mono text-xs font-semibold text-[var(--theme-fg-soft)]"
                >
                  {String(index + 1).padStart(2, '0')}
                </span>
                <div className="min-w-0">
                  <h3 className="text-sm font-semibold text-[var(--theme-fg)]">{step.title}</h3>
                  <p className="mt-1 text-sm leading-6 text-[var(--theme-fg-muted)]">{step.detail}</p>
                </div>
              </li>
            ))}
          </ol>
        </section>

        <section className="min-w-0 border-t border-[var(--theme-border)] py-8" aria-labelledby="example-command-heading">
          <h2 id="example-command-heading" className="text-lg font-semibold text-[var(--theme-fg)]">
            {translate("devices.exampleSupervisorCommand")}</h2>
          <p className="mt-2 text-sm leading-6 text-[var(--theme-fg-muted)]">
            {translate("devices.devicesGeneratesTheRealCommandKeepIts")}</p>
          <pre className="mt-4 block w-full min-w-0 max-w-full overflow-x-auto rounded-lg border border-[var(--theme-border)] bg-[var(--theme-surface-strong)] p-3 text-xs leading-5 text-[var(--theme-fg)]">
            <code className="block min-w-max">{setupCommand}</code>
          </pre>
        </section>

        <section className="border-t border-[var(--theme-border)] py-8" aria-labelledby="after-setup-heading">
          <h2 id="after-setup-heading" className="text-lg font-semibold text-[var(--theme-fg)]">
            {translate("devices.afterSetup")}</h2>
          <ul className="mt-4 space-y-3 text-sm leading-6 text-[var(--theme-fg-muted)]">
            <GuideOutcome>{translate("devices.useDevicesToSwitchBetweenSupervisorMachines")}</GuideOutcome>
            <GuideOutcome>{translate("devices.useSharedWithMeToOpenSessions")}</GuideOutcome>
            <GuideOutcome>{translate("devices.useSharedByMeToReviewAccess")}</GuideOutcome>
          </ul>
        </section>
      </article>
    </main>
  );
}

function GuideOutcome({ children }: { children: React.ReactNode }) {
  const { locale: i18nLocale } = useI18n();
  return (
    <li className="flex gap-3">
      <CheckCircle2
        aria-hidden="true"
        className="mt-1 h-4 w-4 shrink-0 text-[var(--status-success-fg)]"
      />
      <span>{children}</span>
    </li>
  );
}
