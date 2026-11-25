import { useCallback, useState } from 'react';
import { DeviceManagement, type DeviceManagementAction } from '../DeviceManagement';
import type { DeviceManageAction } from './deviceActions';

/** What a page that lists a device already knows of it. */
export interface ListedDevice { id: string; name: string; connected: boolean }

/**
 * Where a page that lists devices (the Devices page, the map) opens their
 * management windows: `manage(device, action)` shows the rename, share,
 * isolation, settings or removal window over the page the reader is on, and
 * `windows` is the element to render for it. Pass `manage` to `deviceActions`.
 */
export function useDeviceActionHost({ onRemoved, onRenamed, onClosed }: {
  /** The device left the account. */
  onRemoved?: ((deviceId: string) => void) | undefined;
  onRenamed?: ((deviceId: string) => void) | undefined;
  /** A window closed: what it changed may need reading again. */
  onClosed?: (() => void) | undefined;
} = {}) {
  const [open, setOpen] = useState<{ device?: ListedDevice; action: DeviceManagementAction } | null>(null);
  // An older relay refuses renames; known after the first refusal.
  const [renameSupported, setRenameSupported] = useState(true);
  const manage = useCallback((device: ListedDevice, action: DeviceManageAction) => setOpen({ device: { id: device.id, name: device.name, connected: device.connected }, action }), []);
  const add = useCallback(() => setOpen({ action: 'add' }), []);
  const windows = open ? (
    <DeviceManagement
      key={`${open.device?.id ?? ''}:${open.action}`}
      embedded
      deviceId={open.device?.id}
      known={open.device}
      action={open.action}
      onRemoved={onRemoved}
      onRenamed={onRenamed}
      onRenameUnsupported={() => setRenameSupported(false)}
      onClose={() => { setOpen(null); onClosed?.(); }}
    />
  ) : null;
  return { manage, add, windows, renameSupported };
}
