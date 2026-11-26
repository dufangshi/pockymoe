import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { ArrowRight } from 'lucide-react';
import { ErrorMark, Select } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import { fetchRelayPortal, relayModeActive } from '../lib/api';
import { devicesHref, deviceSettingsHref } from '../lib/devicesRoute';
import { chooseSettingsDevice, useSettingsDevice } from '../lib/settingsDevice';
import { DeviceStateMark } from './DeviceStateMark';

interface NamedDevice { id: string; name: string; connected: boolean; owned: boolean }

function namedDevices(portal: RelayPortalSummaryDto): NamedDevice[] {
  const devices = new Map<string, NamedDevice>();
  for (const device of portal.devices) devices.set(device.id, { id: device.id, name: device.name, connected: device.connected, owned: true });
  for (const grant of [...(portal.sharedDevicesWithMe ?? []), ...(portal.sharedThreadsWithMe ?? [])]) {
    if (!devices.has(grant.deviceId)) devices.set(grant.deviceId, { id: grant.deviceId, name: grant.deviceName, connected: grant.deviceConnected === true, owned: false });
  }
  return [...devices.values()];
}

// The last answer, so moving between tabs does not blank the name while it loads again.
let lastDevices: NamedDevice[] | null = null;

/**
 * Which device the device-level settings below act on (issue #112). On a device's
 * page it names that device and its connection state; on a page without one it is
 * the choice of a device, in place of a hint with nowhere to go. Local mode has a
 * single Supervisor and shows nothing.
 */
export function SettingsDeviceBar() {
  useI18n();
  const relay = relayModeActive();
  const target = useSettingsDevice();
  const [devices, setDevices] = useState(lastDevices);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!relay) return;
    let alive = true;
    setError(null);
    fetchRelayPortal().then(portal => {
      if (!alive) return;
      lastDevices = namedDevices(portal);
      setDevices(lastDevices);
    }).catch((cause: unknown) => {
      if (alive) setError(cause instanceof Error ? cause.message : translate('devices.unableToLoadDevices'));
    });
    return () => { alive = false; };
  }, [relay, attempt]);
  const owned = devices?.filter(device => device.owned) ?? [];
  const onlyDevice = owned.length === 1 ? owned[0]!.id : null;
  useEffect(() => {
    if (!relay || target.fromPage || !devices) return;
    // One device leaves nothing to choose; a chosen device that left the account is no choice.
    if (!target.deviceId && onlyDevice) chooseSettingsDevice(onlyDevice);
    else if (target.deviceId && !devices.some(device => device.owned && device.id === target.deviceId)) chooseSettingsDevice(null);
  }, [relay, target.fromPage, target.deviceId, devices, onlyDevice]);
  if (!relay) return null;
  const current = devices?.find(device => device.id === target.deviceId);
  const state = current ? <DeviceStateMark state={current.connected ? 'online' : 'offline'} /> : null;
  const retry = error ? <ErrorMark message={error} onRetry={() => setAttempt(value => value + 1)} /> : null;
  const label = translate('settings.deviceScope');
  if (target.fromPage) {
    return (
      <div className="settings-device-bar" role="group" aria-label={label} data-testid="settings-device">
        <span className="settings-device-bar-label">{translate('devices.device')}</span>
        {/* The id stands in only until the name is read, or when it cannot be. */}
        <strong className="settings-device-bar-name" title={current?.name ?? target.deviceId ?? undefined}>{current?.name ?? target.deviceId}</strong>
        {state}
        {retry}
      </div>
    );
  }
  const sorted = [...owned].sort((a, b) => Number(b.connected) - Number(a.connected) || a.name.localeCompare(b.name));
  return (
    <>
      <div className="settings-device-bar" role="group" aria-label={label} data-testid="settings-device">
        <span className="settings-device-bar-label" id="settings-device-bar-label">{translate('devices.device')}</span>
        {sorted.length ? (
          <Select
            aria-labelledby="settings-device-bar-label"
            className="settings-device-bar-select"
            value={target.deviceId ?? ''}
            placeholder={translate('settings.notificationsChooseDevice')}
            options={sorted.map(device => ({ value: device.id, label: device.name }))}
            onValueChange={id => chooseSettingsDevice(id || null)}
          />
        ) : devices ? (
          <span className="min-w-0 text-[var(--theme-fg-muted)]">{translate('settings.deviceScopeNone')} <Link className="underline" to={devicesHref()}>{translate('devices.addDevice')}</Link></span>
        ) : !error ? (
          <span className="text-[var(--theme-fg-muted)]" role="status">{translate('files.loading')}</span>
        ) : <span className="text-[var(--theme-fg-muted)]">{translate('devices.unavailable')}</span>}
        {state}
        {retry}
      </div>
      {target.missing && sorted.length > 1 ? <p className="settings-note py-4 text-sm text-[var(--theme-fg-muted)]">{translate('settings.deviceScopeChoose')}</p> : null}
    </>
  );
}

/**
 * In Relay mode a device's own settings (Harness, upstreams, resources, isolated
 * environments, Supervisor, workspace defaults) are the Settings tab of its page
 * (issue #115). The settings window says where, in one line: the device of the
 * page, or the only one; otherwise all devices, to choose from.
 */
export function DeviceSettingsLink({ onNavigate }: { onNavigate: () => void }) {
  useI18n();
  const target = useSettingsDevice();
  const [devices, setDevices] = useState(lastDevices);
  useEffect(() => {
    let alive = true;
    fetchRelayPortal().then(portal => { lastDevices = namedDevices(portal); if (alive) setDevices(lastDevices); }).catch(() => undefined);
    return () => { alive = false; };
  }, []);
  const owned = devices?.filter(device => device.owned) ?? [];
  // A page of someone else's device has no settings of yours: the line lists yours.
  const device = owned.find(entry => entry.id === target.deviceId) ?? (owned.length === 1 ? owned[0] : undefined);
  return (
    <Link className="settings-device-link" to={device ? deviceSettingsHref(device.id) : devicesHref()} onClick={onNavigate} data-testid="settings-device-link">
      {device ? <DeviceStateMark showLabel={false} state={device.connected ? 'online' : 'offline'} /> : null}
      <span className="settings-device-link-text">{translate('settings.deviceSettingsLink', { device: device?.name ?? translate('desktopDevices.allDevices') })}</span>
      <ArrowRight aria-hidden="true" />
    </Link>
  );
}
