import { translate } from '@pockymoe/thread-ui/i18n';
import { ArrowRight, ChartColumn, LayoutDashboard, Lock, Pencil, Plus, Settings, Share2, Trash2 } from 'lucide-react';
import type { ManagedAction } from './ManagedObjectMenu';
import { devicePageHref, deviceSettingsHref } from '../../lib/devicesRoute';

/** The actions that open a window in place, on the page the reader is on. */
export type DeviceManageAction = 'rename' | 'share' | 'isolate' | 'remove';
export type DeviceActionId = 'open' | 'new' | 'details' | 'reveal' | 'settings' | DeviceManageAction;

export interface DeviceActionOptions {
  navigate(path: string): void;
  /** Open this action's window where the reader is; the page does not change. */
  manage(action: DeviceManageAction): void;
  /** False once the relay refused a rename (an older relay): the item stays, greyed, with the reason. */
  renameSupported?: boolean;
  /** Only where the surface has a thread map to show the device in. */
  reveal?: (() => void) | undefined;
  /** "Open" as this surface does it (the phone list also remembers the device); a plain link otherwise. */
  open?: (() => void) | undefined;
  /** False on the device's own page, which is "Device details" already. */
  details?: boolean;
}

/**
 * A device's actions, the same on every surface that lists it (the card, the
 * phone row, the row on its own page, the map), in the order shared with
 * `workspaceActions`: open and start work, then rename / share / isolate, then
 * settings, and removal last, behind a separator.
 */
export function deviceActions(device: { id: string; name: string; connected: boolean }, options: DeviceActionOptions): ManagedAction[] {
  const id = encodeURIComponent(device.id);
  const offline = device.connected ? undefined : translate('devices.manageOffline');
  const unavailable = (reason?: string) => reason ? { disabled: true, disabledReason: reason } : {};
  return [
    // An offline device still opens: its workspaces page says it is offline.
    { id: 'open', label: translate('devices.manageOpen'), icon: <ArrowRight />, onSelect: options.open ?? (() => options.navigate(`/devices/${id}/workspaces`)) },
    { id: 'new', label: translate('devices.manageNewThread'), icon: <Plus />, onSelect: () => options.navigate(`/devices/${id}/threads/new`), ...unavailable(offline) },
    ...(options.details === false ? [] : [{ id: 'details', label: translate('desktopDevices.deviceDetails'), icon: <ChartColumn />, onSelect: () => options.navigate(devicePageHref(device.id)) }]),
    ...(options.reveal ? [{ id: 'reveal', label: translate('desktopDevices.revealInMap'), icon: <LayoutDashboard />, onSelect: options.reveal }] : []),
    { id: 'rename', label: translate('devices.manageRename'), icon: <Pencil />, onSelect: () => options.manage('rename'), ...unavailable(options.renameSupported === false ? translate('desktopDevices.renameUnsupported') : undefined) },
    { id: 'share', label: translate('devices.manageShare'), icon: <Share2 />, onSelect: () => options.manage('share') },
    { id: 'isolate', label: translate('devices.manageIsolate'), icon: <Lock />, onSelect: () => options.manage('isolate') },
    // Settings are the Settings tab of the device's page, its one home (issue #115).
    { id: 'settings', label: translate('devices.manageSettings'), icon: <Settings />, onSelect: () => options.navigate(deviceSettingsHref(device.id)) },
    { id: 'remove', label: translate('devices.manageRemove'), icon: <Trash2 />, onSelect: () => options.manage('remove'), separator: true, tone: 'danger' },
  ];
}
