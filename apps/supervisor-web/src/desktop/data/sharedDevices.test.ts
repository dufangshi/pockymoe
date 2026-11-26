import { expect, it } from 'vitest';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import { sharedDevices } from './store';

// Issue #116: devices shared with the account were missing from the device
// switcher, because the index keeps only owned devices.
const grant = (deviceId: string, extra: object = {}) => ({ id: `g-${deviceId}`, ownerUserId: 'other', ownerUsername: 'blair', deviceId, deviceName: `Device ${deviceId}`, deviceConnected: true, revokedAt: null, ...extra });

it('lists each device shared with the account once, with who shared it, never an owned or pending one', () => {
  const portal = {
    devices: [{ id: 'mine', ownerUserId: 'me', name: 'Mine', connected: true }],
    sharedDevicesWithMe: [grant('a'), grant('mine'), grant('waiting', { pending: true }), grant('gone', { revokedAt: '2026-11-01T00:00:00Z' })],
    sharedThreadsWithMe: [grant('a'), grant('b', { deviceConnected: false, ownerUsername: 'casey' })],
    sharedWithMe: [{ ...grant('c'), deviceName: 'Cello' }],
  } as unknown as RelayPortalSummaryDto;
  expect(sharedDevices(portal, 'me', new Set(['mine']))).toEqual([
    { id: 'c', name: 'Cello', connected: true, ownerName: 'blair' },
    { id: 'a', name: 'Device a', connected: true, ownerName: 'blair' },
    { id: 'b', name: 'Device b', connected: false, ownerName: 'casey' },
  ]);
});

it('reads an older relay\'s portal without the shared lists', () => {
  expect(sharedDevices({ devices: [] } as unknown as RelayPortalSummaryDto, 'me', new Set())).toEqual([]);
});
