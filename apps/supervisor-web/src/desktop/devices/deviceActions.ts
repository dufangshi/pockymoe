import { translate } from '@pockymoe/thread-ui/i18n';
import type { MapDeviceMenuItem } from '@pockymoe/thread-ui';
import type { DesktopDevice } from '../data';

/** The card and map expose the same management actions, in the same order. */
export function deviceActions(device: DesktopDevice, renameSupported: boolean, navigate: (path: string) => void, reveal: () => void): MapDeviceMenuItem[] {
  const detail = `/desktop/devices/${encodeURIComponent(device.id)}`;
  const offline = !device.connected ? translate('devices.deviceIsolationOffline') : undefined;
  const action = (id: string, label: string, path: string, reason?: string): MapDeviceMenuItem => ({ id, label, onSelect: () => navigate(path), ...(reason ? { disabled: true, disabledReason: reason } : {}) });
  return [
    { id: 'reveal', label: translate('desktopDevices.revealInMap'), onSelect: reveal },
    action('details', translate('desktopDevices.deviceDetails'), detail),
    action('workspaces', translate('desktopDevices.openWorkspaces'), `/devices/${encodeURIComponent(device.id)}/workspaces`),
    action('new', translate('desktopDevices.newThread'), `/devices/${encodeURIComponent(device.id)}/threads/new`, offline),
    action('check', translate('desktopDevices.checkUpdates'), `${detail}?action=update`, offline),
    action('rename', translate('desktopDevices.rename'), `${detail}?action=rename`, renameSupported ? undefined : translate('desktopDevices.renameUnsupported')),
    action('share', translate('desktopDevices.shareAction'), `${detail}?action=share`),
    action('isolate', translate('desktopDevices.isolate'), `${detail}?action=isolate`),
    { ...action('remove', translate('desktopDevices.remove'), `${detail}?action=remove`), separator: true, tone: 'danger' },
  ];
}
