import { openDevicePortPreview } from '../lib/portPreview';
import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef, useState } from 'react';
import { Network } from 'lucide-react';
import { ApiError, request } from '../lib/api';
import { FormDialog } from './FormDialog';

interface Mapping { id: string; port: number; label: string; createdAt: string }
interface LocalLink { port: number; path: string }

export function parseLocalPreviewLink(href: string): LocalLink | null {
  try {
    const url = new URL(/^(?:localhost|127\.0\.0\.1|\[::1\]):\d/i.test(href) ? `http://${href}` : href);
    if (url.protocol !== 'http:' || !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)
      || url.username || url.password) return null;
    // URL normalizes an explicit :80 to the default port.
    const port = Number(url.port || 80);
    if (port < 1 || port > 65535) return null;
    return { port, path: url.pathname + url.search + url.hash };
  } catch { return null; }
}

export function PortMappingsControl({ deviceId, open: controlledOpen, onOpenChange }: {
  deviceId: string; open?: boolean; onOpenChange?: (open: boolean) => void;
}) {
  useI18n();
  const [internalOpen, setInternalOpen] = useState(false);
  const manager = controlledOpen ?? internalOpen;
  const setManager = onOpenChange ?? setInternalOpen;
  const [link, setLink] = useState<LocalLink | null>(null);
  const [mappings, setMappings] = useState<Mapping[]>([]);
  const [available, setAvailable] = useState<boolean | null>(null);
  const [port, setPort] = useState('');
  const [label, setLabel] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const lifetime = useRef<AbortController | null>(null);
  const api = `/relay/devices/${encodeURIComponent(deviceId)}/api/port-mappings`;
  useEffect(() => {
    const controller = new AbortController();
    lifetime.current = controller;
    return () => controller.abort();
  }, []);
  useEffect(() => {
    const intercept = (event: MouseEvent) => {
      if (event.defaultPrevented || event.button !== 0 || !(event.target instanceof Element)) return;
      const anchor = event.target.closest<HTMLAnchorElement>('a[href]');
      if (!anchor?.closest('.thread-timeline-surface')) return;
      const target = parseLocalPreviewLink(anchor.getAttribute('href') ?? '');
      if (!target) return;
      event.preventDefault();
      event.stopPropagation();
      setManager(false);
      setError(null);
      setLink(target);
    };
    document.addEventListener('click', intercept, true);
    return () => document.removeEventListener('click', intercept, true);
  }, [setManager]);
  useEffect(() => {
    if (!manager && !link) return;
    const controller = new AbortController();
    setAvailable(null);
    setError(null);
    void Promise.all([
      request<{ mappings: Mapping[] }>(api, { signal: controller.signal }),
      request<{ available: boolean }>('/relay/port-mappings/config', { signal: controller.signal }),
    ]).then(([list, config]) => {
      if (!controller.signal.aborted) { setMappings(list.mappings); setAvailable(config.available); }
    }).catch(cause => {
      if (!controller.signal.aborted) setError(cause instanceof ApiError && cause.statusCode === 404
        ? translate("devices.updateThisDeviceSSupervisorToEnable_827e58")
        : cause instanceof Error ? cause.message : translate("devices.couldNotLoadPortMappings"));
    });
    return () => controller.abort();
  }, [api, manager, link]);

  async function action(run: (signal: AbortSignal) => Promise<void>) {
    const signal = lifetime.current!.signal;
    setBusy(true); setError(null); setCopied(null);
    try { await run(signal); }
    catch (cause) { if (!signal.aborted) setError(cause instanceof Error ? cause.message : translate("devices.portMappingFailed")); }
    finally { if (!signal.aborted) setBusy(false); }
  }
  async function enable(value: number, signal: AbortSignal): Promise<Mapping> {
    const mapping = await request<Mapping>(api, {
      method: 'POST', signal, body: JSON.stringify({ port: value, label }),
    });
    if (!signal.aborted) setMappings(items => items.some(item => item.id === mapping.id) ? items : [...items, mapping]);
    return mapping;
  }
  async function launch(mapping: Mapping, path: string, signal: AbortSignal) {
    return openDevicePortPreview(deviceId, mapping.id, path, signal);
  }
  function open(mapping?: Mapping, target = link) {
    // Reserve the tab in the click handler so asynchronous API calls do not
    // trigger the browser's popup blocker. Never expose the main window opener.
    const tab = window.open('about:blank', '_blank');
    if (!tab) { setError(translate("devices.allowPopupsForThisSiteThenTry")); return; }
    tab.opener = null;
    void action(async signal => {
      try {
        const selected = mapping ?? await enable(target!.port, signal);
        const result = await launch(selected, target?.path ?? '/', signal);
        if (signal.aborted) { tab.close(); return; }
        tab.location.replace(result.url);
        setLink(null);
      } catch (cause) { tab.close(); throw cause; }
    });
  }
  const buttonClass = 'host-secondary-button min-h-10 rounded-md border px-3 text-sm disabled:opacity-50';
  const inputClass = 'host-input min-h-10 w-full rounded-md border px-3';
  const readiness = available === false
    ? <p className="host-muted text-sm" role="status">{translate("devices.portPreviewsAreNotConfiguredOnThis")}</p>
    : available === null && !error ? <p className="host-muted text-sm" role="status">{translate("devices.loadingPortMappings")}</p> : null;
  const errorMessage = error ? <p className="host-error text-sm" role="alert">{error}</p> : null;
  return <>
    {controlledOpen === undefined && <button aria-label={translate("devices.portMappings")} title={translate("devices.portMappings")} onClick={() => setManager(true)}><Network /></button>}
    {manager && <FormDialog title={translate("devices.portMappings")} description={translate("devices.openHTTPServicesRunningOnThisDevice")} busy={busy} onClose={() => setManager(false)}>
      <div className="space-y-4 min-w-0">
        {readiness}{errorMessage}
        <form className="flex flex-wrap gap-2" onSubmit={event => {
          event.preventDefault();
          void action(async signal => { await enable(Number(port), signal); setPort(''); setLabel(''); });
        }}>
          <label className="min-w-0 flex-1 text-sm">{translate("devices.hTTPPort")}<input aria-label={translate("devices.hTTPPort")} className={inputClass} type="number" inputMode="numeric" min="1" max="65535" required value={port} onChange={event => setPort(event.target.value)} disabled={busy} /></label>
          <label className="min-w-0 flex-1 text-sm">{translate("devices.label")}<input aria-label={translate("devices.portLabel")} className={inputClass} maxLength={40} value={label} onChange={event => setLabel(event.target.value)} disabled={busy} /></label>
          <button className={`${buttonClass} self-end`} disabled={busy || available !== true}>{translate("devices.enable")}</button>
        </form>
        {mappings.length === 0 && available === true && <p className="host-muted text-sm">{translate("devices.noEnabledPorts")}</p>}
        <ul className="space-y-3">
          {mappings.map(mapping => <li key={mapping.id} className="rounded-md border p-3 space-y-2">
            <div className="break-words text-sm font-medium">{mapping.label ? `${mapping.label} · ` : ''}127.0.0.1:{mapping.port}</div>
            <div className="host-muted text-xs">{translate("devices.enabled")} {new Date(mapping.createdAt).toLocaleString(getLocale())}</div>
            <div className="flex flex-wrap gap-2">
              <button className={buttonClass} type="button" disabled={busy || available !== true} onClick={() => open(mapping, null)}>{translate("devices.open")}</button>
              <button className={buttonClass} type="button" disabled={busy || available !== true} onClick={() => void action(async signal => {
                const { url } = await launch(mapping, '/', signal);
                // Copy the stable address, never the short-lived login handoff.
                const address = new URL(url); address.searchParams.delete('__rc_launch');
                await navigator.clipboard.writeText(address.href); setCopied(mapping.id);
              })}>{copied === mapping.id ? translate("devices.copied") : translate("devices.copyAddress")}</button>
              <button className={buttonClass} type="button" disabled={busy} onClick={() => void action(async signal => {
                await request(api + '/' + mapping.id, { method: 'DELETE', signal });
                setMappings(items => items.filter(item => item.id !== mapping.id));
              })}>{translate("devices.stop")}</button>
            </div>
          </li>)}
        </ul>
        <p className="host-muted text-xs">{translate("devices.stoppingAMappingClosesItsConnectionsThe")}</p>
      </div>
    </FormDialog>}
    {link && <FormDialog title={translate("devices.openDeviceWebService")} description={translate("devices.enableAPrivateMappingFor1270", { value1: link.port })} busy={busy} onClose={() => setLink(null)}>
      <div className="space-y-4 min-w-0">
        <p className="host-muted text-sm break-all">{link.path}</p>
        {readiness}{errorMessage}
        <div className="flex justify-end gap-2">
          <button className={buttonClass} disabled={busy} onClick={() => setLink(null)}>{translate("devices.cancel")}</button>
          <button className={buttonClass} disabled={busy || available !== true} onClick={() => open()}>{busy ? translate("devices.opening") : translate("devices.enableAndOpen")}</button>
        </div>
      </div>
    </FormDialog>}
  </>;
}
