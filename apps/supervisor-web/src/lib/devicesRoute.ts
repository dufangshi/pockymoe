/**
 * The device center (issue #115): `/desktop/devices` lists the account's devices,
 * and `/desktop/devices/:id` is the one home of a device, on a computer, a phone
 * and in the native hosts alike. `/relay-devices` and `?action=` links still lead
 * here (DevicesEntryRoute, DevicesPage).
 */
export function devicesHref(query = '') {
  return `/desktop/devices${query}`;
}

/** The tabs of a device's page. */
export type DevicePageTab = 'overview' | 'settings' | 'access';

export function devicePageHref(deviceId: string, tab: DevicePageTab = 'overview', section?: string) {
  const query = new URLSearchParams();
  if (tab !== 'overview') query.set('tab', tab);
  if (tab === 'settings' && section) query.set('section', section);
  const search = query.toString();
  return `/desktop/devices/${encodeURIComponent(deviceId)}${search ? `?${search}` : ''}`;
}

/** Where a device's settings live: the Settings tab of its page, at `section` if given. */
export function deviceSettingsHref(deviceId: string, section?: string) {
  return devicePageHref(deviceId, 'settings', section);
}
