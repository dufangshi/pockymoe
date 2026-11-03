import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { RelayDeviceDto } from '@pockymoe/shared';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { ProtectDevicesButton } from '../../components/RemoteProtectionControls';
import { useDeviceProtection } from '../../lib/useDeviceProtection';
import { getTransportStatus } from '../../lib/relayTransport';

export function DevicesPageTabs({ devices, shared, pending }: { devices: number; shared: number; pending: number }) {
  const { t } = useI18n();
  const [params, setParams] = useSearchParams();
  const tab = params.get('tab') === 'shared' ? 'shared' : 'devices';
  const choose = (next: string) => setParams(previous => { const value = new URLSearchParams(previous); if (next === 'shared') value.set('tab', next); else value.delete('tab'); return value; });
  return <div className="pd-segment devices-page-tabs" role="tablist" aria-label={t('sharing.pageTabs')}>{(['devices', 'shared'] as const).map(id => <button key={id} id={`devices-page-tab-${id}`} role="tab" aria-selected={tab === id} aria-controls={`devices-page-panel-${id}`} tabIndex={tab === id ? 0 : -1} onClick={() => choose(id)} onKeyDown={event => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault(); const next = event.key === 'Home' ? 'devices' : event.key === 'End' ? 'shared' : tab === 'devices' ? 'shared' : 'devices'; choose(next); document.getElementById(`devices-page-tab-${next}`)?.focus();
  }}>{t(id === 'devices' ? 'devices.devices' : 'sharing.tabSharing')}<span className="devices-page-tab-count">{id === 'devices' ? devices : shared}</span>{id === 'shared' && pending ? <span className="devices-page-tab-review">{t('sharing.toReview', { count: pending })}</span> : null}</button>)}</div>;
}

function ProtectionProbe({ id, report }: { id: string; report: (id: string, unprotected: boolean) => void }) {
  const [transport, setTransport] = useState(() => getTransportStatus(id));
  useEffect(() => {
    const update = (event: Event) => { const next = (event as CustomEvent<NonNullable<typeof transport>>).detail; if (next.deviceId === id) setTransport(next); };
    window.addEventListener('remote-codex-transport', update);
    return () => window.removeEventListener('remote-codex-transport', update);
  }, [id]);
  const protection = useDeviceProtection(id, transport ?? undefined);
  useEffect(() => { report(id, protection === 'not-protected' || protection === 'certificate-missing' || protection === 'pending'); }, [id, protection, report]);
  return null;
}
export function UnprotectedDevicesButton({ devices }: { devices: RelayDeviceDto[] }) {
  const [unprotected, setUnprotected] = useState<Record<string, boolean>>({});
  const [report] = useState(() => (id: string, value: boolean) => setUnprotected(previous => previous[id] === value ? previous : { ...previous, [id]: value }));
  return <>{devices.map(device => <ProtectionProbe key={device.id} id={device.id} report={report} />)}{devices.some(device => unprotected[device.id]) ? <ProtectDevicesButton devices={devices} all /> : null}</>;
}
