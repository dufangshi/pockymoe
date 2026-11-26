import { ChevronDown, LayoutGrid, Settings } from 'lucide-react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { DeviceStateMark } from './DeviceStateMark';
import { ManagedObjectMenu, type ManagedAction } from './managed/ManagedObjectMenu';
import { deviceSettingsHref } from '../lib/devicesRoute';
import { useContext, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { DesktopStoreContext } from '../desktop/data/context';
import { useDesktopSelector } from '../desktop/data';

/** A device the switcher lists: one of the account's own, or one shared with it. */
export interface SwitcherDevice {
  id: string;
  name: string;
  connected: boolean;
  /** Someone else's device: who shared it. Only opening it is offered here. */
  sharedBy?: string | undefined;
}

/**
 * The one device switcher, at the top of the thread, workspace and device
 * sidebars (issue #115): the current device with its connection state; the menu
 * lists the account's devices and those shared with it (marked), then
 * "Manage this device…" (its settings on its own page) and "All devices".
 * Choosing a device goes where this surface goes for a device (`hrefFor`).
 */
export function DeviceSwitcher({ devices, currentId, hrefFor, navigate, className = '' }: {
  devices: readonly SwitcherDevice[];
  /** Null on a page about every device (the devices overview). */
  currentId: string | null;
  hrefFor(deviceId: string): string;
  navigate(path: string): void;
  className?: string;
}) {
  useI18n();
  const current = devices.find(device => device.id === currentId);
  const name = current?.name ?? (currentId ? currentId.slice(0, 8) : translate('desktopDevices.allDevices'));
  const items: ManagedAction[] = [
    ...devices.map(device => ({
      id: `device:${device.id}`,
      label: device.name,
      icon: <DeviceStateMark showLabel={false} state={device.connected ? 'online' : 'offline'} />,
      ...(device.sharedBy !== undefined ? { detail: device.sharedBy ? translate('devices.sharedBy', { owner: device.sharedBy }) : translate('devices.sharedMark') } : {}),
      checked: device.id === currentId,
      onSelect: () => navigate(hrefFor(device.id)),
    })),
    // A shared device is managed by its owner: here it is only opened.
    ...(current && current.sharedBy === undefined ? [{ id: 'manage', label: translate('devices.manageThisDevice'), icon: <Settings />, separator: true, onSelect: () => navigate(deviceSettingsHref(current.id)) }] : []),
    { id: 'all', label: translate('desktopDevices.allDevices'), icon: <LayoutGrid />, separator: !(current && current.sharedBy === undefined), onSelect: () => navigate('/desktop/devices') },
  ];
  const state = current ? (current.connected ? 'online' : 'offline') : null;
  return (
    <ManagedObjectMenu
      name={name}
      items={items}
      className={`device-switcher ${className}`}
      trigger={{
        label: state ? translate('devices.switcherLabel', { device: name, state: translate(state === 'online' ? 'desktopDevices.online' : 'desktopDevices.offline') }) : translate('devices.switcherLabelAll', { device: name }),
        menuLabel: translate('devices.switcherMenu'),
        content: <>
          {state ? <DeviceStateMark showLabel={false} state={state} /> : null}
          <span className="managed-menu-switch-name">{name}</span>
          {current?.sharedBy !== undefined ? <span className="managed-menu-switch-shared">{translate('devices.sharedMark')}</span> : null}
          <ChevronDown aria-hidden="true" />
        </>,
      }}
    />
  );
}

/** The switcher over the account index of the computer's navigation shell; nothing without one (a phone, local mode). */
export function IndexedDeviceSwitcher(props: { currentId: string | null; hrefFor(deviceId: string): string; className?: string }) {
  return useContext(DesktopStoreContext) ? <IndexedSwitcher {...props} /> : null;
}

function IndexedSwitcher({ currentId, hrefFor, className }: { currentId: string | null; hrefFor(deviceId: string): string; className?: string | undefined }) {
  const navigate = useNavigate();
  const owned = useDesktopSelector(index => index.devices);
  const shared = useDesktopSelector(index => index.sharedDevices);
  const devices = useMemo<SwitcherDevice[]>(() => [
    ...[...owned].sort((a, b) => Number(b.connected) - Number(a.connected) || a.name.localeCompare(b.name)).map(({ id, name, connected }) => ({ id, name, connected })),
    ...(shared ?? []).map(({ id, name, connected, ownerName }) => ({ id, name, connected, sharedBy: ownerName })),
  ], [owned, shared]);
  return <DeviceSwitcher devices={devices} currentId={currentId} hrefFor={hrefFor} navigate={navigate} {...(className ? { className } : {})} />;
}
