import { useEffect, useState } from 'react';
import { ArrowUp, Loader2 } from 'lucide-react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { PockymoeHost, UpdateState } from './host';

const idle: UpdateState = { status: 'idle' };
function releasePage(value: string | undefined): string | null {
  // A host-provided error must not turn recovery into an arbitrary external link.
  return value && /^https:\/\/github\.com\/dufangshi\/pockymoe-app\/releases\/tag\/v?\d+\.\d+\.\d+$/.test(value) ? value : null;
}

/** The rail only receives a verified-ready state; downloading stays invisible. */
export function ClientUpdateButton({ host, enabled }: { host: PockymoeHost | null; enabled: boolean }) {
  useI18n();
  const [state, setState] = useState<UpdateState>(idle);
  const updates = enabled ? host?.updates : undefined;
  useEffect(() => {
    setState(idle);
    if (!updates || typeof updates.state !== 'function' || typeof updates.onChange !== 'function' || typeof updates.install !== 'function') return;
    let active = true, changed = false;
    // Subscribe first so a delayed initial snapshot cannot overwrite a newer event.
    const unsubscribe = updates.onChange(next => { changed = true; if (active) setState(next); });
    void updates.state().then(next => { if (active && !changed) setState(next); }, () => undefined);
    return () => { active = false; unsubscribe(); };
  }, [updates]);
  if (!updates || !state.version || !['ready', 'installing', 'error'].includes(state.status)) return null;
  const busy = state.status === 'installing';
  const retryDisabled = state.retryAllowed === false;
  const label = translate('desktopShell.updateTo', { version: state.version });
  const install = () => {
    if (busy || retryDisabled) return;
    setState(current => { const next = { ...current, status: 'installing' as const }; delete next.error; return next; });
    try { updates.install(); }
    catch { setState(current => ({ ...current, status: 'error', error: translate('desktopShell.updateFailed') })); }
  };
  const download = releasePage(state.releaseUrl);
  return <div className="desktop-client-update" data-update-status={state.status}>
    <button type="button" className="desktop-client-update-button" aria-label={label} title={label} aria-busy={busy} disabled={busy || retryDisabled} onClick={install}>
      <span className="desktop-client-update-circle">{busy ? <Loader2 aria-hidden="true" size={16} className="composer-send-spinner animate-spin" /> : <ArrowUp aria-hidden="true" size={16} />}</span>
    </button>
    {state.status === 'error' ? <span className="desktop-client-update-failure"><ErrorMark className="desktop-client-update-error" message={state.error || translate('desktopShell.updateFailed')} onRetry={install} retryDisabled={retryDisabled} secondaryAction={download && host?.shell?.openExternal ? { label: translate('desktopShell.downloadUpdate'), onClick: () => host.shell.openExternal(download) } : undefined} /></span> : null}
  </div>;
}
