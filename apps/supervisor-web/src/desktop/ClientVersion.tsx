import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useHostCapabilities } from './surface';
import { useClientUpdate } from './ClientUpdateButton';

export function ClientVersion() {
  useI18n();
  const capabilities = useHostCapabilities();
  const desktop = capabilities?.surface === 'desktop';
  const { state, install } = useClientUpdate(desktop ? window.pockymoeHost ?? null : null, desktop && capabilities.features.updates === true);
  if (!desktop || !capabilities.shellVersion) return null;
  return <p className="client-version py-3 text-[11px] leading-4 text-[var(--theme-fg-muted)]">
    Pockymoe {capabilities.shellVersion}
    {state.status === 'ready' && state.version && <> · <button type="button" role="menuitem" className="relative text-inherit after:absolute after:inset-x-0 after:top-1/2 after:h-10 after:-translate-y-1/2 focus-visible:outline focus-visible:outline-2" onClick={install}>{translate('desktopShell.updateAvailable', { version: state.version })}</button></>}
  </p>;
}
