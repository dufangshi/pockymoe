import { useContext, useSyncExternalStore } from 'react';
import { UNSAFE_LocationContext } from 'react-router-dom';
import { relayModeActive } from './api';
import { currentRelayDeviceIdFromPath, relayDeviceIdFromPath, settingsDeviceId } from './relayRoutes';

// The device chosen in the settings window on a page that shows no single device
// (the map, the devices overview, account pages). It lives only in this page's
// memory and is always shown by name: the window never acts on a device read
// back from storage (issue #112).
let chosen: string | null = null;
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };

export function chooseSettingsDevice(deviceId: string | null) {
  if (chosen === deviceId) return;
  chosen = deviceId;
  for (const listener of listeners) listener();
}

// The router's location where there is one (a retained route keeps its own);
// a settings panel mounted outside any router follows the address bar.
function useRoutedPathname() {
  return useContext(UNSAFE_LocationContext)?.location?.pathname;
}

/** The device whose pages (`/devices/:id/...`) this panel is on, if any. */
export function useRoutedDeviceId() {
  const routed = useRoutedPathname();
  return routed === undefined ? currentRelayDeviceIdFromPath() : relayDeviceIdFromPath(routed);
}

export interface SettingsDeviceTarget {
  /** Null: this Supervisor (local mode), or no device chosen yet (Relay mode). */
  deviceId: string | null;
  /** The page itself names the device, so the window cannot switch to another. */
  fromPage: boolean;
  /** Relay mode with no device: there is nothing to act on until one is chosen. */
  missing: boolean;
}

/**
 * The device a device-level settings panel acts on, in this order: the explicit
 * `deviceId` its owner passes, the device of the page, the device chosen in the
 * window. `explicit` null means this Supervisor.
 */
export function useSettingsDevice(explicit?: string | null): SettingsDeviceTarget {
  const routed = useRoutedPathname();
  const choice = useSyncExternalStore(subscribe, () => chosen, () => null);
  if (explicit !== undefined) return { deviceId: explicit, fromPage: true, missing: false };
  const fromPage = settingsDeviceId(routed);
  if (fromPage) return { deviceId: fromPage, fromPage: true, missing: false };
  if (!relayModeActive()) return { deviceId: null, fromPage: false, missing: false };
  return { deviceId: choice, fromPage: false, missing: !choice };
}
